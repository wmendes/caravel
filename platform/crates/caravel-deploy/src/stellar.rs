//! Everything the tool sends to Stellar goes through the pinned `stellar` CLI,
//! so keys stay in the user's keystore (secure store and Ledger included for
//! the admin). The tool reads identities' public keys, and exports only the
//! keys a host must hold (validators, relayer, feed keys).

use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

use crate::address::strkey;
use crate::manifest::{Manifest, Network};
use crate::plan::{Key, Params, SignerSet};
use crate::render::g;

pub struct Cli {
    net: Vec<String>,
}

fn hex(k: &[u8]) -> String {
    k.iter().map(|b| format!("{b:02x}")).collect()
}

/// Runs `stellar <args>` and returns its trimmed stdout.
fn run(args: &[String]) -> Result<String> {
    // CARAVEL_DEBUG=1 prints each command; no secret is ever an argument.
    if std::env::var_os("CARAVEL_DEBUG").is_some() {
        eprintln!("$ stellar {}", args.join(" "));
    }
    let out = Command::new("stellar")
        .args(args)
        .output()
        .context("running the stellar CLI (is it installed?)")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let all: Vec<&str> = err.lines().collect();
        let tail = all.len().saturating_sub(6);
        // The last lines, and every diagnostic line that names a contract
        // error (they say which contract raised it: the settlement, or the
        // token it called), in their order.
        let lines: Vec<&str> = all
            .iter()
            .enumerate()
            .filter(|(i, l)| *i >= tail || l.contains("Error(Contract, #"))
            .map(|(_, l)| *l)
            .collect();
        bail!(
            "stellar {} failed: {}",
            args.first().map(String::as_str).unwrap_or(""),
            lines.join("\n")
        );
    }
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

fn s(v: &str) -> String {
    v.to_string()
}

impl Cli {
    pub fn new(m: &Manifest) -> Self {
        let net = match &m.env.rpc_url {
            None => vec![s("--network"), s(m.env.network.name())],
            Some(url) => vec![
                s("--rpc-url"),
                url.clone(),
                s("--network-passphrase"),
                s(m.env.network.passphrase()),
            ],
        };
        Self { net }
    }

    /// The installed CLI must be the pinned one (versions.json).
    pub fn check_version() -> Result<()> {
        let have = Self::version()?;
        let want = crate::versions::stellar_cli();
        if have != want {
            bail!("stellar CLI {have} is installed; this tool needs {want} (versions.json)");
        }
        Ok(())
    }

    /// The installed Stellar CLI's version.
    pub fn version() -> Result<String> {
        let v = run(&[s("--version")])?;
        Ok(v.lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .unwrap_or("")
            .to_string())
    }

    /// Whether the keystore has this identity.
    pub fn has_identity(identity: &str) -> bool {
        run(&[s("keys"), s("public-key"), s(identity)]).is_ok()
    }

    /// Creates an identity in the keystore (a seed phrase, the CLI's
    /// default store); never replaces one.
    pub fn generate_identity(identity: &str) -> Result<Key> {
        if Self::has_identity(identity) {
            bail!("identity {identity:?} already exists");
        }
        run(&[s("keys"), s("generate"), s(identity)])
            .with_context(|| format!("creating identity {identity:?}"))?;
        Self::public_key(identity)
    }

    pub fn public_key(identity: &str) -> Result<Key> {
        let g = run(&[s("keys"), s("public-key"), s(identity)]).with_context(|| {
            format!("identity {identity:?} (create it with `stellar keys generate {identity}`)")
        })?;
        Ok(stellar_strkey::ed25519::PublicKey::from_string(&g)
            .map_err(|e| anyhow!("identity {identity:?} has no ed25519 key: {e:?}"))?
            .0)
    }

    /// The admin's public key, which every address of the lane derives
    /// from. A missing identity is to be added from the key, never generated:
    /// a new key would derive another lane's addresses.
    pub fn admin_public_key(identity: &str) -> Result<Key> {
        let g = run(&[s("keys"), s("public-key"), s(identity)]).map_err(|_| {
            anyhow!(
                "no identity {identity:?}: the lane's addresses derive from its admin's public key, so add it (`stellar keys add {identity} --public-key G…`)"
            )
        })?;
        Ok(stellar_strkey::ed25519::PublicKey::from_string(&g)
            .map_err(|e| anyhow!("identity {identity:?} has no ed25519 key: {e:?}"))?
            .0)
    }

