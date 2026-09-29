//! Synthetic load for a lane (T-007 soak, T-014 measurements): a steady
//! stream of orders, cancels and small withdrawals at `--tps`.
//!
//! Accounts are deterministic test keys (seed `[n; 32]`, funded through the
//! internal inbox API, local lanes only) or, with `--key-dir`, the `S...` key
//! files in a directory for accounts already funded through Stellar. Each
//! account sends from its own task, one request at a time, so nonces stay in
//! order and a remote API's round trip does not cap the rate. With
//! `--measure` it subscribes to each account's receipts on the WebSocket and
//! reports soft latency (POST → receipt), hard latency (receipt → the
//! checkpoint holding that block accepted on Stellar) and host CPU per block.
//!
//! cargo run --release -p caravel-node --example loadgen -- \
//!   --url http://127.0.0.1:8080 --lane config/lane.caravel-perps.local.toml --tps 50 --duration-secs 3600

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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
use ed25519_dalek::{Signer, SigningKey};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;

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
    /// Use the S... key files in this directory as the accounts instead of seeds.
    #[arg(long)]
    key_dir: Option<PathBuf>,
    /// Do not fund accounts through the internal inbox API (they must exist).
    #[arg(long)]
    no_fund: bool,
    /// Do not publish oracle prices (a relayer does).
    #[arg(long)]
    no_oracle: bool,
    /// Measure latency from the WebSocket and host CPU per block.
    #[arg(long)]
    measure: bool,
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

#[derive(Clone)]
struct Market {
    id: u16,
    tick: i64,
    price: i64,
}

struct Acct {
    key: SigningKey,
    pk: [u8; 32],
    g: String,
    nonce: u64,
}

#[derive(Default)]
struct Stats {
    sent: u64,
    queued: u64,
    rejected: BTreeMap<String, u64>,
    errors: u64,
    /// POST time of queued transactions waiting for their receipt, by tx hash.
    inflight: HashMap<String, Instant>,
    soft_ms: Vec<f64>,
    /// `(height, receipt time)` of receipts waiting for their checkpoint.
    waiting_hard: Vec<(u64, Instant)>,
    /// `(height, ms)`. The final summary lists them, so the report keeps only
    /// blocks of checkpoints produced under load: after the load stops, the
    /// last receipts wait for an idle checkpoint.
    hard_ms: Vec<(u64, f64)>,
    receipts_ok: u64,
    receipts_rejected: BTreeMap<u16, u64>,
    cpu_by_height: BTreeMap<u64, u64>,
}

struct Shared {
    http: reqwest::Client,
    url: String,
    token: String,
    lane_id: [u8; 32],
    config_hash: [u8; 32],
    markets: Mutex<Vec<Market>>,
    stats: Mutex<Stats>,
    stop: AtomicBool,
}

fn now_ms() -> u64 {
    caravel_node::sequencer::now_ms()
}

fn pct(v: &[f64], p: f64) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let i = ((p / 100.0) * (s.len() - 1) as f64).round() as usize;
    Some((s[i] * 10.0).round() / 10.0)
}

fn num(v: &Value) -> Option<u64> {
    v.as_str().and_then(|s| s.parse().ok())
}

