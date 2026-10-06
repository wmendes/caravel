//! A lane's users, from the CLI (M0.6, C-12 and C-13; DEC-084, DEC-085):
//! accounts with the settlement token, deposits that return once credited,
//! lane transactions signed with SEP-53 through the Stellar CLI keystore,
//! withdrawals and their claims, forced withdrawals, and escapes from a
//! frozen lane. Keys stay in the keystore; a user needs only the lane file
//! and the admin's public key (the settlement address derives from it).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use caravel_core::tx::{sep53_tx_message, SigScheme, StandardBody, TxEnvelopeV1};
use caravel_node::scval::{self, variant};
use caravel_node::stellar_rpc::{account_key, persistent_key, Rpc};
use serde_json::{json, Value};
use stellar_xdr::{ScBytes, ScMap, ScVal};

use crate::address::strkey;
use crate::deploy::{addresses, api_url, validator_url, Addresses};
use crate::lifecycle::{get, get_opt, post};
use crate::manifest::{Manifest, Network, Token};
use crate::plan::Key;
use crate::stellar::Cli;

/// A token amount in token units ("12.5") as base units.
pub fn parse_units(s: &str, decimals: u32) -> Result<i128> {
    caravel_node::plugin::parse_amount(s, Some(decimals))
}

/// Base units as token units: `125000000` with 7 decimals is `12.5`.
pub fn format_units(v: i128, decimals: u32) -> String {
    let scale = 10i128.pow(decimals);
    let (sign, v) = if v < 0 { ("-", -v) } else { ("", v) };
    let whole = v / scale;
    let frac = v % scale;
    if frac == 0 {
        return format!("{sign}{whole}");
    }
    let f = format!("{frac:0width$}", width = decimals as usize);
    format!("{sign}{whole}.{}", f.trim_end_matches('0'))
}

fn g(k: &Key) -> String {
    caravel_runtime::views::g_address(k)
}

/// The `Error(Contract, #N)` a stellar CLI failure reports, and the contract
/// that raised it. The host's event log is newest first, and a token's error
/// inside a settlement call shows again in the settlement's frame ("contract
/// call failed", then the escalation to a trap), so the raiser is the oldest
/// error event: the highest `N:` index.
fn contract_error(text: &str) -> Option<(Option<String>, u32)> {
    const ERR: &str = "Error(Contract, #";
    let code = |l: &str| -> Option<u32> { l.split(ERR).nth(1)?.split(')').next()?.parse().ok() };
    let raiser = |l: &str| {
        l.split("contract:").nth(1).and_then(|rest| {
            let id: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            (id.len() == 56 && id.starts_with('C')).then_some(id)
        })
    };
    let index = |l: &str| -> Option<u64> { l.trim_start().split(':').next()?.parse().ok() };
    // Of equal indexes (none given), the last line: the oldest.
    if let Some(l) = text
        .lines()
        .filter(|l| l.contains(ERR) && raiser(l).is_some())
        .max_by_key(|l| index(l))
    {
        return Some((raiser(l), code(l)?));
    }
    let l = text.lines().find(|l| l.contains(ERR))?;
    Some((None, code(l)?))
}

/// The settlement contract's errors (spec §13).
fn settlement_error(n: u32) -> Option<&'static str> {
    Some(match n {
        10 => "the lane is frozen: use `caravel escape`",
        11 => "below the lane's minimum deposit",
        12 => "the amount must be positive",
        13 => "only the lane account's owner may do this",
        40 => "a signer rotation came too soon after the last one",
        41 => "this signer set was used before",
        50 => "the withdrawal belongs to another recipient",
        51 => "no such checkpoint on Stellar yet",
        52 | 54 => "the proof doesn't match the checkpoint",
        53 => "already claimed",
        60 => "the lane isn't frozen",
        61 => "the contract doesn't allow a freeze yet",
        62 | 63 => "nothing to refund for that inbox message",
        64 => "the escape payout does not compute: the claim is refused and stays open",
        _ => return None,
    })
}

/// A Stellar Asset Contract's errors (soroban-env-host 28.0.2
/// `builtin_contracts/contract_error.rs`).
fn token_error(n: u32) -> Option<&'static str> {
    Some(match n {
        4 | 5 => "the token refused the authorization",
        6 => "the account isn't on the network",
        10 => "not enough of the token",
        11 => "the token's issuer hasn't authorized this trustline",
        13 => "no trustline to the token: `caravel account fund <identity>`",
        14 => "not enough XLM for the account's reserve",
        _ => return None,
    })
}

/// What a contract error in a stellar CLI failure means: the token's codes
/// when the token raised it, the settlement's otherwise (or when unknown).
pub fn explain(text: &str, settlement: &Key, token: &Key) -> Option<&'static str> {
    let (raiser, n) = contract_error(text)?;
    match raiser {
        Some(id) if id == strkey(token) => token_error(n),
        Some(id) if id == strkey(settlement) => settlement_error(n),
        Some(_) => None,
        None => settlement_error(n),
    }
}

/// A platform receipt code's name (an app's codes are its own).
pub fn receipt_name(code: u16) -> String {
    use caravel_core::codes::receipt::*;
    let name = match code {
        OK => "OK",
        WRONG_LANE => "WRONG_LANE",
        UNKNOWN_ACCOUNT => "UNKNOWN_ACCOUNT",
        UNAUTHORIZED_SIGNER => "UNAUTHORIZED_SIGNER",
        EXPIRED => "EXPIRED",
        BAD_NONCE => "BAD_NONCE",
        RATE_LIMITED => "RATE_LIMITED",
        BELOW_MIN_WITHDRAWAL => "BELOW_MIN_WITHDRAWAL (below the lane's minimum withdrawal)",
        INSUFFICIENT_FREE_BALANCE => {
            "INSUFFICIENT_FREE_BALANCE (more than the account's free balance)"
        }
        WITHDRAWAL_QUEUE_FULL => "WITHDRAWAL_QUEUE_FULL",
        INSUFFICIENT_LANE_LIQUIDITY => "INSUFFICIENT_LANE_LIQUIDITY",
        TOO_MANY_SESSION_KEYS => "TOO_MANY_SESSION_KEYS",
        BAD_SESSION_KEY => "BAD_SESSION_KEY",
        c => return format!("the app's code {c}"),
    };
    format!("{name} ({code})")
}

/// Why a sent transaction never made a block.
pub const DROPPED: &str = "no block took it before it expired: the lane dropped it (its nonce was used, or is ahead of the account's) or set it aside";

/// A JSON number, or a number in a string (the API's u64s).
fn num(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// `exit.json` as JSON.
pub fn read_exit(path: &std::path::Path) -> std::result::Result<Value, String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .ok_or_else(|| format!("{} doesn't read as JSON", path.display()))
}

/// Whether an `exit.json` holds the proofs of Stellar's last checkpoint
/// `last` (seq and header hash): the only one escapes are proven against
/// after a freeze. `Err` says what it is for instead.
pub fn exit_matches(e: &Value, last: (u64, [u8; 32])) -> std::result::Result<(), String> {
    let seq = num(&e["seq"]);
    let hash = e["header_hash"].as_str().unwrap_or_default();
    let want = caravel_runtime::sequencer::hex(&last.1);
    if seq == Some(last.0) && hash == want {
        return Ok(());
    }
    Err(format!(
        "is for checkpoint {} ({}), Stellar's last is {} ({})",
        seq.map_or("?".into(), |s| s.to_string()),
        hash.get(..16).unwrap_or(hash),
        last.0,
        &want[..16]
    ))
}

/// One withdrawal leaf of an accepted checkpoint (`/v1/proofs/withdrawals`,
/// `exit.json`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leaf {
    pub seq: u64,
    pub index: u32,
    pub account: Key,
    pub amount: i128,
    pub proof: Vec<String>,
}