    /// The identity's secret key, for a host that must hold it.
    pub fn secret(identity: &str) -> Result<String> {
        run(&[s("keys"), s("secret"), s(identity)]).with_context(|| {
            format!(
                "exporting identity {identity:?} (a Ledger or secure-store key can't be exported)"
            )
        })
    }

    /// `args` with the network flags, before any `--` (after it, the CLI
    /// passes arguments to the contract).
    fn with_net(&self, mut args: Vec<String>) -> Vec<String> {
        let at = args.iter().position(|a| a == "--").unwrap_or(args.len());
        args.splice(at..at, self.net.iter().cloned());
        args
    }

    /// Friendbot funds a new account (local and testnet only). On a fresh
    /// quickstart friendbot answers some time after RPC does, so it retries.
    pub fn fund(&self, identity: &str) -> Result<()> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        loop {
            match run(&self.with_net(vec![s("keys"), s("fund"), s(identity)])) {
                Ok(_) => return Ok(()),
                Err(e) if std::time::Instant::now() < deadline => {
                    eprintln!(
                        "  friendbot not ready ({}); retrying",
                        e.to_string().lines().last().unwrap_or("")
                    );
                    std::thread::sleep(std::time::Duration::from_secs(3));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Deploys the Stellar Asset Contract of `code:issuer`; the issuer signs.
    /// Deploys the Stellar Asset Contract of `code:issuer`; anyone may.
    pub fn deploy_asset(&self, source_identity: &str, code: &str, issuer: &Key) -> Result<()> {
        run(&self.with_net(vec![
            s("contract"),
            s("asset"),
            s("deploy"),
            s("--asset"),
            format!("{code}:{}", g(issuer)),
            s("--source-account"),
            s(source_identity),
        ]))
        .map(|_| ())
    }

    /// A token contract's `decimals()`, read by simulation (nothing is sent).
    pub fn token_decimals(&self, source: &str, token: &Key) -> Result<u32> {
        let out = run(&self.with_net(vec![
            s("contract"),
            s("invoke"),
            s("--id"),
            strkey(token),
            s("--source-account"),
            s(source),
            s("--send=no"),
            s("--"),
            s("decimals"),
        ]))?;
        out.trim()
            .trim_matches('"')
            .parse()
            .map_err(|_| anyhow!("{} answered decimals() with {out:?}", strkey(token)))
    }

    /// Uploads Wasm; returns its hash. Already-uploaded Wasm is skipped.
    pub fn upload(&self, source: &str, wasm: &Path) -> Result<Key> {
        let out = run(&self.with_net(vec![
            s("contract"),
            s("upload"),
            s("--wasm"),
            wasm.display().to_string(),
            s("--source-account"),
            s(source),
        ]))?;
        parse_hash(out.lines().last().unwrap_or(""))
    }

    /// Deploys the settlement contract from uploaded Wasm with a fixed salt.
    #[allow(clippy::too_many_arguments)]
    pub fn deploy_settlement(
        &self,
        admin_identity: &str,
        wasm_hash: &Key,
        salt: &Key,
        admin: &Key,
        token: &Key,
        ids: [&Key; 4],
        signers: &SignerSet,
        params: &Params,
    ) -> Result<Key> {
        let [lane_id, engine, genesis, config] = ids;
        let args = self.with_net(vec![
            s("contract"),
            s("deploy"),
            s("--wasm-hash"),
            hex(wasm_hash),
            s("--salt"),
            hex(salt),
            s("--source-account"),
            s(admin_identity),
            s("--"),
            s("--admin"),
            g(admin),
            // The settlement token: the M0 constructor names it `usdc`.
            s("--usdc"),
            strkey(token),
            s("--lane_id"),
            hex(lane_id),
            s("--engine_wasm_hash"),
            hex(engine),
            s("--genesis_state_hash"),
            hex(genesis),
            s("--config_hash"),
            hex(config),
            s("--signers"),
            signers_json(signers),
            s("--params"),
            params_json(params),
        ]);
        // RPC can answer a ledger behind a Wasm upload just sent.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let out = loop {
            match run(&args) {
                Err(e)
                    if e.to_string().contains("Code not found")
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                }
                r => break r?,
            }
        };
        let c = out.lines().last().unwrap_or("");
        Ok(stellar_strkey::Contract::from_string(c)
            .map_err(|_| anyhow!("stellar contract deploy printed {c:?}, not a contract address"))?
            .0)
    }

    /// `admin_rotate_signers(new)`: a testnet-only admin power.
    pub fn rotate_signers(
        &self,
        admin_identity: &str,
        contract: &Key,
        new: &SignerSet,
    ) -> Result<()> {
        run(&self.with_net(vec![
            s("contract"),
            s("invoke"),
            s("--id"),
            strkey(contract),
            s("--source-account"),
            s(admin_identity),
            s("--send=yes"),
            s("--"),
            s("admin_rotate_signers"),
            s("--new"),
            signers_json(new),
        ]))
        .map(|_| ())
    }

    /// `request_forced_withdrawal(owner, lane_account, amount)`, signed by `owner`.
    /// `request_forced_withdrawal`; the inbox index it took.
    pub fn request_forced_withdrawal(
        &self,
        owner_identity: &str,
        contract: &Key,
        owner: &Key,
        amount: i128,
    ) -> Result<u64> {
        let out = run(&self.with_net(vec![
            s("contract"),
            s("invoke"),
            s("--id"),
            strkey(contract),
            s("--source-account"),
            s(owner_identity),
            s("--send=yes"),
            s("--"),
            s("request_forced_withdrawal"),
            s("--owner"),
            g(owner),
            s("--lane_account"),
            hex(owner),
            s("--amount"),
            amount.to_string(),
        ]))?;
        out.trim()
            .trim_matches('"')
            .parse()
            .map_err(|_| anyhow!("request_forced_withdrawal returned {out:?}, not an inbox index"))
    }

    /// A SEP-53 signature of `message` by `identity` (Stellar CLI 28.1.0
    /// `message sign`, which adds the "Stellar Signed Message:\n" prefix and
    /// hashes): the 64 signature bytes.
    pub fn sign_message(identity: &str, message: &str) -> Result<[u8; 64]> {
        let out = run(&[
            s("message"),
            s("sign"),
            s(message),
            s("--sign-with-key"),
            s(identity),
        ])
        .with_context(|| format!("signing with identity {identity:?}"))?;
        let bytes = base64_decode(out.trim()).ok_or_else(|| {
            anyhow!("stellar message sign printed {out:?}, not a base64 signature")
        })?;
        bytes
            .try_into()
            .map_err(|b: Vec<u8>| anyhow!("a signature is 64 bytes, not {}", b.len()))
    }

    /// A trustline from `identity` to `CODE:issuer` (creating or keeping it).
    pub fn change_trust(&self, identity: &str, code: &str, issuer: &Key) -> Result<()> {
        run(&self.with_net(vec![
            s("tx"),
            s("new"),
            s("change-trust"),
            s("--source-account"),
            s(identity),
            s("--line"),
            format!("{code}:{}", g(issuer)),
        ]))
        .map(|_| ())
    }

    /// `mint` on a Stellar Asset Contract, by its issuer.
    pub fn mint(&self, issuer_identity: &str, token: &Key, to: &Key, amount: i128) -> Result<()> {
        run(&self.with_net(vec![
            s("contract"),
            s("invoke"),
            s("--id"),
            strkey(token),
            s("--source-account"),
            s(issuer_identity),
            s("--send=yes"),
            s("--"),
            s("mint"),
            s("--to"),
            g(to),
            s("--amount"),
            amount.to_string(),
        ]))
        .map(|_| ())
    }

    /// Buys exactly `amount` of `CODE:issuer` with XLM on the DEX, spending
    /// at most `max_xlm` stroops.
    pub fn buy_with_xlm(
        &self,
        identity: &str,
        to: &Key,
        code: &str,
        issuer: &Key,
        amount: i128,
        max_xlm: i128,
    ) -> Result<()> {
        run(&self.with_net(vec![
            s("tx"),
            s("new"),
            s("path-payment-strict-receive"),
            s("--source-account"),
            s(identity),
            s("--send-asset"),
            s("native"),
            s("--send-max"),
            max_xlm.to_string(),
            s("--destination"),
            g(to),
            s("--dest-asset"),
            format!("{code}:{}", g(issuer)),
            s("--dest-amount"),
            amount.to_string(),
        ]))
        .map(|_| ())
    }

    /// `deposit` into the settlement contract; the inbox index it took.
    pub fn deposit(&self, identity: &str, contract: &Key, from: &Key, amount: i128) -> Result<u64> {
        let out = run(&self.with_net(vec![
            s("contract"),
            s("invoke"),
            s("--id"),
            strkey(contract),
            s("--source-account"),
            s(identity),
            s("--send=yes"),
            s("--"),
            s("deposit"),
            s("--from"),
            g(from),
            s("--amount"),
            amount.to_string(),
            s("--lane_account"),
            hex(from),
        ]))?;
        out.trim()
            .trim_matches('"')
            .parse()
            .map_err(|_| anyhow!("deposit returned {out:?}, not an inbox index"))
    }

    /// A SEP-41 token balance (a read, nothing sent).
    pub fn token_balance(&self, source: &str, token: &Key, who: &Key) -> Result<i128> {
        self.token_balance_of(source, token, &g(who))
    }

    /// The token balance of any address (a G… account or a C… contract).
    pub fn token_balance_of(&self, source: &str, token: &Key, address: &str) -> Result<i128> {
        let out = run(&self.with_net(vec![
            s("contract"),
            s("invoke"),
            s("--id"),
            strkey(token),
            s("--source-account"),
            s(source),
            s("--send=no"),
            s("--"),
            s("balance"),
            s("--id"),
            s(address),
        ]))?;
        out.trim()
            .trim_matches('"')
            .parse()
            .map_err(|_| anyhow!("balance returned {out:?}"))
    }

    /// `escape_claim` for one leaf of the last checkpoint, paid to the lane
    /// account's owner; anyone may send it once the lane is frozen.
    pub fn escape_claim(
        &self,
        source: &str,
        contract: &Key,
        owner: &Key,
        index: u32,
        equity: &str,
        proof: &[String],
    ) -> Result<()> {
        run(&self.with_net(vec![
            s("contract"),
            s("invoke"),
            s("--id"),
            strkey(contract),
            s("--source-account"),
            s(source),
            s("--send=yes"),
            s("--"),
            s("escape_claim"),
            s("--recipient"),
            g(owner),
            s("--lane_account"),
            hex(owner),
            s("--index"),
            index.to_string(),
            s("--equity"),
            s(equity),
            s("--proof"),
            serde_json::to_string(proof)?,
        ]))
        .map(|_| ())
    }

    /// `claim_withdrawal` for one withdrawal leaf, paid to its owner.
    #[allow(clippy::too_many_arguments)]
    pub fn claim_withdrawal(
        &self,
        source: &str,
        contract: &Key,
        owner: &Key,
        seq: u64,
        index: u32,
        amount: &str,
        proof: &[String],
    ) -> Result<()> {
        run(&self.with_net(vec![
            s("contract"),
            s("invoke"),
            s("--id"),
            strkey(contract),
            s("--source-account"),
            s(source),
            s("--send=yes"),
            s("--"),
            s("claim_withdrawal"),
            s("--recipient"),
            g(owner),
            s("--lane_account"),
            hex(owner),
            s("--seq"),
            seq.to_string(),
            s("--index"),
            index.to_string(),
            s("--amount"),
            s(amount),
            s("--proof"),
            serde_json::to_string(proof)?,
        ]))
        .map(|_| ())
    }

    /// `freeze()`: anyone may call it once the contract allows it.
    pub fn freeze(&self, source_identity: &str, contract: &Key) -> Result<()> {
        run(&self.with_net(vec![
            s("contract"),
            s("invoke"),
            s("--id"),
            strkey(contract),
            s("--source-account"),
            s(source_identity),
            s("--send=yes"),
            s("--"),
            s("freeze"),
        ]))
        .map(|_| ())
    }
}

fn parse_hash(h: &str) -> Result<Key> {
    let v: Vec<u8> = (0..h.len() / 2)
        .map(|i| u8::from_str_radix(h.get(2 * i..2 * i + 2).unwrap_or("zz"), 16))
        .collect::<Result<_, _>>()
        .map_err(|_| anyhow!("{h:?} is not a hash"))?;
    v.try_into().map_err(|_| anyhow!("{h:?} is not 32 bytes"))
}

/// The constructor's `WeightedSigners`, as the CLI takes it.
pub fn signers_json(set: &SignerSet) -> String {
    serde_json::json!({
        "signers": set.signers.iter().map(|(k, w)| serde_json::json!({ "key": hex(k), "weight": w })).collect::<Vec<_>>(),
        "threshold": set.threshold,
    })
    .to_string()
}

pub fn params_json(p: &Params) -> String {
    serde_json::json!({
        "force_inclusion_window_secs": p.force_inclusion_window_secs,
        "escape_timeout_secs": p.escape_timeout_secs,
        "min_rotation_delay_secs": p.min_rotation_delay_secs,
        "signer_retention_epochs": p.signer_retention_epochs,
        "min_deposit": p.min_deposit.to_string(),
    })
    .to_string()
}

/// Whether the local network answers, and starts it (`stellar container start
/// local`) when it doesn't.
pub fn ensure_local_network(m: &Manifest) -> Result<()> {
    if m.env.network != Network::Local {
        return Ok(());
    }
    let healthy = || {
        Command::new("curl")
            .args([
                "-sf",
                "-X",
                "POST",
                m.rpc_url(),
                "-H",
                "content-type: application/json",
                "-d",
                r#"{"jsonrpc":"2.0","id":1,"method":"getHealth"}"#,
            ])
            .output()
            .is_ok_and(|o| {
                o.status.success() && String::from_utf8_lossy(&o.stdout).contains("healthy")
            })
    };
    if healthy() {
        return Ok(());
    }
    eprintln!("starting a local Stellar network (stellar container start local)");
    run(&[
        s("container"),
        s("start"),
        s("local"),
        s("--limits"),
        s("testnet"),
    ])?;
    for _ in 0..180 {
        if healthy() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    bail!("the local network did not become healthy in 180 s")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_flags_go_before_the_contract_arguments() {
        let cli = Cli {
            net: vec![s("--network"), s("local")],
        };
        assert_eq!(
            cli.with_net(vec![s("contract"), s("invoke"), s("--"), s("freeze")]),
            ["contract", "invoke", "--network", "local", "--", "freeze"]
        );
        assert_eq!(
            cli.with_net(vec![s("keys"), s("fund"), s("a")]),
            ["keys", "fund", "a", "--network", "local"]
        );
    }
}

/// Standard base64 (with padding), for the one signature the CLI prints.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let t = text.as_bytes();
    if t.is_empty() || !t.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(t.len() / 4 * 3);
    for (i, chunk) in t.chunks(4).enumerate() {
        let last = i == t.len() / 4 - 1;
        let pad = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for &c in &chunk[..4 - pad] {
            n = (n << 6) | val(c)?;
        }
        n <<= 6 * pad as u32;
        let b = n.to_be_bytes();
        out.extend_from_slice(&b[1..4 - pad]);
    }
    Some(out)
}

#[cfg(test)]
mod base64_tests {
    use super::base64_decode;

    #[test]
    fn decodes() {
        assert_eq!(base64_decode("TWFu").unwrap(), b"Man");
        assert_eq!(base64_decode("TWE=").unwrap(), b"Ma");
        assert_eq!(base64_decode("TQ==").unwrap(), b"M");
        // The signature `stellar message sign` printed (88 characters).
        let sig = "5Xpt2LJYc1aBvZY9x8EazheLTWzFXI5uXrkNITkKRqosen48H6Pe/2b4q7zcnkY6VwUKyS+HbITmg89NOcF6Dw==";
        assert_eq!(base64_decode(sig).unwrap().len(), 64);
        for bad in ["", "TWF", "T===", "TW=u", "TW*u"] {
            assert!(base64_decode(bad).is_none(), "{bad}");
        }
    }
}
