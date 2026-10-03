//! The sequencer over HTTP and WebSocket (spec §14.4), on the local lane with
//! 200 ms blocks, the engine Wasm of record, and three stub validators (one
//! offline, so 2 of 3 must be enough).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use axum::routing::post;
use axum::{Json, Router};
use caravel_core::checkpoint::CheckpointHeaderV1;
use caravel_core::inbox::{inbox_acc_preimage, InboxKind, InboxMsgV1};
use caravel_node::lane_toml::LaneFile;
use caravel_perps_node::PerpsApp;
use caravel_runtime::checkpoint::sha256;
use caravel_runtime::sequencer::{hex, unhex};
use caravel_types::oracle::OracleUpdateV1;
use caravel_types::tx::{LaneTxV1, PlaceOrder, Side, SigScheme, Tif, TxBody};
use caravel_types::vectors::{key, pk};
use ed25519_dalek::Signer;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};

const A: u8 = 0x41;
const B: u8 = 0x42;
const ORACLE: u8 = 0x31;
const USDC: i128 = 10_000_000;
const BTC_PRICE: i64 = 65_000_000;
const TOKEN: &str = "test-internal-token-0123456789";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn g(seed: u8) -> String {
    caravel_runtime::views::g_address(&pk(seed))
}

fn now_ms() -> u64 {
    caravel_node::sequencer::now_ms()
}

