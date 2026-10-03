//! The payments sequencer over HTTP and WebSocket (spec §14.4), on the local
//! payments lane with 200 ms blocks, the payments engine Wasm of record, and
//! three stub validators (one offline, so 2 of 3 must be enough).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use axum::routing::post;
use axum::{Json, Router};
use caravel_core::checkpoint::CheckpointHeaderV1;
use caravel_core::inbox::{inbox_acc_preimage, InboxKind, InboxMsgV1};
use caravel_core::merkle::{verify, NativeSha256};
use caravel_core::preimage::{account_leaf_preimage, withdrawal_leaf_preimage};
use caravel_core::tx::{SigScheme, StandardBody, TxEnvelopeV1};
use caravel_harness::{key, pk, sha256, USDC};
use caravel_node::lane_toml::LaneFile;
use caravel_payments::{Transfer, TRANSFER};
use caravel_payments_node::PaymentsApp;
use caravel_runtime::sequencer::{hex, unhex};
use ed25519_dalek::Signer;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};

const A: u8 = 0x41;
const B: u8 = 0x42;
/// The local lane's treasury (seed 0x22, `lane.caravel-payments.local.toml`).
const TREASURY: u8 = 0x22;
const FEE: i128 = 100_000;
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
            root().join("lanes/payments/config/lane.caravel-payments.local.toml"),
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
            wasm = root()
                .join("target/contracts/payments_engine.wasm")
                .display(),
            hash = versions["lanes"]["payments"]["engine_wasm_sha256"]
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
        let (_app, router) = caravel_node::sequencer::start(PaymentsApp, &cfg)
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let file = LaneFile::load(&dir.path().join("lane.toml")).unwrap();
        let (_, config_bytes, _) = caravel_node::lane_toml::genesis(&PaymentsApp, &file).unwrap();
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
        let (status, _) = self
            .internal(
                "/internal/inbox",
                json!({ "index": self.inbox_n.to_string(), "msg_hex": hex(&msg.encode()), "acc_after_hex": hex(&acc) }),
            )
            .await;
        assert_eq!(status, 200);
        self.inbox_n += 1;
        self.inbox_acc = acc;
    }

    async fn account(&self, seed: u8) -> Value {
        self.get(&format!("/v1/accounts/{}", g(seed))).await.1
    }

    async fn nonce(&self, seed: u8) -> u64 {
        self.account(seed).await["next_nonce"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    }

    fn tx(&self, signer: u8, account: u8, nonce: u64, kind: u8, body: Vec<u8>) -> TxEnvelopeV1 {
        let mut tx = TxEnvelopeV1 {
            lane_id: self.lane_id,
            account: pk(account),
            signer: pk(signer),
            nonce,
            expiry_ms: now_ms() + 60_000,
            kind,
            sig_scheme: SigScheme::RawEd25519,
            body,
            signature: [0; 64],
        };
        tx.signature = key(signer)
            .sign(&sha256(&tx.tx_hash_preimage(&self.config_hash)))
            .to_bytes();
        tx
    }

    fn transfer(&self, from: u8, nonce: u64, to: u8, amount: i128) -> TxEnvelopeV1 {
        self.tx(
            from,
            from,
            nonce,
            TRANSFER,
            Transfer {
                to: pk(to),
                amount,
                memo: 42,
            }
            .encode(),
        )
    }

    async fn post_raw(&self, tx: &TxEnvelopeV1) -> (u16, Value) {
        let r = self
            .http
            .post(format!("{}/v1/tx", self.base))
            .header("content-type", "application/octet-stream")
            .body(tx.encode().unwrap())
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap())
    }

    async fn post_json(&self, tx: &TxEnvelopeV1) -> (u16, Value) {
        let r = self
            .http
            .post(format!("{}/v1/tx", self.base))
            .json(&json!({ "tx": hex(&tx.encode().unwrap()) }))
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

fn proof_of(v: &Value) -> Vec<[u8; 32]> {
    v["proof"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| unhex(p.as_str().unwrap()).unwrap().try_into().unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn payments_sequencer_end_to_end() {
    let mut lane = Lane::start().await;
    let (status, s) = lane.get("/v1/status").await;
    assert_eq!(status, 200);
    assert_eq!(s["lane_name"], "caravel-payments-local-0");
    assert_eq!(s["template"], "payments");
    assert_eq!(s["executor"], "wasm");
    // What it runs, for the deploy tool's host check.
    assert_eq!(s["config_hash"], hex(&lane.config_hash));
    assert_eq!(
        s["settlement"],
        stellar_strkey::Contract([7; 32]).to_string().as_str()
    );
    let versions: Value =
        serde_json::from_str(&std::fs::read_to_string(root().join("versions.json")).unwrap())
            .unwrap();
    assert_eq!(
        s["engine_wasm_sha256"],
        versions["lanes"]["payments"]["engine_wasm_sha256"]
    );
    assert_eq!(s["network_passphrase"], "Test SDF Network ; September 2015");
    assert!(s["release"]["version"].is_string());
    // Payments has no feed, so no feed route.
    let r = lane
        .http
        .post(format!("{}/internal/oracle", lane.base))
        .bearer_auth(TOKEN)
        .json(&json!({ "update": "00" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 404);
    // Perps routes don't exist on this lane.
    assert_eq!(lane.get("/v1/markets").await.0, 404);

    // The treasury exists from genesis, as system account 0.
    let t = lane.account(TREASURY).await;
    assert_eq!(
        (
            t["index"].as_u64(),
            t["system"].as_bool(),
            t["balance"].as_str()
        ),
        (Some(0), Some(true), Some("0"))
    );

    lane.deposit(A, 100 * USDC).await;
    lane.deposit(B, 50 * USDC).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!(
        "{}/v1/stream",
        lane.base.replace("http://", "ws://")
    ))
    .await
    .unwrap();
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        json!({ "blocks": true, "account": g(A) })
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
    assert_eq!(acct["balance"], (100 * USDC).to_string());
    assert_eq!(acct["system"], false);
    lane.until(&format!("/v1/accounts/{}", g(B)), "B's deposit", |s, _| {
        s == 200
    })
    .await;

    // Transfers by raw bytes and by JSON; the mempool refuses what can't run.
    let (na, nb) = (lane.nonce(A).await, lane.nonce(B).await);
    let (s1, v1) = lane.post_raw(&lane.transfer(A, na, B, 10 * USDC)).await;
    assert_eq!((s1, v1["status"].as_str()), (202, Some("queued")));
    let (s2, _) = lane.post_json(&lane.transfer(B, nb, A, 5 * USDC)).await;
    assert_eq!(s2, 202);
    let mut bad = lane.transfer(B, nb + 1, A, USDC);
    bad.signature[1] ^= 1;
    let (s3, v3) = lane.post_raw(&bad).await;
    assert_eq!((s3, v3["code"].as_str()), (400, Some("BAD_SIGNATURE")));
    let (s4, v4) = lane.post_json(&lane.transfer(0x77, 0, A, USDC)).await;
    assert_eq!((s4, v4["code"].as_str()), (400, Some("UNKNOWN_ACCOUNT")));
    let (s5, v5) = lane
        .post_raw(&lane.tx(A, A, na + 1, TRANSFER, vec![0; 55]))
        .await;
    assert_eq!((s5, v5["code"].as_str()), (400, Some("DECODE")));
    let (s6, v6) = lane.post_raw(&lane.tx(A, A, na + 1, 17, vec![0; 56])).await;
    assert_eq!((s6, v6["code"].as_str()), (400, Some("DECODE")));

    let a = lane
        .until(
            &format!("/v1/accounts/{}", g(A)),
            "the transfers",
            |_, v| v["balance"].as_str() == Some(&(95 * USDC - FEE).to_string()),
        )
        .await;
    assert_eq!(a["next_nonce"], (na + 1).to_string());
    // Nothing of A's is queued any more: the next transaction takes next_nonce.
    assert_eq!(a["pending_nonce"], a["next_nonce"]);
    lane.until(&format!("/v1/accounts/{}", g(B)), "B's balance", |_, v| {
        v["balance"].as_str() == Some(&(55 * USDC - FEE).to_string())
    })
    .await;
    assert_eq!(
        lane.account(TREASURY).await["balance"],
        (2 * FEE).to_string()
    );

    // A session key with PERM_TRANSFER, then a transfer it signs.
    let na = lane.nonce(A).await;
    let add = StandardBody::AddSessionKey {
        session_key: pk(0x5E),
        expires_at_ms: now_ms() + 3_600_000,
        permissions: caravel_payments::PERM_TRANSFER,
    };
    let (s7, _) = lane
        .post_raw(&lane.tx(A, A, na, add.kind(), add.encode()))
        .await;
    assert_eq!(s7, 202);
    lane.until(
        &format!("/v1/accounts/{}", g(A)),
        "the session key",
        |_, v| v["session_keys"].as_array().is_some_and(|k| k.len() == 1),
    )
    .await;
    let (s8, _) = lane
        .post_raw(
            &lane.tx(
                0x5E,
                A,
                na + 1,
                TRANSFER,
                Transfer {
                    to: pk(B),
                    amount: USDC,
                    memo: 0,
                }
                .encode(),
            ),
        )
        .await;
    assert_eq!(s8, 202);
    lane.until(
        &format!("/v1/accounts/{}", g(B)),
        "the session-key transfer",
        |_, v| v["balance"].as_str() == Some(&(56 * USDC - FEE).to_string()),
    )
    .await;

    // The stream delivered blocks, A's receipts and A's account.
    let mut seen = std::collections::BTreeSet::new();
    let mut transfer_receipt = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !(seen.contains("receipt")
        && seen.contains("block")
        && seen.contains("account")
        && transfer_receipt.is_some())
    {
        assert!(Instant::now() < deadline, "stream saw only {seen:?}");
        let Ok(Some(Ok(msg))) = tokio::time::timeout(Duration::from_secs(5), ws.next()).await
        else {
            panic!("stream closed")
        };
        if let tokio_tungstenite::tungstenite::Message::Text(t) = msg {
            let v: Value = serde_json::from_str(t.as_str()).unwrap();
            let ty = v["type"].as_str().unwrap().to_string();
            if ty == "receipt" && v.to_string().contains("TransferEvent") {
                transfer_receipt = Some(v.clone());
            }
            seen.insert(ty);
        }
    }

    // A withdraws; the next checkpoint commits it and 2 of 3 validators sign.
    let na = lane.nonce(A).await;
    let w = StandardBody::Withdraw { amount: 20 * USDC };
    let (s9, _) = lane
        .post_raw(&lane.tx(A, A, na, w.kind(), w.encode()))
        .await;
    assert_eq!(s9, 202);
    let mut withdrawal_seq = None;
    let mut header = None;
    while header.is_none() {
        let (status, pending) = lane.get_internal("/internal/checkpoints/pending").await;
        if status == 204 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        let p: u64 = pending["seq"].as_str().unwrap().parse().unwrap();
        assert_eq!(pending["sigs"].as_array().unwrap().len(), 2);
        let h = CheckpointHeaderV1::decode(&unhex(pending["header"].as_str().unwrap()).unwrap())
            .unwrap();
        let (status, _) = lane
            .internal(
                &format!("/internal/checkpoints/{p}/accepted"),
                json!({ "stellar_tx_hash": format!("{p:064x}"), "ledger": 100 + p }),
            )
            .await;
        assert_eq!(status, 200);
        if h.withdrawal_count == 1 {
            withdrawal_seq = Some(p);
            header = Some(h);
        }
    }
    let (seq, header) = (withdrawal_seq.unwrap(), header.unwrap());
    assert_eq!(header.withdrawals_total, 20 * USDC);

    // The withdrawal's proof and B's escape proof verify against the header.
    let (_, w) = lane
        .get(&format!("/v1/proofs/withdrawals?account={}", g(A)))
        .await;
    let leaf = &w["withdrawals"][0];
    assert_eq!(leaf["amount"], (20 * USDC).to_string());
    let index = leaf["index"].as_u64().unwrap() as u32;
    let hash = sha256(&withdrawal_leaf_preimage(
        &header.lane_id,
        header.seq,
        index,
        &pk(A),
        20 * USDC,
    ));
    assert!(verify(
        &NativeSha256,
        &hash,
        index,
        header.withdrawal_count,
        &proof_of(leaf),
        &header.withdrawals_root
    ));

    let (status, e) = lane
        .get(&format!("/v1/proofs/escape?account={}", g(B)))
        .await;
    assert_eq!(status, 200, "{e}");
    let eseq: u64 = e["seq"].as_str().unwrap().parse().unwrap();
    assert!(eseq >= seq);
    let (_, cp) = lane.get(&format!("/v1/checkpoints/{eseq}")).await;
    let eh =
        CheckpointHeaderV1::decode(&unhex(cp["header_hex"].as_str().unwrap()).unwrap()).unwrap();
    let equity: i128 = e["equity"].as_str().unwrap().parse().unwrap();
    assert_eq!(equity, 56 * USDC - FEE);
    let index = e["index"].as_u64().unwrap() as u32;
    let hash = sha256(&account_leaf_preimage(
        &eh.lane_id,
        eh.seq,
        index,
        &pk(B),
        equity,
    ));
    assert!(verify(
        &NativeSha256,
        &hash,
        index,
        eh.account_count,
        &proof_of(&e),
        &eh.accounts_root
    ));

    // Blocks are served with their receipts, events rendered as text.
    let (status, b) = lane.get("/v1/blocks/1").await;
    assert_eq!((status, b["height"].as_str()), (200, Some("1")));
    assert_eq!(lane.get("/v1/blocks/999999").await.0, 404);
    let receipt = transfer_receipt.unwrap();
    assert!(receipt.to_string().contains("fee: 100000"), "{receipt}");
}
