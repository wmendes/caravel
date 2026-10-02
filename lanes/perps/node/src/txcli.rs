//! `caravel-perps-node tx`: signs a lane transaction with an account's `S...` key
//! file (raw ed25519 over the tx hash) and submits it to a sequencer. Used by
//! `scripts/e2e-local.sh` and by operators (spec §14.6).

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use caravel_runtime::checkpoint::sha256;
use caravel_runtime::sequencer::hex;
use caravel_runtime::views;
use caravel_types::tx::{LaneTxV1, PlaceOrder, Side, SigScheme, Tif, TxBody, ALL_MARKETS};
use clap::{Args, Subcommand, ValueEnum};
use ed25519_dalek::Signer;
use serde_json::{json, Value};

use caravel_node::lane_toml::LaneFile;
use caravel_node::plugin::parse_amount;

use crate::PerpsApp;

#[derive(Args, Debug)]
pub struct TxArgs {
    /// The lane TOML (for lane_id and config_hash).
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

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum SideArg {
    Buy,
    Sell,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum TifArg {
    Gtc,
    Ioc,
    PostOnly,
}

/// Token amounts (`price`, `amount`) are the token's base units (stroops for
/// USDC); under `caravel tx` (`plugin body --decimals`) they are token units.
#[derive(Subcommand, Debug)]
pub enum Body {
    PlaceOrder {
        #[arg(long)]
        market: u16,
        #[arg(long, value_enum)]
        side: SideArg,
        #[arg(long, value_enum, default_value = "gtc")]
        tif: TifArg,
        /// Per lot.
        #[arg(long)]
        price: String,
        #[arg(long)]
        lots: i64,
        #[arg(long)]
        reduce_only: bool,
        #[arg(long, default_value_t = 0)]
        client_order_id: u64,
    },
    CancelOrder {
        #[arg(long)]
        market: u16,
        #[arg(long)]
        order_id: u64,
    },
    /// `--market all` cancels on every market.
    CancelAll {
        #[arg(long)]
        market: String,
    },
    /// Move an amount to the pending withdrawal queue.
    Withdraw {
        #[arg(long)]
        amount: String,
    },
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
    // The perps body codec is the frozen engine's; its envelope is the
    // platform's `TxEnvelopeV1` byte for byte (tests/format_compat.rs).
    let tx = LaneTxV1 {
        lane_id: [0; 32],
        account: [0; 32],
        signer: [0; 32],
        nonce: 0,
        expiry_ms: 0,
        sig_scheme: SigScheme::RawEd25519,
        body: body(&p.body, decimals)?,
        signature: [0; 64],
    };
    let env = caravel_core::tx::TxEnvelopeV1::decode(&tx.encode())
        .map_err(|e| anyhow!("perps body: {e:?}"))?;
    Ok((env.kind, env.body))
}

fn body(b: &Body, decimals: Option<u32>) -> Result<TxBody> {
    Ok(match b {
        Body::PlaceOrder {
            market,
            side,
            tif,
            price,
            lots,
            reduce_only,
            client_order_id,
        } => TxBody::PlaceOrder(PlaceOrder {
            market_id: *market,
            side: match side {
                SideArg::Buy => Side::Buy,
                SideArg::Sell => Side::Sell,
            },
            tif: match tif {
                TifArg::Gtc => Tif::Gtc,
                TifArg::Ioc => Tif::Ioc,
                TifArg::PostOnly => Tif::PostOnly,
            },
            reduce_only: *reduce_only,
            price: i64::try_from(parse_amount(price, decimals)?).context("--price")?,
            lots: *lots,
            client_order_id: *client_order_id,
        }),
        Body::CancelOrder { market, order_id } => TxBody::CancelOrder {
            market_id: *market,
            order_id: *order_id,
        },
        Body::CancelAll { market } => TxBody::CancelAll {
            market_id: if market == "all" {
                ALL_MARKETS
            } else {
                market.parse().context("--market")?
            },
        },
        Body::Withdraw { amount } => TxBody::Withdraw {
            amount: parse_amount(amount, decimals)?,
        },
    })
}

pub async fn run(a: TxArgs) -> Result<()> {
    let lane = LaneFile::load(&a.lane)?;
    let (_, config_bytes, _) = caravel_node::lane_toml::genesis(&PerpsApp, &lane)?;
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
    let now_ms = caravel_node::sequencer::now_ms();
    let mut tx = LaneTxV1 {
        lane_id: lane.lane_id(),
        account,
        signer: account,
        nonce,
        expiry_ms: now_ms + a.expiry_secs * 1000,
        sig_scheme: SigScheme::RawEd25519,
        body: body(&a.body, None)?,
        signature: [0; 64],
    };
    let tx_hash = sha256(&tx.tx_hash_preimage(&sha256(&config_bytes)));
    tx.signature = key.sign(&tx_hash).to_bytes();
    if a.dry_run {
        println!(
            "{}",
            json!({ "tx": hex(&tx.encode()), "tx_hash": hex(&tx_hash), "nonce": nonce.to_string() })
        );
        return Ok(());
    }
    let r = http
        .post(format!("{base}/v1/tx"))
        .json(&json!({ "tx": hex(&tx.encode()) }))
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