/// A validator stand-in that signs whatever it is sent with key `seed`.
async fn stub_validator(seed: u8) -> String {
    let app = Router::new().route(
        "/v1/sign",
        post(move |Json(body): Json<Value>| async move {
            let header = unhex(body["header"].as_str().unwrap()).unwrap();
            let sig = key(seed).sign(&sha256(&header)).to_bytes();
            Json(json!({ "signer_key": g(seed), "signature": hex(&sig) }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

struct Lane {
    lane_id: [u8; 32],
    config_hash: [u8; 32],
    base: String,
    http: reqwest::Client,
    inbox_n: u64,
    inbox_acc: [u8; 32],
    _dir: tempfile::TempDir,
}

impl Lane {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let lane_text = std::fs::read_to_string(
            root().join("lanes/perps/config/lane.caravel-perps.local.toml"),
        )
        .unwrap()
        .replace("block_time_ms = 1000", "block_time_ms = 200");
        std::fs::write(dir.path().join("lane.toml"), lane_text).unwrap();
        let versions: Value =
            serde_json::from_str(&std::fs::read_to_string(root().join("versions.json")).unwrap())
                .unwrap();
        let v1 = stub_validator(0x61).await;
        let v2 = stub_validator(0x62).await;
        let settlement = stellar_strkey::Contract([7; 32]).to_string();
        let cfg = format!(
            r#"
[sequencer]
listen = "127.0.0.1:0"
lane = "lane.toml"
engine_wasm = "{wasm}"
engine_wasm_sha256 = "{hash}"
db = "seq.sqlite"
network_passphrase = "Test SDF Network ; September 2015"
settlement_contract = "{settlement}"
internal_token_env = "CARAVEL_TEST_TOKEN"

[signers]
epoch = 1
threshold = 2
validators = [
  {{ url = "{v1}", key = "{k1}" }},
  {{ url = "{v2}", key = "{k2}" }},
  {{ url = "http://127.0.0.1:9", key = "{k3}" }},
]
"#,
            wasm = root().join("target/contracts/perps_engine.wasm").display(),
            hash = versions["artifacts"]["engine_wasm_sha256"]
                .as_str()
                .unwrap(),
            settlement = settlement.as_str(),
            k1 = g(0x61),
            k2 = g(0x62),
            k3 = g(0x63),
        );
        let path = dir.path().join("sequencer.toml");
        std::fs::write(&path, cfg).unwrap();
        // SAFETY: set once, before any thread reads the environment in this test binary.
        unsafe { std::env::set_var("CARAVEL_TEST_TOKEN", TOKEN) };
        let cfg = caravel_node::node_config::SequencerConfig::load(&path).unwrap();
        let (_app, router) = caravel_node::sequencer::start(PerpsApp, &cfg)
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let file = LaneFile::load(&dir.path().join("lane.toml")).unwrap();
        let (_, config_bytes, _) = caravel_node::lane_toml::genesis(&PerpsApp, &file).unwrap();
        Self {
            lane_id: file.lane_id(),
            config_hash: sha256(&config_bytes),
            base,
            http: reqwest::Client::new(),
            inbox_n: 0,
            inbox_acc: [0; 32],
            _dir: dir,
        }
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        let r = self
            .http
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null))
    }

    async fn internal(&self, path: &str, body: Value) -> (u16, Value) {
        let r = self
            .http
            .post(format!("{}{path}", self.base))
            .bearer_auth(TOKEN)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null))
    }

    async fn deposit(&mut self, seed: u8, amount: i128) {
        let msg = InboxMsgV1 {
            kind: InboxKind::Deposit,
            index: self.inbox_n,
            lane_account: pk(seed),
            amount,
            enqueued_at: now_ms() / 1000,
        };
        let acc = sha256(&inbox_acc_preimage(&self.inbox_acc, &msg.encode()));
        let (status, _) = self.internal("/internal/inbox", json!({ "index": self.inbox_n.to_string(), "msg_hex": hex(&msg.encode()), "acc_after_hex": hex(&acc) })).await;
        assert_eq!(status, 200);
        self.inbox_n += 1;
        self.inbox_acc = acc;
    }

    async fn oracle(&self, market_id: u16, price: i64) {
        let mut u = OracleUpdateV1 {
            market_id,
            price,
            publish_time_ms: now_ms(),
            oracle_key: pk(ORACLE),
            signature: [0; 64],
        };
        u.signature = key(ORACLE)
            .sign(&sha256(&u.signing_preimage(&self.lane_id)))
            .to_bytes();
        let (status, _) = self
            .internal("/internal/oracle", json!({ "update": hex(&u.encode()) }))
            .await;
        assert_eq!(status, 200);
    }

    async fn nonce(&self, seed: u8) -> u64 {
        let (_, a) = self.get(&format!("/v1/accounts/{}", g(seed))).await;
        a["next_nonce"].as_str().unwrap().parse().unwrap()
    }

    fn tx(&self, seed: u8, nonce: u64, body: TxBody) -> LaneTxV1 {
        let mut tx = LaneTxV1 {
            lane_id: self.lane_id,
            account: pk(seed),
            signer: pk(seed),
            nonce,
            expiry_ms: now_ms() + 60_000,
            sig_scheme: SigScheme::RawEd25519,
            body,
            signature: [0; 64],
        };
        tx.signature = key(seed)
            .sign(&sha256(&tx.tx_hash_preimage(&self.config_hash)))
            .to_bytes();
        tx
    }

    async fn post_raw(&self, tx: &LaneTxV1) -> (u16, Value) {
        let r = self
            .http
            .post(format!("{}/v1/tx", self.base))
            .header("content-type", "application/octet-stream")
            .body(tx.encode())
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap())
    }

    async fn post_json(&self, tx: &LaneTxV1) -> (u16, Value) {
        let r = self
            .http
            .post(format!("{}/v1/tx", self.base))
            .json(&json!({ "tx": hex(&tx.encode()) }))
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap())
    }

    /// Polls `path` until `f` accepts the body.
    async fn until(&self, path: &str, what: &str, f: impl Fn(u16, &Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let (status, v) = self.get(path).await;
            if f(status, &v) {
                return v;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}: last {status} {v}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

fn order(side: Side, price: i64, lots: i64) -> TxBody {
    TxBody::PlaceOrder(PlaceOrder {
        market_id: 1,
        side,
        tif: Tif::Gtc,
        reduce_only: false,
        price,
        lots,
        client_order_id: 7,
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn sequencer_api_end_to_end() {
    let mut lane = Lane::start().await;
    let (status, s) = lane.get("/v1/status").await;
    assert_eq!(status, 200);
    assert_eq!(s["lane_name"], "caravel-perps-local-0");
    assert_eq!(s["executor"], "wasm");
    assert_eq!(s["signers"]["validators"].as_array().unwrap().len(), 3);

    // The internal API needs the token.
    let r = lane
        .http
        .post(format!("{}/internal/oracle", lane.base))
        .json(&json!({ "update": "00" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 401);

    // Deposits, prices, and a WebSocket subscriber for A.
    lane.deposit(A, 10_000 * USDC).await;
    lane.deposit(B, 10_000 * USDC).await;
    // Out of order: the sequencer names the index it needs.
    let skip = InboxMsgV1 {
        kind: InboxKind::Deposit,
        index: 5,
        lane_account: pk(A),
        amount: USDC,
        enqueued_at: 1,
    };
    let (status, gap) = lane.internal("/internal/inbox", json!({ "index": "5", "msg_hex": hex(&skip.encode()), "acc_after_hex": hex(&[0u8; 32]) })).await;
    assert_eq!(
        (status, gap["code"].as_str(), gap["expected"].as_str()),
        (409, Some("INBOX_GAP"), Some("2"))
    );
    for (m, p) in [(1, BTC_PRICE), (2, 35_000_000), (3, 40_000_000)] {
        lane.oracle(m, p).await;
    }
    let (mut ws, _) = tokio_tungstenite::connect_async(format!(
        "{}/v1/stream",
        lane.base.replace("http://", "ws://")
    ))
    .await
    .unwrap();
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        json!({ "blocks": true, "markets": [1], "tickers": true, "account": g(A) })
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let acct = lane
        .until(&format!("/v1/accounts/{}", g(A)), "A's deposit", |s, _| {
            s == 200
        })
        .await;
    assert_eq!(acct["collateral"], (10_000 * USDC).to_string());
    lane.until(&format!("/v1/accounts/{}", g(B)), "B's deposit", |s, _| {
        s == 200
    })
    .await;
    lane.until("/v1/markets", "prices", |_, v| {
        v[0]["oracle_price"].as_str() == Some("65000000")
    })
    .await;

    // Orders by raw bytes and by JSON; a bad signature is refused.
    let (na, nb) = (lane.nonce(A).await, lane.nonce(B).await);
    let (s1, v1) = lane
        .post_raw(&lane.tx(A, na, order(Side::Buy, BTC_PRICE, 10)))
        .await;
    assert_eq!((s1, v1["status"].as_str()), (202, Some("queued")));
    let (s2, _) = lane
        .post_json(&lane.tx(B, nb, order(Side::Sell, BTC_PRICE, 4)))
        .await;
    assert_eq!(s2, 202);
    let mut bad = lane.tx(B, nb + 1, order(Side::Sell, BTC_PRICE, 1));
    bad.signature[1] ^= 1;
    let (s3, v3) = lane.post_raw(&bad).await;
    assert_eq!((s3, v3["code"].as_str()), (400, Some("BAD_SIGNATURE")));
    let (s4, v4) = lane
        .post_json(&lane.tx(0x77, 0, order(Side::Buy, BTC_PRICE, 1)))
        .await;
    assert_eq!((s4, v4["code"].as_str()), (400, Some("UNKNOWN_ACCOUNT")));

    let acct = lane
        .until(&format!("/v1/accounts/{}", g(A)), "A's fill", |_, v| {
            v["positions"].as_array().is_some_and(|p| !p.is_empty())
        })
        .await;
    assert_eq!(acct["positions"][0]["lots"], 4);
    assert_eq!(acct["open_orders"][0]["lots_remaining"], 6);
    let (_, book) = lane.get("/v1/markets/1/book?depth=5").await;
    assert_eq!(
        book["bids"][0],
        json!({ "price": BTC_PRICE.to_string(), "lots": 6, "orders": 1 })
    );
    let (_, trades) = lane.get("/v1/markets/1/trades?limit=10").await;
    assert_eq!(trades[0]["lots"], 4);
    assert_eq!(trades[0]["taker"], g(B));

    // Candles of the oracle price (H-15): the one-minute candle holds the
    // price posted above; an unknown interval or market is refused.
    let (_, candles) = lane.get("/v1/markets/1/candles?interval=1m").await;
    let last = candles.as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["close"], BTC_PRICE.to_string());
    assert!(last["t"].as_u64().unwrap() % 60_000 == 0);
    let (_, hourly) = lane.get("/v1/markets/1/candles?interval=1h&limit=1").await;
    assert_eq!(hourly.as_array().unwrap().len(), 1);
    assert_eq!(lane.get("/v1/markets/1/candles?interval=2m").await.0, 400);
    assert_eq!(lane.get("/v1/markets/99/candles").await.0, 404);

    // The stream delivered a block, the fill and A's receipt.
    let mut seen = std::collections::BTreeSet::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !(seen.contains("fill")
        && seen.contains("tickers")
        && seen.contains("receipt")
        && seen.contains("block")
        && seen.contains("account"))
    {
        assert!(Instant::now() < deadline, "stream saw only {seen:?}");
        let Ok(Some(Ok(msg))) = tokio::time::timeout(Duration::from_secs(5), ws.next()).await
        else {
            panic!("stream closed")
        };
        if let tokio_tungstenite::tungstenite::Message::Text(t) = msg {
            let v: Value = serde_json::from_str(t.as_str()).unwrap();
            if v["type"] == "tickers" {
                // Every market's price line, on every block.
                assert_eq!(v["markets"].as_array().unwrap().len(), 3);
                assert!(v["markets"][0]["oracle_price"].is_string());
                assert!(v["timestamp_ms"].is_string());
            }
            seen.insert(v["type"].as_str().unwrap().to_string());
        }
    }

    // A withdraws; the next checkpoint commits it and 2 of 3 validators sign.
    let na = lane.nonce(A).await;
    let (s5, _) = lane
        .post_raw(&lane.tx(A, na, TxBody::Withdraw { amount: 100 * USDC }))
        .await;
    assert_eq!(s5, 202);
    let withdrawal_seq = loop {
        let v = lane
            .until("/v1/status", "a signed checkpoint", |_, v| {
                v["checkpoints"]["signed"].is_string()
            })
            .await;
        let seq: u64 = v["checkpoints"]["signed"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let (_, cp) = lane.get(&format!("/v1/checkpoints/{seq}")).await;
        if cp["header"]["withdrawal_count"] == 1 {
            break seq;
        }
        // Earlier checkpoints: accept them in order, as the relayer would.
        let (status, pending) = lane.get_internal("/internal/checkpoints/pending").await;
        assert_eq!(status, 200);
        let p: u64 = pending["seq"].as_str().unwrap().parse().unwrap();
        lane.internal(
            &format!("/internal/checkpoints/{p}/accepted"),
            json!({ "stellar_tx_hash": format!("{p:064x}"), "ledger": 100 + p }),
        )
        .await;
    };
    // Accept everything up to the withdrawal's checkpoint, checking each pending head.
    loop {
        let (status, pending) = lane.get_internal("/internal/checkpoints/pending").await;
        if status == 204 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        let p: u64 = pending["seq"].as_str().unwrap().parse().unwrap();
        let header = unhex(pending["header"].as_str().unwrap()).unwrap();
        let sigs = pending["sigs"].as_array().unwrap();
        assert_eq!(sigs.len(), 2);
        let idx: Vec<u64> = sigs
            .iter()
            .map(|s| s["signer_index"].as_u64().unwrap())
            .collect();
        assert!(idx.windows(2).all(|w| w[0] < w[1]));
        let (status, _) = lane
            .internal(
                &format!("/internal/checkpoints/{p}/accepted"),
                json!({ "stellar_tx_hash": format!("{p:064x}"), "ledger": 100 + p }),
            )
            .await;
        assert_eq!(status, 200);
        if p == withdrawal_seq {
            let h = CheckpointHeaderV1::decode(&header).unwrap();
            assert_eq!(h.seq, p);
            break;
        }
    }

    // Proofs for the withdrawal and the escape leaf verify against the header.
    let (_, cp) = lane.get(&format!("/v1/checkpoints/{withdrawal_seq}")).await;
    assert_eq!(cp["status"], "accepted");
    let header =
        CheckpointHeaderV1::decode(&unhex(cp["header_hex"].as_str().unwrap()).unwrap()).unwrap();
    let (_, w) = lane
        .get(&format!("/v1/proofs/withdrawals?account={}", g(A)))
        .await;
    let leaf = &w["withdrawals"][0];
    assert_eq!(leaf["amount"], (100 * USDC).to_string());
    let proof: Vec<[u8; 32]> = leaf["proof"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| unhex(p.as_str().unwrap()).unwrap().try_into().unwrap())
        .collect();
    let index = leaf["index"].as_u64().unwrap() as u32;
    let hash = sha256(&caravel_types::preimage::withdrawal_leaf_preimage(
        &header.lane_id,
        header.seq,
        index,
        &pk(A),
        100 * USDC,
    ));
    assert!(caravel_merkle::verify(
        &caravel_merkle::NativeSha256,
        &hash,
        index,
        header.withdrawal_count,
        &proof,
        &header.withdrawals_root
    ));
    let (status, e) = lane
        .get(&format!("/v1/proofs/escape?account={}", g(B)))
        .await;
    assert_eq!(status, 200, "{e}");
    let last_accepted: u64 = lane.get("/v1/status").await.1["checkpoints"]["accepted"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(e["seq"], last_accepted.to_string());

    // Blocks are served with their receipts.
    let (status, b) = lane.get("/v1/blocks/1").await;
    assert_eq!((status, b["height"].as_str()), (200, Some("1")));
    assert_eq!(lane.get("/v1/blocks/999999").await.0, 404);
}

impl Lane {
    async fn get_internal(&self, path: &str) -> (u16, Value) {
        let r = self
            .http
            .get(format!("{}{path}", self.base))
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null))
    }
}