impl Leaf {
    fn parse(v: &Value) -> Result<Self> {
        let s = |k: &str| v[k].as_str().ok_or_else(|| anyhow!("leaf: no {k}"));
        Ok(Self {
            seq: s("seq")?.parse().context("leaf seq")?,
            index: v["index"]
                .as_u64()
                .ok_or_else(|| anyhow!("leaf: no index"))? as u32,
            account: caravel_node::lane_toml::parse_account(s("account")?)?,
            amount: s("amount")?.parse().context("leaf amount")?,
            proof: v["proof"]
                .as_array()
                .ok_or_else(|| anyhow!("leaf: no proof"))?
                .iter()
                .map(|p| {
                    p.as_str()
                        .map(String::from)
                        .ok_or_else(|| anyhow!("leaf: bad proof"))
                })
                .collect::<Result<_>>()?,
        })
    }

    fn json(&self, decimals: u32) -> Value {
        json!({ "seq": self.seq, "index": self.index, "amount": format_units(self.amount, decimals) })
    }
}

/// What claiming leaves did, leaf by leaf.
#[derive(Debug, Default)]
pub struct Claims {
    pub paid: Vec<Leaf>,
    /// Claimed by someone else meanwhile (a claim always pays the owner).
    pub already: Vec<Leaf>,
    pub failed: Vec<(Leaf, String)>,
}

impl Claims {
    pub fn json(&self, decimals: u32) -> Value {
        let list = |ls: &[Leaf]| ls.iter().map(|l| l.json(decimals)).collect::<Vec<_>>();
        json!({
            "paid": list(&self.paid),
            "already_claimed": list(&self.already),
            "failed": self.failed.iter().map(|(l, e)| {
                let mut v = l.json(decimals);
                v["error"] = json!(e);
                v
            }).collect::<Vec<_>>(),
        })
    }
}

/// A lane transaction sent: what `tx` waits on.
pub struct Sent {
    pub account: Key,
    pub nonce: u64,
    pub expiry_ms: u64,
    pub tx_hash: String,
    pub hex: String,
    /// The lane's height before it was sent.
    pub from_height: u64,
}

/// What made a withdrawal leaf: a lane transaction (its hex), or an inbox
/// message (its index).
#[derive(Clone, Copy, Debug)]
enum Mine<'a> {
    Tx(&'a str),
    Inbox(u64),
}

/// How a sent transaction ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Included {
        height: u64,
        code: u16,
        events: Vec<String>,
    },
    /// The lane dropped it (it expired before a block took it).
    Dropped,
    /// Still not in a block when the wait ran out.
    Pending,
}

/// The lane's freeze, from Stellar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Freeze {
    pub at: u64,
    pub payout_num: i128,
    pub payout_den: i128,
    /// The checkpoint every escape proves against (`LastCkpt`).
    pub last_seq: u64,
    pub last_header_hash: [u8; 32],
}

/// A deployment, seen by its users.
/// How often a flow looks again while it waits on the lane (K-09).
const POLL: Duration = Duration::from_millis(250);
/// Blocks fetched at once when a flow reads a checkpoint's blocks (K-09).
const BLOCK_FETCHES: usize = 16;

pub struct Flows {
    pub m: Manifest,
    pub admin: Key,
    pub addrs: Addresses,
    pub cli: Cli,
    /// `exit.json`, if the operator's `destroy` wrote it here: proofs when
    /// the lane's nodes are gone.
    pub exit_file: Option<PathBuf>,
}

impl Flows {
    pub fn new(m: Manifest) -> Result<Self> {
        let admin = Cli::admin_public_key(&m.env.admin)?;
        let addrs = addresses(&m, &admin)?;
        let cli = Cli::new(&m);
        Ok(Self {
            m,
            admin,
            addrs,
            cli,
            exit_file: None,
        })
    }

    fn explained(&self, e: anyhow::Error) -> anyhow::Error {
        let text = format!("{e:#}");
        match explain(&text, &self.addrs.settlement, &self.addrs.token) {
            Some(why) => e.context(why),
            None => e,
        }
    }

    /// The settlement token's decimals: 7 for a Stellar asset, else asked.
    pub fn decimals(&self) -> Result<u32> {
        match self.m.env.token {
            Token::Contract { .. } => self
                .cli
                .token_decimals(&self.m.env.admin, &self.addrs.token),
            _ => Ok(7),
        }
    }

    /// The lane's API, where users reach it.
    pub fn api(&self) -> Result<String> {
        api_url(&self.m).ok_or_else(|| {
            anyhow!("the host has no public_url: the lane's API isn't reachable from here (pass --no-wait, or use the lane's URL)")
        })
    }

    /// The Stellar asset behind the settlement token, and whether the lane
    /// file's admin issues it (so it can mint).
    fn asset(&self) -> Option<(String, Key, bool)> {
        match &self.m.env.token {
            Token::Named(_) => {
                let issuer = stellar_strkey::ed25519::PublicKey::from_string(
                    crate::versions::testnet_usdc_issuer(),
                )
                .ok()?
                .0;
                Some(("USDC".into(), issuer, false))
            }
            _ => self
                .addrs
                .token_asset
                .clone()
                .map(|(code, issuer)| (code, issuer, issuer == self.admin)),
        }
    }

    fn rpc(&self) -> Result<Rpc> {
        Rpc::new(self.m.rpc_url())
    }

    async fn account_exists(&self, who: &Key) -> Result<bool> {
        Ok(self.rpc()?.ledger_entry(&account_key(who)).await?.is_some())
    }

