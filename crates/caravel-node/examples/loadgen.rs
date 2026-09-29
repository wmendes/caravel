//! Synthetic load for a LOCAL sequencer (T-007 soak test): deposits through
//! the internal inbox API, signed oracle updates, and a steady stream of
//! orders, cancels and small withdrawals at `--tps`.
//!
//! Keys are deterministic test keys (seed `[n; 32]`), and the oracle key is
//! the local lane's fixture key. Local lanes only; never testnet.
//!
//! cargo run --release -p caravel-node --example loadgen -- \
//!   --url http://127.0.0.1:8080 --lane config/lane.caravel-perps.local.toml --tps 50 --duration-secs 3600

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use caravel_lane::checkpoint::sha256;
use caravel_lane::sequencer::{hex, unhex};
use caravel_node::lane_toml::LaneFile;
use caravel_types::inbox::{inbox_acc_preimage, InboxKind, InboxMsgV1};
use caravel_types::oracle::OracleUpdateV1;
use caravel_types::tx::{LaneTxV1, PlaceOrder, Side, SigScheme, Tif, TxBody};
use caravel_types::vectors::{key, pk};
use clap::Parser;
use ed25519_dalek::Signer;
use serde_json::{json, Value};

const ORACLE_SEED: u8 = 0x31;
const USDC: i128 = 10_000_000;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    url: String,
    #[arg(long)]
    lane: PathBuf,
    #[arg(long, default_value = "CARAVEL_INTERNAL_TOKEN")]
    token_env: String,
    #[arg(long, default_value_t = 50)]
    tps: u64,
    #[arg(long, default_value_t = 60)]
    duration_secs: u64,
    #[arg(long, default_value_t = 24)]
    accounts: u8,
    /// Seed of the first account key.
    #[arg(long, default_value_t = 0x80)]
    seed_base: u8,
    #[arg(long, default_value_t = 60)]
    report_secs: u64,
}

/// xorshift64*: deterministic, no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

struct Market {
    id: u16,
    tick: i64,
    price: i64,
}

struct Gen {
    http: reqwest::Client,
    url: String,
    token: String,
    lane_id: [u8; 32],
    config_hash: [u8; 32],
    seeds: Vec<u8>,
    nonces: BTreeMap<u8, u64>,
    markets: Vec<Market>,
    rng: Rng,
    sent: u64,
    queued: u64,
    rejected: BTreeMap<String, u64>,
    errors: u64,
}

fn now_ms() -> u64 {
    caravel_node::sequencer::now_ms()
}

impl Gen {
    async fn get(&self, path: &str) -> Result<(u16, Value)> {
        let r = self.http.get(format!("{}{path}", self.url)).send().await?;
        let s = r.status().as_u16();
        Ok((s, r.json().await.unwrap_or(Value::Null)))
    }

    async fn internal(&self, path: &str, body: Value) -> Result<(u16, Value)> {
        let r = self
            .http
            .post(format!("{}{path}", self.url))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await?;
        let s = r.status().as_u16();
        Ok((s, r.json().await.unwrap_or(Value::Null)))
    }

    fn g(seed: u8) -> String {
        caravel_lane::views::g_address(&pk(seed))
    }