impl Shared {
    fn stats(&self) -> std::sync::MutexGuard<'_, Stats> {
        self.stats.lock().expect("stats lock")
    }

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

    async fn height(&self) -> Result<u64> {
        num(&self.get("/v1/status").await?.1["height"]).context("status.height")
    }

    async fn fund(&self, accounts: &[Acct]) -> Result<()> {
        let (_, status) = self.get("/v1/status").await?;
        let mut n: u64 = num(&status["inbox"]["reported"]).context("status.inbox.reported")?;
        let mut acc: [u8; 32] = unhex(
            status["inbox"]["reported_acc"]
                .as_str()
                .context("status.inbox.reported_acc")?,
        )
        .context("acc hex")?
        .try_into()
        .map_err(|_| anyhow::anyhow!("acc length"))?;
        for a in accounts {
            if self.get(&format!("/v1/accounts/{}", a.g)).await?.0 == 200 {
                continue;
            }
            let msg = InboxMsgV1 {
                kind: InboxKind::Deposit,
                index: n,
                lane_account: a.pk,
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
        Ok(())
    }

    async fn next_nonce(&self, g: &str) -> Result<Option<u64>> {
        let (s, a) = self.get(&format!("/v1/accounts/{g}")).await?;
        Ok(if s == 200 {
            num(&a["next_nonce"])
        } else {
            None
        })
    }

    async fn load_nonces(&self, accounts: &mut [Acct]) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(60);
        for a in accounts {
            loop {
                if let Some(n) = self.next_nonce(&a.g).await? {
                    a.nonce = n;
                    break;
                }
                if Instant::now() > deadline {
                    bail!("account {} never appeared", a.g);
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        Ok(())
    }

    /// The lane's oracle prices, when a relayer publishes them.
    async fn read_prices(&self) -> Result<()> {
        let (_, v) = self.get("/v1/markets").await?;
        let mut markets = self.markets.lock().expect("markets lock");
        for m in v.as_array().into_iter().flatten() {
            let id = m["market_id"].as_u64().unwrap_or(0) as u16;
            if let Some(mk) = markets.iter_mut().find(|x| x.id == id) {
                mk.price = num(&m["oracle_price"]).unwrap_or(0) as i64;
            }
        }
        Ok(())
    }

    async fn publish_prices(&self, rng: &mut Rng) -> Result<()> {
        let updates: Vec<OracleUpdateV1> = {
            let mut markets = self.markets.lock().expect("markets lock");
            markets
                .iter_mut()
                .map(|m| {
                    // A small random walk, snapped to the tick.
                    let step = (m.price / 5_000 / m.tick).max(1) * m.tick;
                    let dir = (rng.below(3) as i64) - 1;
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
                    u
                })
                .collect()
        };
        for u in updates {
            let (s, v) = self
                .internal("/internal/oracle", json!({ "update": hex(&u.encode()) }))
                .await?;
            if s != 200 {
                bail!("oracle refused: {s} {v}");
            }
        }
        Ok(())
    }

    fn body(&self, rng: &mut Rng) -> TxBody {
        let (id, tick, mid) = {
            let markets = self.markets.lock().expect("markets lock");
            let m = &markets[rng.below(markets.len() as u64) as usize];
            (m.id, m.tick, (m.price / m.tick) * m.tick)
        };
        let roll = rng.below(100);
        let side = if rng.below(2) == 0 {
            Side::Buy
        } else {
            Side::Sell
        };
        let lots = 1 + rng.below(5) as i64;
        match roll {
            0..=59 => {
                // Resting orders 1..20 ticks away from the mid.
                let off = (1 + rng.below(20) as i64) * tick;
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
                    client_order_id: rng.next(),
                })
            }
            60..=84 => {
                // Takers that cross a few ticks.
                let off = (1 + rng.below(5) as i64) * tick;
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
                    client_order_id: rng.next(),
                })
            }
            85..=96 => TxBody::CancelAll { market_id: id },
            _ => TxBody::Withdraw { amount: USDC },
        }
    }

    async fn send_one(&self, a: &mut Acct, rng: &mut Rng) -> Result<()> {
        let body = self.body(rng);
        let mut tx = LaneTxV1 {
            lane_id: self.lane_id,
            account: a.pk,
            signer: a.pk,
            nonce: a.nonce,
            expiry_ms: now_ms() + 30_000,
            sig_scheme: SigScheme::RawEd25519,
            body,
            signature: [0; 64],
        };
        let hash = sha256(&tx.tx_hash_preimage(&self.config_hash));
        tx.signature = a.key.sign(&hash).to_bytes();
        let hash = hex(&hash);
        {
            // In the map before the POST: the receipt can beat the response.
            let mut st = self.stats();
            st.sent += 1;
            st.inflight.insert(hash.clone(), Instant::now());
        }
        let r = self
            .http
            .post(format!("{}/v1/tx", self.url))
            .header("content-type", "application/octet-stream")
            .body(tx.encode())
            .send()
            .await;
        match r {
            Ok(r) if r.status().as_u16() == 202 => {
                self.stats().queued += 1;
                a.nonce += 1;
            }
            Ok(r) => {
                let v: Value = r.json().await.unwrap_or(Value::Null);
                let code = v["code"].as_str().unwrap_or("?").to_string();
                {
                    let mut st = self.stats();
                    st.inflight.remove(&hash);
                    *st.rejected.entry(code.clone()).or_default() += 1;
                }
                if code == "BAD_NONCE" || code == "ACCOUNT_QUEUE_FULL" {
                    if let Some(n) = self.next_nonce(&a.g).await? {
                        a.nonce = n;
                    }
                }
            }
            Err(_) => {
                let mut st = self.stats();
                st.inflight.remove(&hash);
                st.errors += 1;
            }
        }
        Ok(())
    }

    fn on_receipt(&self, v: &Value) {
        let Some(tx) = v["tx_hash"].as_str() else {
            return;
        };
        let mut st = self.stats();
        let Some(at) = st.inflight.remove(tx) else {
            return;
        };
        st.soft_ms.push(at.elapsed().as_secs_f64() * 1000.0);
        let code = v["code"].as_u64().unwrap_or(0) as u16;
        if code == 0 {
            st.receipts_ok += 1;
        } else {
            *st.receipts_rejected.entry(code).or_default() += 1;
        }
        // Hard latency (spec §19.6): from the receipt (the fill, if any, is in
        // the same block message) to its checkpoint accepted on Stellar, for
        // every tenth receipt.
        if st.soft_ms.len() % 10 == 1 {
            if let Some(h) = num(&v["height"]) {
                st.waiting_hard.push((h, Instant::now()));
            }
        }
    }

    /// Samples host CPU of the last block, and resolves hard latency for
    /// receipts whose block is now in an accepted checkpoint.
    async fn probe(&self, accepted: &mut (u64, u64)) -> Result<()> {
        let (_, s) = self.get("/v1/status").await?;
        if let (Some(h), Some(cpu)) = (
            num(&s["host_metering"]["height"]),
            num(&s["host_metering"]["cpu_insns"]),
        ) {
            if h > 0 {
                self.stats().cpu_by_height.insert(h, cpu);
            }
        }
        let Some(seq) = num(&s["checkpoints"]["accepted"]) else {
            return Ok(());
        };
        if seq != accepted.0 {
            let (_, c) = self.get(&format!("/v1/checkpoints/{seq}")).await?;
            if let Some(last) = num(&c["last_block_height"]) {
                *accepted = (seq, last);
            }
        }
        let now = Instant::now();
        let mut st = self.stats();
        let (done, still): (Vec<_>, Vec<_>) = std::mem::take(&mut st.waiting_hard)
            .into_iter()
            .partition(|(h, _)| *h <= accepted.1);
        st.hard_ms.extend(
            done.iter()
                .map(|(h, at)| (*h, (now - *at).as_secs_f64() * 1000.0)),
        );
        st.waiting_hard = still;
        Ok(())
    }

    async fn report(
        &self,
        started: Instant,
        start_height: u64,
        window: Option<(f64, u64)>,
    ) -> Value {
        let status = self
            .get("/v1/status")
            .await
            .map(|(_, v)| v)
            .unwrap_or(Value::Null);
        let height = num(&status["height"]).unwrap_or(0);
        let (secs, end_height) = window.unwrap_or((started.elapsed().as_secs_f64(), height));
        let secs = secs.max(1.0);
        let st = self.stats();
        // Host CPU of the blocks produced under load only.
        let cpu: Vec<f64> = st
            .cpu_by_height
            .range(start_height + 1..=end_height)
            .map(|(_, v)| *v as f64)
            .collect();
        let hard: Vec<f64> = st.hard_ms.iter().map(|(_, ms)| *ms).collect();
        let hard_samples: Vec<(u64, u64)> = match window {
            Some(_) => st.hard_ms.iter().map(|(h, ms)| (*h, *ms as u64)).collect(),
            None => Vec::new(),
        };
        json!({
            "elapsed_s": started.elapsed().as_secs(),
            "sent": st.sent,
            "queued": st.queued,
            "rejected": st.rejected,
            "http_errors": st.errors,
            "height": height,
            "start_height": start_height,
            "end_height": end_height,
            "load_s": (secs * 10.0).round() / 10.0,
            "blocks_per_s": ((end_height - start_height.min(end_height)) as f64 / secs * 100.0).round() / 100.0,
            "queued_tx_per_s": (st.queued as f64 / secs * 100.0).round() / 100.0,
            "mempool": status["mempool"],
            "checkpoints": status["checkpoints"],
            "receipts": { "ok": st.receipts_ok, "rejected_by_code": st.receipts_rejected, "waiting": st.inflight.len() },
            "soft_latency_ms": { "n": st.soft_ms.len(), "p50": pct(&st.soft_ms, 50.0), "p99": pct(&st.soft_ms, 99.0) },
            "hard_latency_ms": { "n": hard.len(), "p50": pct(&hard, 50.0), "p99": pct(&hard, 99.0) },
            "hard_samples": hard_samples,
            "host_cpu_insns_per_block": { "n": cpu.len(), "p50": pct(&cpu, 50.0), "p99": pct(&cpu, 99.0) },
            "state_hash": status["state_hash"],
        })
    }
}