    /// An identity ready to use the lane: on the network (friendbot), with a
    /// trustline to the settlement token, and `amount` of it (minted by the
    /// admin when the admin issues it; bought with XLM on the DEX for
    /// Circle's testnet USDC; else funding is up to its issuer).
    pub async fn fund(&self, identity: &str, amount: Option<i128>, max_xlm: i128) -> Result<Value> {
        let who = Cli::public_key(identity)?;
        let mut did = Vec::new();
        if !self.account_exists(&who).await? {
            self.cli.fund(identity)?;
            did.push("funded with XLM by friendbot".to_string());
            let deadline = Instant::now() + Duration::from_secs(60);
            while !self.account_exists(&who).await? {
                if Instant::now() > deadline {
                    bail!("{identity} isn't on the network a minute after friendbot");
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
        let decimals = self.decimals()?;
        match self.asset() {
            Some((code, issuer, _)) if issuer != who => {
                self.cli
                    .change_trust(identity, &code, &issuer)
                    .map_err(|e| self.explained(e))?;
                did.push(format!("trusts {code}:{}", g(&issuer)));
            }
            _ => {}
        }
        if let Some(amount) = amount {
            match (self.asset(), self.m.env.network) {
                (Some((_, _, true)), _) => {
                    self.cli
                        .mint(&self.m.env.admin, &self.addrs.token, &who, amount)
                        .map_err(|e| self.explained(e))?;
                    did.push(format!("{} minted by the admin", format_units(amount, decimals)));
                }
                (Some((code, issuer, false)), Network::Testnet) if matches!(self.m.env.token, Token::Named(_)) => {
                    self.cli
                        .buy_with_xlm(identity, &who, &code, &issuer, amount, max_xlm)
                        .with_context(|| format!("buying {} {code} on the testnet DEX", format_units(amount, decimals)))?;
                    did.push(format!("{} {code} bought with XLM on the DEX", format_units(amount, decimals)));
                }
                _ => bail!(
                    "the lane file's admin doesn't issue the settlement token ({}): get it from its issuer, then deposit",
                    strkey(&self.addrs.token)
                ),
            }
        }
        Ok(json!({
            "identity": identity,
            "account": g(&who),
            "did": did,
            "token": self.token_balance(identity, &who).ok().map(|b| format_units(b, decimals)),
        }))
    }

    /// A token balance, read with `source` as the simulation's account.
    pub fn token_balance(&self, source: &str, who: &Key) -> Result<i128> {
        self.cli.token_balance(source, &self.addrs.token, who)
    }

    /// An account's balance on Stellar and its lane account. `lane_error`
    /// says why there's no lane account when the API didn't answer.
    pub async fn balance(&self, source: &str, who: &Key) -> Result<Value> {
        let decimals = self.decimals()?;
        let (stellar, stellar_error) = match self.token_balance(source, who) {
            Ok(b) => (Some(format_units(b, decimals)), None),
            // The token's "no trustline" (#13) is an answer: none of it.
            Err(e) => match contract_error(&format!("{e:#}")) {
                Some((raiser, 13))
                    if raiser
                        .as_ref()
                        .is_none_or(|r| *r == strkey(&self.addrs.token)) =>
                {
                    (None, None)
                }
                _ => (None, Some(format!("{e:#}"))),
            },
        };
        let (lane, lane_error) = match self.api() {
            Ok(api) => match get_opt(&api, &format!("/v1/accounts/{}", g(who))).await {
                Ok(v) => (v, None),
                Err(e) => (None, Some(format!("{e:#}"))),
            },
            Err(e) => (None, Some(format!("{e:#}"))),
        };
        Ok(json!({
            "account": g(who),
            "stellar": stellar,
            "stellar_error": stellar_error,
            "lane": lane,
            "lane_error": lane_error,
        }))
    }

    /// A contract's balance of the settlement token (the settlement's own is
    /// what the lane holds on Stellar), read with the admin as the
    /// simulation's source.
    pub fn contract_balance(&self, contract: &Key) -> Result<Value> {
        let b =
            self.cli
                .token_balance_of(&self.m.env.admin, &self.addrs.token, &strkey(contract))?;
        Ok(json!({ "contract": strkey(contract), "stellar": format_units(b, self.decimals()?) }))
    }

    /// Deposits `amount`; with `wait`, polls until the lane has processed it.
    /// The result says whether it was credited, bounced (the lane refused
    /// it; it comes back as a withdrawal) or is still pending (`credited`
    /// false, `timed_out` true).
    pub async fn deposit(
        &self,
        identity: &str,
        amount: i128,
        wait: Option<Duration>,
    ) -> Result<Value> {
        let who = Cli::public_key(identity)?;
        let decimals = self.decimals()?;
        let min = self.m.min_deposit()?;
        if amount < min {
            bail!(
                "the lane's minimum deposit is {}; {} is below it",
                format_units(min, decimals),
                format_units(amount, decimals)
            );
        }
        let have = self.token_balance(identity, &who).map_err(|e| {
            anyhow!("{identity} can't hold the settlement token yet ({e:#}): `caravel account fund {identity}`")
        })?;
        if have < amount {
            bail!(
                "{identity} holds {} of the token, less than {}: `caravel account fund {identity} --amount …`",
                format_units(have, decimals),
                format_units(amount, decimals)
            );
        }
        // Where to watch, before anything is sent.
        let api = match wait {
            Some(_) => Some(self.api()?),
            None => None,
        };
        let path = format!("/v1/accounts/{}", g(&who));
        let existed = match &api {
            Some(api) => get_opt(api, &path).await?.is_some(),
            None => false,
        };
        let index = self
            .cli
            .deposit(identity, &self.addrs.settlement, &who, amount)
            .map_err(|e| self.explained(e))?;
        let mut out = json!({
            "account": g(&who),
            "amount": format_units(amount, decimals),
            "inbox_index": index,
            "credited": false,
            "bounced": false,
            "timed_out": false,
        });
        let (Some(timeout), Some(api)) = (wait, api) else {
            return Ok(out);
        };
        let deadline = Instant::now() + timeout;
        loop {
            if processed(&api).await.is_some_and(|p| p > index) {
                let lane = get_opt(&api, &path).await?;
                // A new account that still isn't there was refused (it
                // comes back as a withdrawal leaf); an account that existed
                // is always credited.
                if lane.is_none() && !existed {
                    out["bounced"] = json!(true);
                } else {
                    out["credited"] = json!(true);
                    out["lane"] = lane.unwrap_or(Value::Null);
                }
                return Ok(out);
            }
            if Instant::now() > deadline {
                out["timed_out"] = json!(true);
                return Ok(out);
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// The lane's config hash and height, checking it is this lane.
    async fn lane_status(&self, api: &str) -> Result<([u8; 32], u64)> {
        let s = get(api, "/v1/status")
            .await
            .context("the lane's sequencer")?;
        let hex32 = |k: &str| -> Result<[u8; 32]> {
            caravel_runtime::sequencer::unhex(s[k].as_str().unwrap_or_default())
                .and_then(|v| v.try_into().ok())
                .ok_or_else(|| anyhow!("/v1/status has no {k}"))
        };
        if hex32("lane_id")? != self.m.lane.lane_id() {
            bail!("the sequencer at {api} runs another lane");
        }
        let height = s["height"]
            .as_str()
            .and_then(|h| h.parse().ok())
            .unwrap_or(0);
        Ok((hex32("config_hash")?, height))
    }

    /// Builds an envelope around `kind`/`body` for `identity`'s lane
    /// account, signs it with SEP-53 through the Stellar CLI keystore (no key
    /// leaves it), checks the signature, and sends it. The nonce is `nonce`,
    /// else the one after the account's queued transactions.
    pub async fn send(
        &self,
        identity: &str,
        kind: u8,
        body: Vec<u8>,
        expiry_secs: u64,
        nonce_given: Option<u64>,
    ) -> Result<Sent> {
        let api = self.api()?;
        let who = Cli::public_key(identity)?;
        let (config_hash, from_height) = self.lane_status(&api).await?;
        // Two senders can read the same nonce at once: the sequencer refuses
        // the second (NONCE_QUEUED), which then signs the next one.
        let mut attempts = 0;
        let (nonce, expiry_ms, hex, reply) = loop {
            attempts += 1;
            let account = get_opt(&api, &format!("/v1/accounts/{}", g(&who)))
                .await?
                .ok_or_else(|| {
                    anyhow!("{identity} has no lane account: `caravel deposit {identity} …` first")
                })?;
            let nonce = match nonce_given {
                Some(n) => n,
                None => num(&account["next_nonce"])
                    .ok_or_else(|| anyhow!("the lane account has no next_nonce"))?
                    .max(num(&account["pending_nonce"]).unwrap_or(0)),
            };
            let expiry_ms = caravel_node::sequencer::now_ms() + expiry_secs * 1000;
            let mut tx = TxEnvelopeV1 {
                lane_id: self.m.lane.lane_id(),
                account: who,
                // SEP-53 is only valid when the owner signs.
                signer: who,
                nonce,
                expiry_ms,
                kind,
                sig_scheme: SigScheme::Sep53,
                body: body.clone(),
                signature: [0; 64],
            };
            let tx_hash = caravel_runtime::checkpoint::sha256(&tx.tx_hash_preimage(&config_hash));
            let message = sep53_tx_message(&tx_hash);
            let message = std::str::from_utf8(&message).expect("the SEP-53 message is ASCII");
            tx.signature = Cli::sign_message(identity, message)?;
            if !caravel_runtime::mempool::tx_signature_ok(&tx, &config_hash) {
                bail!("{identity}'s signature doesn't verify for its account: is it an ed25519 key in the keystore?");
            }
            let bytes = tx
                .encode()
                .ok_or_else(|| anyhow!("the transaction is too large"))?;
            let hex = caravel_runtime::sequencer::hex(&bytes);
            let (status, reply) = post(&api, "/v1/tx", &json!({ "tx": hex })).await?;
            if status == 202 {
                break (nonce, expiry_ms, hex, reply);
            }
            let code = reply["code"].as_str().unwrap_or("?");
            if code == "NONCE_QUEUED" && nonce_given.is_none() && attempts < 5 {
                continue;
            }
            bail!(
                "the sequencer refused the transaction: {} ({code})",
                reply["error"].as_str().unwrap_or("no reason given"),
            );
        };
        Ok(Sent {
            account: who,
            nonce,
            expiry_ms,
            tx_hash: reply["tx_hash"].as_str().unwrap_or_default().to_string(),
            hex,
            from_height,
        })
    }

    /// Waits for `sent` in a block (scanning the blocks after it was sent for
    /// its exact bytes): its receipt code and events.
    pub async fn included(&self, sent: &Sent, timeout: Duration) -> Result<Outcome> {
        let api = self.api()?;
        let deadline = Instant::now() + timeout;
        let mut next = sent.from_height + 1;
        loop {
            // A failed read (a restart, a dropped connection) is retried
            // until the deadline: the transaction is already sent.
            let head = match get(&api, "/v1/status").await {
                Ok(s) => num(&s["height"]).unwrap_or(0),
                Err(_) => 0,
            };
            while next <= head {
                let Ok(b) = get(&api, &format!("/v1/blocks/{next}")).await else {
                    break;
                };
                for e in b["entries"].as_array().into_iter().flatten() {
                    if e["type"] == "user" && e["hex"].as_str() == Some(sent.hex.as_str()) {
                        return Ok(Outcome::Included {
                            height: next,
                            code: e["code"].as_u64().unwrap_or(0) as u16,
                            events: e["events"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect(),
                        });
                    }
                }
                if num(&b["timestamp_ms"]).unwrap_or(0) > sent.expiry_ms {
                    // Past its expiry, no block can take it any more.
                    return Ok(Outcome::Dropped);
                }
                next += 1;
            }
            if Instant::now() > deadline {
                return Ok(Outcome::Pending);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// `who`'s withdrawal leaves that a node serves (accepted checkpoints).
    async fn node_leaves(&self, base: &str, who: &Key) -> Result<Vec<Leaf>> {
        let v = get(base, &format!("/v1/proofs/withdrawals?account={}", g(who))).await?;
        let mut out = Vec::new();
        for l in v["withdrawals"].as_array().into_iter().flatten() {
            let l = Leaf::parse(l)?;
            if l.account == *who {
                out.push(l);
            }
        }
        Ok(out)
    }

    /// The account's withdrawal leaves in accepted checkpoints, from the
    /// sequencer, else a validator, else `exit.json` (when it is for
    /// Stellar's last checkpoint).
    pub async fn leaves(&self, who: &Key) -> Result<Vec<Leaf>> {
        let mut tried = Vec::new();
        let mut sources: Vec<String> = self.api().into_iter().collect();
        sources.extend((0..self.m.env.validators.len()).filter_map(|i| validator_url(&self.m, i)));
        // A node's list counts when it has every checkpoint Stellar accepted
        // (a sequencer whose relayer died before reporting one lags); else the
        // node closest to it, with a warning.
        let last = self.last_checkpoint().await?;
        let mut behind: Option<(u64, String)> = None;
        for base in sources {
            let accepted = match get(&base, "/v1/status").await {
                Ok(s) => num(&s["checkpoints"]["accepted"]).unwrap_or(0),
                Err(e) => {
                    tried.push(format!("{e:#}"));
                    continue;
                }
            };
            if accepted < last.0 {
                tried.push(format!(
                    "{base} has checkpoints through {accepted}, Stellar through {}",
                    last.0
                ));
                if behind.as_ref().is_none_or(|(a, _)| accepted > *a) {
                    behind = Some((accepted, base));
                }
                continue;
            }
            match self.node_leaves(&base, who).await {
                Ok(ls) => return Ok(ls),
                Err(e) => tried.push(format!("{e:#}")),
            }
        }
        if self.exit_file.as_ref().is_some_and(|p| p.exists()) {
            match self.exit_doc(last) {
                Ok(Some(e)) => {
                    return e["withdrawals"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|l| l["account"].as_str() == Some(g(who).as_str()))
                        .map(Leaf::parse)
                        .collect()
                }
                Ok(None) => {}
                Err(why) => tried.push(why),
            }
        }
        if let Some((accepted, base)) = behind {
            if let Ok(ls) = self.node_leaves(&base, who).await {
                eprintln!(
                    "warning: no node has Stellar's checkpoint {} yet; {base} has them through {accepted}, so later withdrawals aren't listed",
                    last.0
                );
                return Ok(ls);
            }
        }
        bail!(
            "no node answered for the withdrawal proofs: {}",
            tried.join("; ")
        )
    }

    /// Which of `leaves` are claimed on Stellar (`Claimed(seq, index)`).
    pub async fn claimed(&self, leaves: &[Leaf]) -> Result<Vec<bool>> {
        let mut out = Vec::with_capacity(leaves.len());
        for chunk in leaves.chunks(100) {
            let keys: Vec<_> = chunk
                .iter()
                .map(|l| {
                    persistent_key(
                        &self.addrs.settlement,
                        variant("Claimed", vec![ScVal::U64(l.seq), ScVal::U32(l.index)]),
                    )
                })
                .collect();
            let (entries, _) = self.rpc()?.ledger_entries(&keys).await?;
            out.extend(entries.iter().map(Option::is_some));
        }
        Ok(out)
    }

    pub async fn unclaimed(&self, who: &Key) -> Result<Vec<Leaf>> {
        let leaves = self.leaves(who).await?;
        let claimed = self.claimed(&leaves).await?;
        Ok(leaves
            .into_iter()
            .zip(claimed)
            .filter(|(_, c)| !c)
            .map(|(l, _)| l)
            .collect())
    }

    /// Whether a failure is the settlement's "already claimed" (#53).
    fn already_claimed(&self, e: &anyhow::Error) -> bool {
        matches!(
            contract_error(&format!("{e:#}")),
            Some((raiser, 53)) if raiser.as_ref().is_none_or(|r| *r == strkey(&self.addrs.settlement))
        )
    }

    /// Claims leaves for their owner, sending as `identity`: every one, past
    /// any that fails. A leaf someone claimed meanwhile is already paid (a
    /// claim always pays the owner).
    pub fn claim(&self, identity: &str, leaves: &[Leaf]) -> Claims {
        let mut c = Claims::default();
        for l in leaves {
            match self.cli.claim_withdrawal(
                identity,
                &self.addrs.settlement,
                &l.account,
                l.seq,
                l.index,
                &l.amount.to_string(),
                &l.proof,
            ) {
                Ok(()) => c.paid.push(l.clone()),
                Err(e) if self.already_claimed(&e) => c.already.push(l.clone()),
                Err(e) => c
                    .failed
                    .push((l.clone(), format!("{:#}", self.explained(e)))),
            }
        }
        c
    }

    /// Claims (with `claim`) the leaf a command's withdrawal made, among
    /// `candidates`: the account's leaves of its amount in its checkpoint, in
    /// leaf order. With `rank` (from [`Flows::queued_by`]) that is exactly
    /// the `rank`-th; without it, any of them pays the same, so the first not
    /// yet claimed on Stellar, moving past one claimed meanwhile. `out` gets
    /// the leaf, and `claimed` (only for a claim this command paid),
    /// `claim_error`, or `already_claimed`.
    async fn take_leaf(
        &self,
        identity: &str,
        candidates: &[Leaf],
        rank: Option<usize>,
        claim: bool,
        decimals: u32,
        out: &mut Value,
    ) {
        let pool = match rank.and_then(|r| candidates.get(r)) {
            Some(l) => std::slice::from_ref(l),
            None => candidates,
        };
        let claimed = self
            .claimed(pool)
            .await
            .unwrap_or_else(|_| vec![false; pool.len()]);
        let open: Vec<&Leaf> = pool
            .iter()
            .zip(claimed)
            .filter(|(_, c)| !c)
            .map(|(l, _)| l)
            .collect();
        for l in &open {
            out["leaf"] = l.json(decimals);
            if !claim {
                return;
            }
            let c = self.claim(identity, std::slice::from_ref(*l));
            if !c.paid.is_empty() {
                out["claimed"] = json!(true);
                return;
            }
            if let Some((_, e)) = c.failed.into_iter().next() {
                out["claimed"] = json!(false);
                out["claim_error"] = json!(e);
                return;
            }
            // Claimed by someone else meanwhile: the next one.
        }
        if let Some(l) = pool.first() {
            out["leaf"] = l.json(decimals);
        }
        out["already_claimed"] = json!(true);
        out["note"] = json!("already claimed on Stellar (a claim always pays the account's owner)");
    }

    /// What `mine` queued for `who` in checkpoint `seq`, and how many of the
    /// account's pushes of that amount came before it there. Every push to
    /// the pending queue becomes a leaf, in order (spec §11.8), and a push is
    /// a WITHDRAW with code 0, a bounced deposit, or a forced withdrawal that
    /// queued something (their platform events, DEC-052), so that is the
    /// leaf's rank among the account's leaves of that amount. Reads the
    /// checkpoint's blocks and receipts; `None` if the deadline comes first
    /// or `mine` isn't there.
    async fn queued_by(
        &self,
        api: &str,
        seq: u64,
        who: &Key,
        mine: Mine<'_>,
        upto: Option<u64>,
        deadline: Instant,
    ) -> Option<(i128, usize)> {
        let read = |path: String| async move {
            loop {
                if let Ok(v) = get(api, &path).await {
                    return Some(v);
                }
                if Instant::now() > deadline {
                    return None;
                }
                tokio::time::sleep(POLL).await;
            }
        };
        let c = read(format!("/v1/checkpoints/{seq}")).await?;
        let (first, last) = (
            num(&c["first_block_height"])?,
            num(&c["last_block_height"])?,
        );
        let last = upto.map_or(last, |u| u.min(last));
        // Fetched BLOCK_FETCHES at a time, scanned in order (K-09).
        use futures_util::StreamExt;
        let mut blocks = futures_util::stream::iter(first..=last)
            .map(|h| read(format!("/v1/blocks/{h}")))
            .buffered(BLOCK_FETCHES);
        let mut before: Vec<i128> = Vec::new();
        while let Some(b) = blocks.next().await {
            match scan_block(&b?, who, mine, &mut before) {
                Ok(Some(found)) => return Some(found),
                Ok(None) => {}
                Err(_) => return None,
            }
        }
        None
    }

    /// The first of the sequencer's checkpoints that `is` holds for, where
    /// `is` is false up to some checkpoint and true from it on; waits for the
    /// lane to seal it. Sealed checkpoints are binary-searched, and failed
    /// reads retried until the deadline.
    async fn first_checkpoint(
        &self,
        api: &str,
        is: impl Fn(&Value) -> bool,
        deadline: Instant,
    ) -> Option<u64> {
        let read = |n: u64| async move {
            get_opt(api, &format!("/v1/checkpoints/{n}"))
                .await
                .ok()
                .flatten()
        };
        // `is` is false below `lo`.
        let mut lo = 1u64;
        loop {
            let sealed = match get(api, "/v1/status").await {
                Ok(s) => num(&s["checkpoints"]["sequenced"]),
                Err(_) => None,
            };
            if let Some(mut hi) = sealed.filter(|h| *h >= lo) {
                match read(hi).await {
                    Some(c) if is(&c) => {
                        while lo < hi {
                            let mid = lo + (hi - lo) / 2;
                            match read(mid).await {
                                Some(c) if is(&c) => hi = mid,
                                Some(_) => lo = mid + 1,
                                None if Instant::now() > deadline => return None,
                                None => tokio::time::sleep(POLL).await,
                            }
                        }
                        return Some(hi);
                    }
                    Some(_) => lo = hi + 1,
                    None => {}
                }
            }
            if Instant::now() > deadline {
                return None;
            }
            tokio::time::sleep(POLL).await;
        }
    }

    /// `who`'s leaves in checkpoint `seq`, once the sequencer has it accepted
    /// on Stellar (none then means none); `None` if the deadline comes first.
    async fn leaves_in(
        &self,
        api: &str,
        who: &Key,
        seq: u64,
        deadline: Instant,
    ) -> Option<Vec<Leaf>> {
        loop {
            let accepted = match get(api, "/v1/status").await {
                Ok(s) => num(&s["checkpoints"]["accepted"]),
                Err(_) => None,
            };
            if accepted.is_some_and(|a| a >= seq) {
                if let Ok(ls) = self.node_leaves(api, who).await {
                    return Some(ls.into_iter().filter(|l| l.seq == seq).collect());
                }
            }
            if Instant::now() > deadline {
                return None;
            }
            tokio::time::sleep(POLL).await;
        }
    }

    /// Withdraws `amount` from the lane: a WITHDRAW transaction, then (with
    /// `wait`) its leaf in an accepted checkpoint, then (with `claim`) the
    /// claim on Stellar. Every withdrawal pending at a checkpoint's end is
    /// one of its leaves (spec §11.8), so the leaf is in the checkpoint whose
    /// blocks hold the transaction's.
    pub async fn withdraw(
        &self,
        identity: &str,
        amount: i128,
        wait: Option<Duration>,
        claim: bool,
    ) -> Result<Value> {
        let who = Cli::public_key(identity)?;
        let decimals = self.decimals()?;
        let body = StandardBody::Withdraw { amount };
        let sent = self
            .send(identity, body.kind(), body.encode(), 60, None)
            .await?;
        eprintln!(
            "Sent the withdrawal: {} (nonce {}).",
            sent.tx_hash, sent.nonce
        );
        let mut out = json!({
            "account": g(&who),
            "amount": format_units(amount, decimals),
            "tx_hash": sent.tx_hash,
            "nonce": sent.nonce.to_string(),
        });
        let Some(timeout) = wait else {
            return Ok(out);
        };
        let deadline = Instant::now() + timeout;
        let height = match self.included(&sent, timeout).await? {
            Outcome::Included {
                height, code: 0, ..
            } => height,
            Outcome::Included { code, .. } => {
                bail!("the lane refused the withdrawal: {}", receipt_name(code))
            }
            Outcome::Dropped => bail!("the withdrawal: {DROPPED}"),
            Outcome::Pending => {
                out["timed_out"] = json!(true);
                return Ok(out);
            }
        };
        out["height"] = json!(height);
        let api = self.api()?;
        let in_ckpt = |c: &Value| num(&c["last_block_height"]).is_some_and(|l| l >= height);
        let Some(seq) = self.first_checkpoint(&api, in_ckpt, deadline).await else {
            out["timed_out"] = json!(true);
            return Ok(out);
        };
        let Some(mine) = self.leaves_in(&api, &who, seq, deadline).await else {
            out["timed_out"] = json!(true);
            return Ok(out);
        };
        let candidates: Vec<Leaf> = mine.into_iter().filter(|l| l.amount == amount).collect();
        if candidates.is_empty() {
            bail!(
                "checkpoint {seq} is accepted, but has no withdrawal of {} for {}",
                format_units(amount, decimals),
                g(&who)
            );
        }
        // Which leaf is this one matters only when the account has several of
        // the same amount in the checkpoint; then the blocks up to this one's
        // say (K-09: a quiet lane's checkpoint can span hundreds of blocks).
        let rank = if candidates.len() == 1 {
            None
        } else {
            self.queued_by(&api, seq, &who, Mine::Tx(&sent.hex), Some(height), deadline)
                .await
                .filter(|(a, _)| *a == amount)
                .map(|(_, r)| r)
        };
        self.take_leaf(identity, &candidates, rank, claim, decimals, &mut out)
            .await;
        Ok(out)
    }

    /// A forced withdrawal through the settlement contract (when the lane
    /// won't take a WITHDRAW): the lane must process it within its inclusion
    /// window, and queues at most what the account can withdraw (maybe
    /// nothing). Its leaf is in the checkpoint whose inbox range holds it.
    pub async fn force_withdraw(
        &self,
        identity: &str,
        amount: i128,
        wait: Option<Duration>,
        claim: bool,
    ) -> Result<Value> {
        let who = Cli::public_key(identity)?;
        let decimals = self.decimals()?;
        // Where to watch, before anything is sent.
        let api = match wait {
            Some(_) => Some(self.api()?),
            None => None,
        };
        let index = self
            .cli
            .request_forced_withdrawal(identity, &self.addrs.settlement, &who, amount)
            .map_err(|e| self.explained(e))?;
        eprintln!("Requested the forced withdrawal: inbox message {index}.");
        let mut out = json!({ "account": g(&who), "requested": format_units(amount, decimals), "inbox_index": index });
        let (Some(timeout), Some(api)) = (wait, api) else {
            return Ok(out);
        };
        let deadline = Instant::now() + timeout;
        let in_ckpt = |c: &Value| num(&c["header"]["inbox_through"]).is_some_and(|t| t > index);
        let Some(seq) = self.first_checkpoint(&api, in_ckpt, deadline).await else {
            out["timed_out"] = json!(true);
            out["note"] = json!("the lane hasn't processed it in a checkpoint yet: it must within its inclusion window, or anyone may freeze the lane");
            return Ok(out);
        };
        out["processed"] = json!(true);
        // The lane queues at most what the account can withdraw.
        let Some((queued, rank)) = self
            .queued_by(&api, seq, &who, Mine::Inbox(index), None, deadline)
            .await
        else {
            out["timed_out"] = json!(true);
            return Ok(out);
        };
        out["queued"] = json!(format_units(queued, decimals));
        if queued <= 0 {
            out["note"] = json!("the lane processed it and queued nothing: the account had nothing free to withdraw");
            return Ok(out);
        }
        let Some(mine) = self.leaves_in(&api, &who, seq, deadline).await else {
            out["timed_out"] = json!(true);
            return Ok(out);
        };
        let candidates: Vec<Leaf> = mine.into_iter().filter(|l| l.amount == queued).collect();
        if candidates.is_empty() {
            bail!(
                "checkpoint {seq} is accepted, but has no withdrawal of {} for {}",
                format_units(queued, decimals),
                g(&who)
            );
        }
        self.take_leaf(identity, &candidates, Some(rank), claim, decimals, &mut out)
            .await;
        Ok(out)
    }

    /// The settlement contract's instance storage.
    async fn settlement_storage(&self) -> Result<ScMap> {
        self.rpc()?
            .instance_storage(&self.addrs.settlement)
            .await?
            .ok_or_else(|| {
                anyhow!(
                    "no settlement contract at {}",
                    strkey(&self.addrs.settlement)
                )
            })
    }

    /// Stellar's last accepted checkpoint (`LastCkpt`): its seq and header
    /// hash.
    pub async fn last_checkpoint(&self) -> Result<(u64, [u8; 32])> {
        last_of(&self.settlement_storage().await?)
    }

    /// The freeze, if the lane is frozen.
    pub async fn freeze(&self) -> Result<Option<Freeze>> {
        let storage = self.settlement_storage().await?;
        let Some(f) = scval::entry(&storage, &variant("Frozen", vec![])) else {
            return Ok(None);
        };
        let f = scval::map(f, "Frozen")?;
        let (last_seq, last_header_hash) = last_of(&storage)?;
        Ok(Some(Freeze {
            at: scval::u64_of(scval::field(f, "at")?, "at")?,
            payout_num: scval::i128_of(scval::field(f, "payout_num")?, "payout_num")?,
            payout_den: scval::i128_of(scval::field(f, "payout_den")?, "payout_den")?,
            last_seq,
            last_header_hash,
        }))
    }

    /// `exit.json`, if the operator's `destroy` wrote one here for Stellar's
    /// last checkpoint `last` (the same seq and header hash): `Err` says why
    /// a file that is there doesn't count (an earlier deployment's, say).
    fn exit_doc(&self, last: (u64, [u8; 32])) -> std::result::Result<Option<Value>, String> {
        let Some(path) = self.exit_file.as_ref().filter(|p| p.exists()) else {
            return Ok(None);
        };
        let e = read_exit(path)?;
        exit_matches(&e, last)
            .map(|()| Some(e))
            .map_err(|why| format!("{} {why}", path.display()))
    }

    async fn escape_claimed(&self, who: &Key) -> Result<bool> {
        let key = persistent_key(
            &self.addrs.settlement,
            variant(
                "EscapeClaimed",
                vec![ScVal::Bytes(ScBytes(
                    who.to_vec().try_into().expect("32 bytes"),
                ))],
            ),
        );
        Ok(self.rpc()?.ledger_entry(&key).await?.is_some())
    }

    /// The account's escape leaf of the last checkpoint: from `exit.json`,
    /// else a validator or the sequencer, each checked against Stellar's
    /// last checkpoint. `None` when exit.json has no leaf for the account.
    async fn escape_leaf(&self, who: &Key, f: &Freeze) -> Result<Option<(u32, i128, Vec<String>)>> {
        let parse = |v: &Value| -> Option<(u32, i128, Vec<String>)> {
            Some((
                v["index"].as_u64()? as u32,
                v["equity"].as_str()?.parse().ok()?,
                v["proof"]
                    .as_array()?
                    .iter()
                    .filter_map(|p| p.as_str().map(String::from))
                    .collect(),
            ))
        };
        let mut tried = Vec::new();
        match self.exit_doc((f.last_seq, f.last_header_hash)) {
            Ok(Some(e)) => {
                if let Some(p) = e["escape"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|p| p["account"].as_str() == Some(g(who).as_str()))
                    .and_then(parse)
                {
                    return Ok(Some(p));
                }
                return Ok(None);
            }
            Ok(None) => {}
            Err(why) => tried.push(why),
        }
        let mut sources: Vec<String> = (0..self.m.env.validators.len())
            .filter_map(|i| validator_url(&self.m, i))
            .collect();
        sources.extend(self.api());
        for base in sources {
            // A node at Stellar's last checkpoint that has no leaf for the
            // account (404) means it has none.
            let at_last = get(&base, "/v1/status")
                .await
                .is_ok_and(|s| num(&s["checkpoints"]["accepted"]) == Some(f.last_seq));
            match get_opt(&base, &format!("/v1/proofs/escape?account={}", g(who))).await {
                Ok(Some(v)) if num(&v["seq"]) == Some(f.last_seq) => {
                    if let Some(p) = parse(&v) {
                        return Ok(Some(p));
                    }
                }
                Ok(Some(v)) => tried.push(format!("{base} has checkpoint {}", v["seq"])),
                Ok(None) if at_last => return Ok(None),
                Ok(None) => tried.push(format!("{base} has no checkpoint {}", f.last_seq)),
                Err(e) => tried.push(format!("{e:#}")),
            }
        }
        bail!(
            "no escape proof for checkpoint {} ({}): `caravel replay --prove-escape {}` builds one from Stellar alone",
            f.last_seq,
            tried.join("; "),
            g(who)
        )
    }

    /// Escapes a frozen lane: claims the account's withdrawal leaves (unless
    /// told not to; best effort, reported leaf by leaf), then its share of
    /// the last checkpoint. An escape that would pay nothing is skipped, as
    /// claiming it would use it up, unless `allow_zero`.
    pub async fn escape(
        &self,
        identity: &str,
        withdrawals: bool,
        allow_zero: bool,
    ) -> Result<Value> {
        let who = Cli::public_key(identity)?;
        let decimals = self.decimals()?;
        let freeze = self.freeze().await?.ok_or_else(|| {
            anyhow!("the lane isn't frozen, so there is nothing to escape from: withdraw instead (a freeze becomes possible once the lane stops posting checkpoints; see `caravel status`)")
        })?;
        let balance = || self.token_balance(identity, &who);
        let start = balance().map_err(|e| {
            anyhow!("{identity} can't hold the settlement token ({e:#}): `caravel account fund {identity}` first")
        })?;
        let mut out = json!({ "account": g(&who) });
        // From here on Stellar changes: every outcome goes in the report,
        // which is returned whatever fails (the CLI exits 1 on an error in it).
        let mut before_escape = Some(start);
        if withdrawals {
            match self.unclaimed(&who).await {
                Ok(open) => {
                    let c = self.claim(identity, &open);
                    for (l, e) in &c.failed {
                        eprintln!(
                            "warning: claiming the withdrawal in checkpoint {} (leaf {}) failed: {e}",
                            l.seq, l.index
                        );
                    }
                    out["withdrawals"] = c.json(decimals);
                }
                Err(e) => {
                    eprintln!("warning: {identity}'s withdrawals couldn't be read ({e:#}): `caravel claim {identity}` once a node answers");
                    out["withdrawals_error"] = json!(format!("{e:#}"));
                }
            }
            before_escape = balance().ok();
            out["withdrawals_paid"] =
                json!(before_escape.map(|b| format_units(b - start, decimals)));
        }
        if let Err(e) = self
            .escape_share(identity, &who, &freeze, allow_zero, decimals, &mut out)
            .await
        {
            out["escape"] = json!("failed");
            out["escape_error"] = json!(format!("{e:#}"));
        }
        match balance() {
            Ok(after) => {
                out["escape_paid"] =
                    json!(before_escape.map(|b| format_units(after - b, decimals)));
                out["paid"] = json!(format_units(after - start, decimals));
            }
            Err(e) => out["balance_error"] = json!(format!("{e:#}")),
        }
        Ok(out)
    }

    /// The escape claim itself, recorded in `out`: `escape` is "claimed",
    /// "already claimed", "none" (no balance in the last checkpoint) or
    /// "skipped" (it would pay nothing).
    async fn escape_share(
        &self,
        identity: &str,
        who: &Key,
        freeze: &Freeze,
        allow_zero: bool,
        decimals: u32,
        out: &mut Value,
    ) -> Result<()> {
        if freeze.last_seq == 0 {
            out["escape"] = json!("none");
            out["note"] = json!("the lane froze before its first checkpoint: no account has a share, and every deposit comes back through the settlement's refund_unprocessed_deposit");
            return Ok(());
        }
        if self.escape_claimed(who).await? {
            out["escape"] = json!("already claimed");
            return Ok(());
        }
        let Some((index, equity, proof)) = self.escape_leaf(who, freeze).await? else {
            out["escape"] = json!("none");
            out["note"] = json!("the account has no balance in the lane's last checkpoint");
            return Ok(());
        };
        // What the contract pays (`escape_claim`, issue #145 S-01).
        let expected = if freeze.payout_den <= 0 || freeze.payout_num <= 0 || equity <= 0 {
            0
        } else {
            caravel_core::wide::mul_div_floor(equity, freeze.payout_num, freeze.payout_den)
                .map_err(|_| {
                    anyhow!(
                        "the escape payout does not compute (equity {equity}, payout {}/{}): the contract refuses the claim",
                        freeze.payout_num,
                        freeze.payout_den
                    )
                })?
        };
        out["equity"] = json!(format_units(equity, decimals));
        out["expected"] = json!(format_units(expected.max(0), decimals));
        if expected <= 0 && !allow_zero {
            out["escape"] = json!("skipped");
            out["note"] = json!(format!(
                "the escape would pay 0 (equity {}, payout {}/{}) and claiming it uses it up: --allow-zero claims it anyway",
                format_units(equity, decimals),
                freeze.payout_num,
                freeze.payout_den
            ));
            return Ok(());
        }
        match self.cli.escape_claim(
            identity,
            &self.addrs.settlement,
            who,
            index,
            &equity.to_string(),
            &proof,
        ) {
            Ok(()) => out["escape"] = json!("claimed"),
            // Someone claimed it for the owner meanwhile (`destroy --pay-out`).
            Err(e) if self.already_claimed(&e) => out["escape"] = json!("already claimed"),
            Err(e) => return Err(self.explained(e)),
        }
        Ok(())
    }
}

/// One block's part in [`Flows::queued_by`]: adds `who`'s pushes to the
/// pending queue before `mine` to `before`, and once the block holds `mine`,
/// returns what it queued and its rank among `before` of that amount.
fn scan_block(
    b: &Value,
    who: &Key,
    mine: Mine<'_>,
    before: &mut Vec<i128>,
) -> Result<Option<(i128, usize)>> {
    use caravel_core::inbox::InboxMsgV1;
    use caravel_core::receipts::{DepositOutcome, PlatformEvent, ReceiptsV1};
    use caravel_core::tx::kind;
    let unhex = |v: &Value| {
        caravel_runtime::sequencer::unhex(v.as_str().unwrap_or_default())
            .ok_or_else(|| anyhow!("not hex"))
    };
    let receipts =
        ReceiptsV1::decode(&unhex(&b["receipts_hex"])?).map_err(|e| anyhow!("receipts: {e:?}"))?;
    for e in b["entries"].as_array().into_iter().flatten() {
        let entry = num(&e["index"]).ok_or_else(|| anyhow!("entry index"))? as u32;
        let bytes = unhex(&e["hex"]).unwrap_or_default();
        let is_mine = match (e["type"].as_str(), mine) {
            (Some("user"), Mine::Tx(hex)) => e["hex"].as_str() == Some(hex),
            (Some("inbox"), Mine::Inbox(i)) => {
                InboxMsgV1::decode(&bytes).is_ok_and(|m| m.index == i)
            }
            _ => false,
        };
        let mut pushed = Vec::new();
        if e["type"] == "user" && num(&e["code"]) == Some(0) {
            if let Ok(tx) = TxEnvelopeV1::decode(&bytes) {
                if let (true, Some(Ok(StandardBody::Withdraw { amount }))) = (
                    tx.account == *who && tx.kind == kind::WITHDRAW,
                    StandardBody::decode(tx.kind, &tx.body),
                ) {
                    pushed.push(amount);
                }
            }
        }
        let events = receipts
            .receipts
            .iter()
            .filter(|r| r.entry_index == entry)
            .flat_map(|r| &r.events)
            .filter_map(|ev| ev.platform()?.ok());
        for ev in events {
            match ev {
                PlatformEvent::Deposit {
                    key,
                    amount,
                    outcome: DepositOutcome::Bounced,
                } if key == *who => pushed.push(amount),
                PlatformEvent::ForcedWithdrawalProcessed { key, amount }
                    if key == *who && amount >= 1 =>
                {
                    pushed.push(amount)
                }
                _ => {}
            }
        }
        if is_mine {
            let amount = pushed.first().copied().unwrap_or(0);
            return Ok(Some((
                amount,
                before.iter().filter(|a| **a == amount).count(),
            )));
        }
        before.extend(pushed);
    }
    Ok(None)
}

/// `LastCkpt`'s seq and header hash, from the settlement's instance storage.
fn last_of(storage: &ScMap) -> Result<(u64, [u8; 32])> {
    let last = scval::map(
        scval::entry(storage, &variant("LastCkpt", vec![]))
            .ok_or_else(|| anyhow!("no LastCkpt"))?,
        "LastCkpt",
    )?;
    Ok((
        scval::u64_of(scval::field(last, "seq")?, "seq")?,
        scval::bytes32(scval::field(last, "header_hash")?, "header_hash")?,
    ))
}

/// `/v1/status` `inbox.processed`, if the sequencer answers.
async fn processed(api: &str) -> Option<u64> {
    get(api, "/v1/status").await.ok()?["inbox"]["processed"]
        .as_str()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exit_file_counts_only_for_stellars_last_checkpoint() {
        let hash = [0xab; 32];
        let hex = caravel_runtime::sequencer::hex(&hash);
        let e = serde_json::json!({ "seq": "7", "header_hash": hex });
        assert_eq!(exit_matches(&e, (7, hash)), Ok(()));
        assert_eq!(
            exit_matches(
                &serde_json::json!({ "seq": 7, "header_hash": hex }),
                (7, hash)
            ),
            Ok(())
        );
        // A checkpoint landed after the export: the freeze fixed seq 8.
        let late = exit_matches(&e, (8, [0xcd; 32])).unwrap_err();
        assert!(
            late.contains("checkpoint 7") && late.contains("last is 8"),
            "{late}"
        );
        // Same seq, another header (a redeployed lane's file).
        assert!(exit_matches(&e, (7, [0xcd; 32])).is_err());
        assert!(exit_matches(&serde_json::json!({}), (7, hash)).is_err());
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("exit.json");
        std::fs::write(&p, "not json").unwrap();
        assert!(read_exit(&p).unwrap_err().contains("doesn't read as JSON"));
    }

    #[test]
    fn units() {
        assert_eq!(parse_units("12.5", 7).unwrap(), 125_000_000);
        assert_eq!(format_units(125_000_000, 7), "12.5");
        assert_eq!(format_units(10_000_000, 7), "1");
        assert_eq!(format_units(1, 7), "0.0000001");
        assert_eq!(format_units(-5, 1), "-0.5");
        for v in [0i128, 1, 99, 10_000_000, 123_456_789_012] {
            assert_eq!(parse_units(&format_units(v, 7), 7).unwrap(), v);
        }
    }

    #[test]
    fn contract_errors_by_who_raised_them() {
        let settlement = [7u8; 32];
        let token = [9u8; 32];
        let (s, t) = (strkey(&settlement), strkey(&token));
        let raised = |by: &str, n: u32| {
            format!("stellar contract failed: ...\n0: [Diagnostic Event] contract:{by}, topics:[error, Error(Contract, #{n})], data:\"...\"")
        };
        assert_eq!(
            explain(&raised(&s, 11), &settlement, &token),
            Some("below the lane's minimum deposit")
        );
        assert_eq!(
            explain(&raised(&t, 13), &settlement, &token),
            Some("no trustline to the token: `caravel account fund <identity>`")
        );
        assert_eq!(
            explain(&raised(&t, 10), &settlement, &token),
            Some("not enough of the token")
        );
        // A third contract's codes mean nothing here.
        assert_eq!(
            explain(&raised(&strkey(&[1; 32]), 11), &settlement, &token),
            None
        );
        // A token's error inside a settlement call: the host's log is newest
        // first, and the settlement's frame repeats the token's code twice
        // before the token's own event (soroban-env-host 28.0.2).
        let nested = format!(
            "stellar contract failed: transaction simulation failed: HostError: Error(Contract, #13)\n\nEvent log (newest first):\n   0: [Diagnostic Event] contract:{s}, topics:[error, Error(Contract, #13)], data:\"escalating error to VM trap from failed host function call: call\"\n   1: [Diagnostic Event] contract:{s}, topics:[error, Error(Contract, #13)], data:[\"contract call failed\", transfer, [...]]\n   2: [Diagnostic Event] contract:{t}, topics:[error, Error(Contract, #13)], data:\"trustline entry is missing for account\""
        );
        assert_eq!(
            explain(&nested, &settlement, &token),
            Some("no trustline to the token: `caravel account fund <identity>`")
        );
        // The same lines in another order still pick the oldest.
        let reordered: Vec<&str> = nested.lines().rev().collect();
        assert_eq!(
            explain(&reordered.join("\n"), &settlement, &token),
            Some("no trustline to the token: `caravel account fund <identity>`")
        );
        // Without a diagnostic line, the settlement's meaning.
        assert_eq!(
            explain("Error(Contract, #53)", &settlement, &token),
            Some("already claimed")
        );
        assert_eq!(explain("no error here", &settlement, &token), None);
    }

    #[test]
    fn a_withdrawals_leaf_is_its_rank_among_the_accounts_pushes() {
        use caravel_core::inbox::{InboxKind, InboxMsgV1};
        use caravel_core::receipts::{DepositOutcome, PlatformEvent, ReceiptV1, ReceiptsV1};
        let (alice, bob) = ([1u8; 32], [2u8; 32]);
        let hex = caravel_runtime::sequencer::hex;
        let withdraw = |who: [u8; 32], nonce: u64, amount: i128| {
            let body = StandardBody::Withdraw { amount };
            hex(&TxEnvelopeV1 {
                lane_id: [0; 32],
                account: who,
                signer: who,
                nonce,
                expiry_ms: 0,
                kind: body.kind(),
                sig_scheme: SigScheme::Sep53,
                body: body.encode(),
                signature: [0; 64],
            }
            .encode()
            .unwrap())
        };
        let inbox = |kind, index, who: [u8; 32], amount| {
            hex(&InboxMsgV1 {
                kind,
                index,
                lane_account: who,
                amount,
                enqueued_at: 0,
            }
            .encode())
        };
        let mine = withdraw(alice, 3, 20);
        // alice's 20 (pushed), a refused 20 (code 10, not pushed), bob's 20,
        // alice's bounced deposit of 20, a forced withdrawal queuing 5 for
        // her, then the one looked for.
        let entries = [
            ("user", withdraw(alice, 1, 20), 0),
            ("user", withdraw(alice, 2, 20), 10),
            ("user", withdraw(bob, 1, 20), 0),
            ("inbox", inbox(InboxKind::Deposit, 7, alice, 20), 0),
            ("inbox", inbox(InboxKind::ForcedWithdrawal, 8, alice, 9), 0),
            ("user", mine.clone(), 0),
        ];
        let events = |i: u32| match i {
            3 => vec![PlatformEvent::Deposit {
                key: alice,
                amount: 20,
                outcome: DepositOutcome::Bounced,
            }
            .to_event()],
            4 => vec![PlatformEvent::ForcedWithdrawalProcessed {
                key: alice,
                amount: 5,
            }
            .to_event()],
            _ => vec![],
        };
        let receipts = ReceiptsV1 {
            receipts: (0..entries.len() as u32)
                .map(|i| ReceiptV1 {
                    entry_index: i,
                    code: entries[i as usize].2,
                    events: events(i),
                })
                .collect(),
        };
        let block = json!({
            "receipts_hex": hex(&receipts.encode().unwrap()),
            "entries": entries.iter().enumerate().map(|(i, (t, h, c))| json!({ "index": i, "type": t, "hex": h, "code": c })).collect::<Vec<_>>(),
        });
        let mut before = Vec::new();
        assert_eq!(
            scan_block(&block, &alice, Mine::Tx(&mine), &mut before).unwrap(),
            Some((20, 2))
        );
        // The forced withdrawal: it queued 5, the first 5 of hers.
        let mut before = Vec::new();
        assert_eq!(
            scan_block(&block, &alice, Mine::Inbox(8), &mut before).unwrap(),
            Some((5, 0))
        );
        // Not in this block: its pushes carry over to the next.
        let mut before = Vec::new();
        assert_eq!(
            scan_block(&block, &alice, Mine::Inbox(99), &mut before).unwrap(),
            None
        );
        assert_eq!(before, [20, 20, 5, 20]);
    }

    #[test]
    fn receipt_names() {
        assert_eq!(receipt_name(0), "OK (0)");
        assert!(receipt_name(41).starts_with("INSUFFICIENT_FREE_BALANCE"));
        assert_eq!(receipt_name(12), "the app's code 12");
    }

    #[test]
    fn leaves_parse() {
        let v = json!({ "seq": "3", "index": 1, "account": "GCQJVJPUPJTVTABP7FK7RXBNFIKKLSM5EO7JP6DECJ77SOBUKWSPB64N", "amount": "50000000", "proof": ["ab", "cd"] });
        let l = Leaf::parse(&v).unwrap();
        assert_eq!(
            (l.seq, l.index, l.amount, l.proof.len()),
            (3, 1, 50_000_000, 2)
        );
        assert!(Leaf::parse(&json!({ "seq": 3 })).is_err());
    }

    #[test]
    fn circle_usdc_is_its_issuers_asset() {
        let issuer =
            stellar_strkey::ed25519::PublicKey::from_string(crate::versions::testnet_usdc_issuer())
                .unwrap()
                .0;
        let sac = crate::address::asset_contract_id(Network::Testnet.passphrase(), "USDC", &issuer);
        assert_eq!(strkey(&sac), crate::versions::testnet_usdc());
    }
}