    async fn fund(&mut self) -> Result<()> {
        let (_, status) = self.get("/v1/status").await?;
        let mut n: u64 = status["inbox"]["reported"]
            .as_str()
            .context("status.inbox.reported")?
            .parse()?;
        let mut acc: [u8; 32] = unhex(
            status["inbox"]["reported_acc"]
                .as_str()
                .context("status.inbox.reported_acc")?,
        )
        .context("acc hex")?
        .try_into()
        .map_err(|_| anyhow::anyhow!("acc length"))?;
        for seed in self.seeds.clone() {
            if self
                .get(&format!("/v1/accounts/{}", Self::g(seed)))
                .await?
                .0
                == 200
            {
                continue;
            }
            let msg = InboxMsgV1 {
                kind: InboxKind::Deposit,
                index: n,
                lane_account: pk(seed),
                amount: 1_000_000 * USDC,
                enqueued_at: now_ms() / 1000,
            };
            acc = sha256(&inbox_acc_preimage(&acc, &msg.encode()));
            let (s, v) = self.internal("/internal/inbox", json!({ "index": n.to_string(), "msg_hex": hex(&msg.encode()), "acc_after_hex": hex(&acc) })).await?;
            if s != 200 {
                bail!("deposit {n} refused: {s} {v}");
            }
            n += 1;
        }
        let deadline = Instant::now() + Duration::from_secs(60);
        for seed in self.seeds.clone() {
            loop {
                let (s, a) = self.get(&format!("/v1/accounts/{}", Self::g(seed))).await?;
                if s == 200 {
                    self.nonces
                        .insert(seed, a["next_nonce"].as_str().unwrap_or("0").parse()?);
                    break;
                }
                if Instant::now() > deadline {
                    bail!("account {} never appeared", Self::g(seed));
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        Ok(())
    }

    async fn publish_prices(&mut self) -> Result<()> {
        for i in 0..self.markets.len() {
            // A small random walk, snapped to the tick.
            let m = &mut self.markets[i];
            let step = (m.price / 5_000 / m.tick).max(1) * m.tick;
            let dir = (self.rng.below(3) as i64) - 1;
            m.price = (m.price + dir * step).max(m.tick);
            let mut u = OracleUpdateV1 {
                market_id: m.id,
                price: m.price,
                publish_time_ms: now_ms(),
                oracle_key: pk(ORACLE_SEED),
                signature: [0; 64],
            };
            u.signature = key(ORACLE_SEED)
                .sign(&sha256(&u.signing_preimage(&self.lane_id)))
                .to_bytes();
            let (s, v) = self
                .internal("/internal/oracle", json!({ "update": hex(&u.encode()) }))
                .await?;
            if s != 200 {
                bail!("oracle refused: {s} {v}");
            }
        }
        Ok(())
    }

    fn body(&mut self) -> TxBody {
        let m = &self.markets[self.rng.below(self.markets.len() as u64) as usize];
        let (id, tick, mid) = (m.id, m.tick, m.price);
        let roll = self.rng.below(100);
        let side = if self.rng.below(2) == 0 {
            Side::Buy
        } else {
            Side::Sell
        };
        let lots = 1 + self.rng.below(5) as i64;
        match roll {
            0..=59 => {
                // Resting orders 1..20 ticks away from the mid.
                let off = (1 + self.rng.below(20) as i64) * tick;
                let price = if side == Side::Buy {
                    mid - off
                } else {
                    mid + off
                };
                TxBody::PlaceOrder(PlaceOrder {
                    market_id: id,
                    side,
                    tif: Tif::Gtc,
                    reduce_only: false,
                    price,
                    lots,
                    client_order_id: self.rng.next(),
                })
            }
            60..=84 => {
                // Takers that cross a few ticks.
                let off = (1 + self.rng.below(5) as i64) * tick;
                let price = if side == Side::Buy {
                    mid + off
                } else {
                    mid - off
                };
                TxBody::PlaceOrder(PlaceOrder {
                    market_id: id,
                    side,
                    tif: Tif::Ioc,
                    reduce_only: false,
                    price,
                    lots,
                    client_order_id: self.rng.next(),
                })
            }
            85..=96 => TxBody::CancelAll { market_id: id },
            _ => TxBody::Withdraw { amount: USDC },
        }
    }

    async fn send_one(&mut self) -> Result<()> {
        let seed = self.seeds[self.rng.below(self.seeds.len() as u64) as usize];
        let nonce = self.nonces[&seed];
        let body = self.body();
        let mut tx = LaneTxV1 {
            lane_id: self.lane_id,
            account: pk(seed),
            signer: pk(seed),
            nonce,
            expiry_ms: now_ms() + 30_000,
            sig_scheme: SigScheme::RawEd25519,
            body,
            signature: [0; 64],
        };
        tx.signature = key(seed)
            .sign(&sha256(&tx.tx_hash_preimage(&self.config_hash)))
            .to_bytes();
        self.sent += 1;
        let r = self
            .http
            .post(format!("{}/v1/tx", self.url))
            .header("content-type", "application/octet-stream")
            .body(tx.encode())
            .send()
            .await;
        match r {
            Ok(r) if r.status().as_u16() == 202 => {
                self.queued += 1;
                self.nonces.insert(seed, nonce + 1);
            }
            Ok(r) => {
                let v: Value = r.json().await.unwrap_or(Value::Null);
                let code = v["code"].as_str().unwrap_or("?").to_string();
                if code == "BAD_NONCE" || code == "ACCOUNT_QUEUE_FULL" {
                    let (_, a) = self.get(&format!("/v1/accounts/{}", Self::g(seed))).await?;
                    self.nonces
                        .insert(seed, a["next_nonce"].as_str().unwrap_or("0").parse()?);
                }
                *self.rejected.entry(code).or_default() += 1;
            }
            Err(_) => self.errors += 1,
        }
        Ok(())
    }

    async fn report(&self, started: Instant) -> Value {
        let status = self
            .get("/v1/status")
            .await
            .map(|(_, v)| v)
            .unwrap_or(Value::Null);
        json!({
            "elapsed_s": started.elapsed().as_secs(),
            "sent": self.sent,
            "queued": self.queued,
            "rejected": self.rejected,
            "http_errors": self.errors,
            "height": status["height"],
            "mempool": status["mempool"],
            "checkpoints": status["checkpoints"],
            "host_metering": status["host_metering"],
            "state_hash": status["state_hash"],
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let lane = LaneFile::load(&args.lane)?;
    let (_, config_bytes, _) = caravel_node::lane_toml::genesis(&lane)?;
    let token =
        std::env::var(&args.token_env).with_context(|| format!("set {}", args.token_env))?;
    let markets = lane
        .markets
        .iter()
        .map(|m| Market {
            id: m.market_id,
            tick: m.tick,
            price: 0,
        })
        .collect::<Vec<_>>();
    let mut g = Gen {
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?,
        url: args.url.trim_end_matches('/').to_string(),
        token,
        lane_id: lane.lane_id(),
        config_hash: sha256(&config_bytes),
        seeds: (0..args.accounts).map(|i| args.seed_base + i).collect(),
        nonces: BTreeMap::new(),
        markets,
        rng: Rng(0x9E37_79B9_7F4A_7C15),
        sent: 0,
        queued: 0,
        rejected: BTreeMap::new(),
        errors: 0,
    };
    let (_, status) = g.get("/v1/status").await?;
    if status["lane_id"].as_str() != Some(hex(&g.lane_id).as_str()) {
        bail!("the sequencer runs another lane: {}", status["lane_id"]);
    }
    // Start prices near the testkit levels (BTC $65,000, ETH $3,500, XLM $0.40 per lot unit).
    for m in &mut g.markets {
        m.price = match m.id {
            1 => 65_000_000,
            2 => 35_000_000,
            _ => 40_000_000,
        };
    }
    g.publish_prices().await?;
    g.fund().await?;
    let started = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_micros(1_000_000 / args.tps.max(1)));
    let mut last_price = Instant::now();
    let mut last_report = Instant::now();
    while started.elapsed() < Duration::from_secs(args.duration_secs) {
        tick.tick().await;
        if last_price.elapsed() >= Duration::from_secs(2) {
            if let Err(e) = g.publish_prices().await {
                eprintln!("oracle: {e}");
                g.errors += 1;
            }
            last_price = Instant::now();
        }
        if let Err(e) = g.send_one().await {
            eprintln!("tx: {e}");
            g.errors += 1;
        }
        if last_report.elapsed() >= Duration::from_secs(args.report_secs) {
            println!("{}", g.report(started).await);
            last_report = Instant::now();
        }
    }
    let mut summary = g.report(started).await;
    summary["tx_per_s"] = json!(g.queued as f64 / started.elapsed().as_secs_f64());
    println!("{summary}");
    Ok(())
}