/// One account's sender: a request at a time, at `per_sec`.
async fn sender(sh: Arc<Shared>, mut a: Acct, seed: u64, per_sec: f64) {
    let mut rng = Rng(seed);
    let mut tick = tokio::time::interval(Duration::from_secs_f64(1.0 / per_sec));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Spread the accounts' first requests over one interval.
    tokio::time::sleep(Duration::from_secs_f64(
        (rng.below(1000) as f64 / 1000.0) / per_sec,
    ))
    .await;
    while !sh.stop.load(Ordering::Relaxed) {
        tick.tick().await;
        if let Err(e) = sh.send_one(&mut a, &mut rng).await {
            eprintln!("tx: {e}");
            sh.stats().errors += 1;
        }
    }
}

/// Streams one account's receipts from `WS /v1/stream`.
async fn watch(url: String, g: String, tx: mpsc::UnboundedSender<Value>) {
    let ws_url = format!("{}/v1/stream", url.replacen("http", "ws", 1));
    loop {
        if let Ok((mut ws, _)) = tokio_tungstenite::connect_async(&ws_url).await {
            let sub = json!({ "blocks": false, "markets": [], "account": g }).to_string();
            if ws
                .send(tokio_tungstenite::tungstenite::Message::Text(sub.into()))
                .await
                .is_ok()
            {
                while let Some(Ok(m)) = ws.next().await {
                    if let tokio_tungstenite::tungstenite::Message::Text(t) = m {
                        if let Ok(v) = serde_json::from_str::<Value>(t.as_str()) {
                            if v["type"] == "receipt" && tx.send(v).is_err() {
                                return;
                            }
                        }
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn read_key(path: &Path) -> Result<SigningKey> {
    let s = std::fs::read_to_string(path)?;
    let k = stellar_strkey::ed25519::PrivateKey::from_string(s.trim())
        .map_err(|e| anyhow::anyhow!("{}: {e:?}", path.display()))?;
    Ok(SigningKey::from_bytes(&k.0))
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let lane = LaneFile::load(&args.lane)?;
    let (_, config_bytes, _) = caravel_node::lane_toml::genesis(&lane)?;
    let token = if args.no_fund && args.no_oracle {
        String::new()
    } else {
        std::env::var(&args.token_env).with_context(|| format!("set {}", args.token_env))?
    };
    let keys: Vec<SigningKey> = match &args.key_dir {
        Some(dir) => {
            let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "key"))
                .collect();
            files.sort();
            files.iter().map(|p| read_key(p)).collect::<Result<_>>()?
        }
        None => (0..args.accounts)
            .map(|i| key(args.seed_base + i))
            .collect(),
    };
    if keys.is_empty() {
        bail!("no accounts");
    }
    let mut accounts: Vec<Acct> = keys
        .into_iter()
        .map(|k| {
            let pk = k.verifying_key().to_bytes();
            Acct {
                g: caravel_lane::views::g_address(&pk),
                key: k,
                pk,
                nonce: 0,
            }
        })
        .collect();
    let markets = lane
        .markets
        .iter()
        .map(|m| Market {
            id: m.market_id,
            tick: m.tick,
            price: 0,
        })
        .collect::<Vec<_>>();
    let sh = Arc::new(Shared {
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?,
        url: args.url.trim_end_matches('/').to_string(),
        token,
        lane_id: lane.lane_id(),
        config_hash: sha256(&config_bytes),
        markets: Mutex::new(markets),
        stats: Mutex::new(Stats::default()),
        stop: AtomicBool::new(false),
    });
    let (_, status) = sh.get("/v1/status").await?;
    if status["lane_id"].as_str() != Some(hex(&sh.lane_id).as_str()) {
        bail!("the sequencer runs another lane: {}", status["lane_id"]);
    }
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    if args.no_oracle {
        sh.read_prices().await?;
        if sh
            .markets
            .lock()
            .expect("markets lock")
            .iter()
            .any(|m| m.price <= 0)
        {
            bail!("the lane has no oracle prices yet");
        }
    } else {
        // Start near the testkit levels (BTC $65,000, ETH $3,500, XLM $0.40).
        for m in sh.markets.lock().expect("markets lock").iter_mut() {
            m.price = match m.id {
                1 => 65_000_000,
                2 => 35_000_000,
                _ => 40_000_000,
            };
        }
        sh.publish_prices(&mut rng).await?;
    }
    if !args.no_fund {
        sh.fund(&accounts).await?;
    }
    sh.load_nonces(&mut accounts).await?;

    let (rx_tx, mut rx) = mpsc::unbounded_channel();
    if args.measure {
        for a in &accounts {
            tokio::spawn(watch(sh.url.clone(), a.g.clone(), rx_tx.clone()));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        let s = sh.clone();
        tokio::spawn(async move {
            while let Some(v) = rx.recv().await {
                s.on_receipt(&v);
            }
        });
    }
    // Prices every 2 s, and with --measure a status probe every second.
    let s = sh.clone();
    let no_oracle = args.no_oracle;
    tokio::spawn(async move {
        let mut rng = Rng(0xD1B5_4A32_D192_ED03);
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let r = if no_oracle {
                s.read_prices().await
            } else {
                s.publish_prices(&mut rng).await
            };
            if let Err(e) = r {
                eprintln!("prices: {e}");
                s.stats().errors += 1;
            }
        }
    });
    if args.measure {
        let s = sh.clone();
        tokio::spawn(async move {
            let mut accepted = (0, 0);
            loop {
                let _ = s.probe(&mut accepted).await;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
    }

    let start_height = sh.height().await?;
    let started = Instant::now();
    let per_sec = args.tps.max(1) as f64 / accounts.len() as f64;
    let workers: Vec<_> = accounts
        .into_iter()
        .enumerate()
        .map(|(i, a)| {
            tokio::spawn(sender(
                sh.clone(),
                a,
                0x9E37_79B9_7F4A_7C15 ^ (i as u64 + 1).wrapping_mul(0xA24B_AED4_963E_E407),
                per_sec,
            ))
        })
        .collect();
    let mut last_report = Instant::now();
    while started.elapsed() < Duration::from_secs(args.duration_secs) {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if last_report.elapsed() >= Duration::from_secs(args.report_secs) {
            println!("{}", sh.report(started, start_height, None).await);
            last_report = Instant::now();
        }
    }
    sh.stop.store(true, Ordering::Relaxed);
    for w in workers {
        let _ = w.await;
    }
    let window = Some((started.elapsed().as_secs_f64(), sh.height().await?));
    // Let the last receipts and their checkpoint arrive before the final numbers.
    if args.measure {
        let drain = Instant::now();
        loop {
            let (waiting_hard, inflight) = {
                let st = sh.stats();
                (st.waiting_hard.len(), st.inflight.len())
            };
            let receipts_due = inflight > 0 && drain.elapsed() < Duration::from_secs(15);
            if drain.elapsed() > Duration::from_secs(150) || (waiting_hard == 0 && !receipts_due) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    let mut summary = sh.report(started, start_height, window).await;
    summary["final"] = json!(true);
    println!("{summary}");
    Ok(())
}
