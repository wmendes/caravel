//! A lane's users on Stellar (M0.6, C-12, DEC-084): accounts with the
//! settlement token, deposits that return once the lane has credited them,
//! and balances on both sides. Keys stay in the Stellar CLI's keystore.

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

use crate::deploy::{addresses, api_url, Addresses, Keys};
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

/// What a settlement contract error means (`Error(Contract, #N)`, spec §13).
pub fn explain(err: &str) -> Option<&'static str> {
    let n: u32 = err
        .split("Error(Contract, #")
        .nth(1)?
        .split(')')
        .next()?
        .parse()
        .ok()?;
    Some(match n {
        10 => "the lane is frozen: use escape",
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
        _ => return None,
    })
}

fn explained(e: anyhow::Error) -> anyhow::Error {
    let text = format!("{e:#}");
    match explain(&text) {
        Some(why) => e.context(why),
        None => e,
    }
}

/// A deployment, seen by its users.
pub struct Flows {
    pub m: Manifest,
    pub keys: Keys,
    pub addrs: Addresses,
    pub cli: Cli,
}

impl Flows {
    pub fn new(m: Manifest) -> Result<Self> {
        let keys = Keys::from_keystore(&m)?;
        let addrs = addresses(&m, &keys)?;
        let cli = Cli::new(&m);
        Ok(Self {
            m,
            keys,
            addrs,
            cli,
        })
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
            anyhow!("the host has no public_url: the lane's API isn't reachable from here")
        })
    }

    /// The Stellar asset behind the settlement token, and whether the
    /// lane file's admin issues it (so it can mint).
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
                .map(|(code, issuer)| (code, issuer, issuer == self.keys.admin)),
        }
    }

    async fn account_exists(&self, who: &Key) -> Result<bool> {
        let rpc = caravel_node::stellar_rpc::Rpc::new(self.m.rpc_url())?;
        Ok(rpc
            .ledger_entry(&caravel_node::stellar_rpc::account_key(who))
            .await?
            .is_some())
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
                    .map_err(explained)?;
                did.push(format!(
                    "trusts {code}:{}",
                    caravel_runtime::views::g_address(&issuer)
                ));
            }
            _ => {}
        }
        if let Some(amount) = amount {
            match (self.asset(), self.m.env.network) {
                (Some((_, _, true)), _) => {
                    self.cli
                        .mint(&self.m.env.admin, &self.addrs.token, &who, amount)
                        .map_err(explained)?;
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
                    crate::address::strkey(&self.addrs.token)
                ),
            }
        }
        Ok(json!({
            "identity": identity,
            "account": caravel_runtime::views::g_address(&who),
            "did": did,
            "token": self.token_balance(&who).ok().map(|b| format_units(b, decimals)),
        }))
    }

    pub fn token_balance(&self, who: &Key) -> Result<i128> {
        self.cli
            .token_balance(&self.m.env.admin, &self.addrs.token, who)
    }

    /// An account's balance on Stellar and its lane account.
    pub async fn balance(&self, who: &Key) -> Result<Value> {
        let decimals = self.decimals()?;
        let stellar = self.token_balance(who).ok();
        let g = caravel_runtime::views::g_address(who);
        let lane = match self.api() {
            Ok(api) => crate::lifecycle::get(&api, &format!("/v1/accounts/{g}"))
                .await
                .ok(),
            Err(_) => None,
        };
        Ok(json!({
            "account": g,
            "stellar": stellar.map(|b| format_units(b, decimals)),
            "lane": lane,
        }))
    }

    /// Deposits `amount` into the lane; with `wait`, returns once the lane
    /// has credited it (the sequencer processed its inbox message).
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
        if let Ok(have) = self.token_balance(&who) {
            if have < amount {
                bail!(
                    "{identity} holds {} of the token, less than {}: `caravel account fund {identity} --amount …`",
                    format_units(have, decimals),
                    format_units(amount, decimals)
                );
            }
        }
        let index = self
            .cli
            .deposit(identity, &self.addrs.settlement, &who, amount)
            .map_err(explained)?;
        let mut out = json!({
            "account": caravel_runtime::views::g_address(&who),
            "amount": format_units(amount, decimals),
            "inbox_index": index,
            "credited": false,
        });
        let Some(timeout) = wait else {
            return Ok(out);
        };
        let api = self.api()?;
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(s) = crate::lifecycle::get(&api, "/v1/status").await {
                let processed: u64 = s["inbox"]["processed"]
                    .as_str()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(0);
                if processed > index {
                    out["credited"] = json!(true);
                    out["lane"] = crate::lifecycle::get(
                        &api,
                        &format!(
                            "/v1/accounts/{}",
                            out["account"].as_str().unwrap_or_default()
                        ),
                    )
                    .await
                    .unwrap_or(Value::Null);
                    return Ok(out);
                }
            }
            if Instant::now() > deadline {
                bail!("deposit {index} is on Stellar, but the lane hasn't credited it after {} s: is the relayer running?", timeout.as_secs());
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn contract_errors() {
        assert_eq!(
            explain("stellar contract failed: HostError: Error(Contract, #11)"),
            Some("below the lane's minimum deposit")
        );
        assert_eq!(explain("Error(Contract, #53)"), Some("already claimed"));
        assert_eq!(explain("Error(Contract, #99)"), None);
        assert_eq!(explain("no error here"), None);
    }

    #[test]
    fn circle_usdc_is_its_issuers_asset() {
        let issuer =
            stellar_strkey::ed25519::PublicKey::from_string(crate::versions::testnet_usdc_issuer())
                .unwrap()
                .0;
        let sac = crate::address::asset_contract_id(Network::Testnet.passphrase(), "USDC", &issuer);
        assert_eq!(
            crate::address::strkey(&sac),
            crate::versions::testnet_usdc()
        );
    }
}
