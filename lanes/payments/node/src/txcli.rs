//! `caravel-payments-node tx`: signs a payments transaction with an account's
//! `S...` key file (raw ed25519 over the tx hash) and submits it to a
//! sequencer. Used by `scripts/e2e-local.sh` and by operators.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use caravel_core::tx::{SigScheme, StandardBody, TxEnvelopeV1};
use caravel_node::lane_toml::{parse_account, LaneFile};
use caravel_node::plugin::parse_amount;
use caravel_payments::{Transfer, TRANSFER};
use caravel_runtime::checkpoint::sha256;
use caravel_runtime::sequencer::hex;
use caravel_runtime::views;
use clap::{Args, Subcommand};
use ed25519_dalek::Signer;
use serde_json::{json, Value};

use crate::PaymentsApp;

#[derive(Args, Debug)]
pub struct TxArgs {
    /// The lane file (for lane_id and config_hash).
    #[arg(long)]
    pub lane: PathBuf,
    /// The account's S... secret key file; the account is its G... key.
    #[arg(long)]
    pub key_file: PathBuf,
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    pub sequencer: String,
    /// Defaults to the account's next nonce from the sequencer.
    #[arg(long)]
    pub nonce: Option<u64>,
    #[arg(long, default_value_t = 60)]
    pub expiry_secs: u64,
    /// Print the signed transaction instead of submitting it.
    #[arg(long)]
    pub dry_run: bool,
    #[command(subcommand)]
    pub body: Body,
}

/// Amounts are the token's base units (stroops for USDC); under `caravel tx`
/// (`plugin body --decimals`) they are token units.
#[derive(Subcommand, Debug)]
pub enum Body {
    /// Pay another lane account (it must have deposited once).
    Transfer {
        /// The recipient's G... account.
        #[arg(long)]
        to: String,
        #[arg(long)]
        amount: String,
        #[arg(long, default_value_t = 0)]
        memo: u64,
    },
    /// Move an amount to the pending withdrawal queue.
    Withdraw {
        #[arg(long)]
        amount: String,
    },
}

/// A body's kind and bytes; token amounts are read with `decimals`
/// ([`caravel_node::plugin::parse_amount`]).
pub fn body(b: &Body, decimals: Option<u32>) -> Result<(u8, Vec<u8>)> {
    Ok(match b {
        Body::Transfer { to, amount, memo } => (
            TRANSFER,
            Transfer {
                to: parse_account(to)?,
                amount: parse_amount(amount, decimals)?,
                memo: *memo,
            }
            .encode(),
        ),
        Body::Withdraw { amount } => {
            let b = StandardBody::Withdraw {
                amount: parse_amount(amount, decimals)?,
            };
            (b.kind(), b.encode())
        }
    })
}

/// `plugin body`: a body from its arguments, as `tx` takes them.
pub fn plugin_body(args: &[String], decimals: Option<u32>) -> Result<(u8, Vec<u8>)> {
    #[derive(clap::Parser)]
    #[command(name = "body", no_binary_name = true)]
    struct Parsed {
        #[command(subcommand)]
        body: Body,
    }
    let p = <Parsed as clap::Parser>::try_parse_from(args).map_err(|e| anyhow!("{e}"))?;
    body(&p.body, decimals)
}

pub async fn run(a: TxArgs) -> Result<()> {
    let lane = LaneFile::load(&a.lane)?;
    let (_, config_bytes, _) = caravel_node::lane_toml::genesis(&PaymentsApp, &lane)?;
    let key = caravel_node::validator::read_key_file(&a.key_file)?;
    let account = key.verifying_key().to_bytes();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let base = a.sequencer.trim_end_matches('/');
    let nonce = match a.nonce {
        Some(n) => n,
        None => {
            let r = http
                .get(format!("{base}/v1/accounts/{}", views::g_address(&account)))
                .send()
                .await?;
            if !r.status().is_success() {
                bail!(
                    "no lane account for {} (status {}); deposit first",
                    views::g_address(&account),
                    r.status()
                );
            }
            let v: Value = r.json().await?;
            v["next_nonce"]
                .as_str()
                .ok_or_else(|| anyhow!("no next_nonce"))?
                .parse()?
        }
    };
    let (kind, body) = body(&a.body, None)?;
    let now_ms = caravel_node::sequencer::now_ms();
    let mut tx = TxEnvelopeV1 {
        lane_id: lane.lane_id(),
        account,
        signer: account,
        nonce,
        expiry_ms: now_ms + a.expiry_secs * 1000,
        kind,
        sig_scheme: SigScheme::RawEd25519,
        body,
        signature: [0; 64],
    };
    let tx_hash = sha256(&tx.tx_hash_preimage(&sha256(&config_bytes)));
    tx.signature = key.sign(&tx_hash).to_bytes();
    let bytes = tx.encode().ok_or_else(|| anyhow!("tx does not encode"))?;
    if a.dry_run {
        println!(
            "{}",
            json!({ "tx": hex(&bytes), "tx_hash": hex(&tx_hash), "nonce": nonce.to_string() })
        );
        return Ok(());
    }
    let r = http
        .post(format!("{base}/v1/tx"))
        .json(&json!({ "tx": hex(&bytes) }))
        .send()
        .await?;
    let status = r.status();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    println!(
        "{}",
        json!({ "status": status.as_u16(), "response": v, "nonce": nonce.to_string() })
    );
    if !status.is_success() {
        bail!("the sequencer refused the transaction");
    }
    Ok(())
}
