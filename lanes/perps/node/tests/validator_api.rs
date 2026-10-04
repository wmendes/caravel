//! Sequencer and validators over HTTP (spec §14.3, §15): three real
//! validators follow the sequencer and sign its checkpoints; a fourth follows
//! a proxy that tampers with one block and must stop and refuse to sign; a
//! mock Stellar RPC tells validators which checkpoint the contract accepted.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::Path as UrlPath;
use axum::routing::{get, post};
use axum::{Json, Router};
use caravel_core::inbox::{inbox_acc_preimage, InboxKind, InboxMsgV1};
use caravel_node::lane_toml::LaneFile;
use caravel_perps::native::verify_strict;
use caravel_perps_node::PerpsApp;
use caravel_runtime::checkpoint::sha256;
use caravel_runtime::sequencer::{hex, unhex};
use caravel_types::oracle::OracleUpdateV1;
use caravel_types::vectors::{key, pk};
use ed25519_dalek::Signer;
use serde_json::{json, Value};
use stellar_xdr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ContractId, ExtensionPoint,
    Hash, LedgerEntryData, Limits, ScAddress, ScBytes, ScContractInstance, ScMap, ScMapEntry,
    ScSymbol, ScVal, WriteXdr,
};
use tokio::net::TcpListener;

const TOKEN: &str = "test-internal-token-abcdef012345";
const CONTRACT: [u8; 32] = [7; 32];
const VALIDATORS: [u8; 3] = [0x61, 0x62, 0x63];

/// What the mock RPC reports as the contract's `LastCkpt`: `(seq, header_hash)`.
type LastAccepted = Arc<Mutex<Option<(u64, [u8; 32])>>>;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn g(seed: u8) -> String {
    caravel_runtime::views::g_address(&pk(seed))
}

fn now_ms() -> u64 {
    caravel_node::sequencer::now_ms()
}

async fn listener() -> (TcpListener, String) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    (l, url)
}

fn serve(l: TcpListener, r: Router) {
    tokio::spawn(async move { axum::serve(l, r).await.unwrap() });
}

fn wasm_hash() -> String {
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(root().join("versions.json")).unwrap())
            .unwrap();
    v["artifacts"]["engine_wasm_sha256"]
        .as_str()
        .unwrap()
        .to_string()
}

fn common_fields(db: &str) -> String {
    format!(
        "lane = \"lane.toml\"\nengine_wasm = \"{}\"\nengine_wasm_sha256 = \"{}\"\ndb = \"{db}\"\nnetwork_passphrase = \"Test SDF Network ; September 2015\"\nsettlement_contract = \"{}\"\n",
        root().join("target/contracts/perps_engine.wasm").display(),
        wasm_hash(),
        stellar_strkey::Contract(CONTRACT).to_string().as_str()
    )
}

/// The sequencer's request key (DEC-095): honest validators know it.
const SEQUENCER_SEED: u8 = 0x5e;

fn validator_config(dir: &Path, n: usize, seed: u8, sequencer: &str, rpc: Option<&str>) -> PathBuf {
    let key_file = dir.join(format!("v{n}.key"));
    std::fs::write(
        &key_file,
        stellar_strkey::ed25519::PrivateKey([seed; 32])
            .to_string()
            .as_str(),
    )
    .unwrap();
    let rpc_line = rpc
        .map(|r| format!("rpc_url = \"{r}\"\nstellar_poll_secs = 1\n"))
        .unwrap_or_default();
    // The tampered follower (no RPC) takes any request.
    let rpc_line = if rpc.is_some() {
        format!("{rpc_line}sequencer_key = \"{}\"\n", g(SEQUENCER_SEED))
    } else {
        rpc_line
    };
    let cfg = format!(
        "[validator]\nlisten = \"127.0.0.1:0\"\nsequencer_url = \"{sequencer}\"\nkey_file = \"v{n}.key\"\n{}poll_ms = 50\n{rpc_line}",
        common_fields(&format!("v{n}.sqlite"))
    );
    let path = dir.join(format!("validator-{n}.toml"));
    std::fs::write(&path, cfg).unwrap();
    path
}

async fn start_validator(
    dir: &Path,
    n: usize,
    seed: u8,
    sequencer: &str,
    rpc: Option<&str>,
) -> String {
    let (l, url) = listener().await;
    let path = validator_config(dir, n, seed, sequencer, rpc);
    let cfg = caravel_node::validator::ValidatorConfig::load(&path).unwrap();
    let (_app, router) = caravel_node::validator::start(PerpsApp, cfg).await.unwrap();
    serve(l, router);
    url
}

/// The settlement contract's `Config` as far as nodes read it: the lane,
/// engine Wasm, genesis state and genesis config it commits to.
fn contract_config(lane: &LaneFile, engine_wasm_hash: [u8; 32]) -> ScMapEntry {
    let (_, config_bytes, genesis_state) =
        caravel_node::lane_toml::genesis(&PerpsApp, lane).unwrap();
    let sym = |s: &str| ScVal::Symbol(ScSymbol(s.try_into().unwrap()));
    let bytes = |b: [u8; 32]| ScVal::Bytes(ScBytes(b.to_vec().try_into().unwrap()));
    let fields = [
        ("config_hash", sha256(&config_bytes)),
        ("engine_wasm_hash", engine_wasm_hash),
        ("genesis_state_hash", sha256(&genesis_state)),
        ("lane_id", lane.lane_id()),
    ];
    ScMapEntry {
        key: caravel_node::replay::variant("Config", vec![]),
        val: ScVal::Map(Some(ScMap(
            fields
                .iter()
                .map(|(k, v)| ScMapEntry {
                    key: sym(k),
                    val: bytes(*v),
                })
                .collect::<Vec<_>>()
                .try_into()
                .unwrap(),
        ))),
    }
}

fn engine_hash() -> [u8; 32] {
    unhex(&wasm_hash()).unwrap().try_into().unwrap()
}

/// A JSON-RPC stand-in for Stellar RPC that serves the settlement contract's
/// instance storage: its `Config`, and a settable `LastCkpt`.
async fn mock_rpc(last: LastAccepted, config: ScMapEntry) -> String {
    let app = Router::new().route(
        "/",
        post(move |Json(req): Json<Value>| {
            let (last, config) = (last.clone(), config.clone());
            async move {
                assert_eq!(req["method"], "getLedgerEntries");
                let mut storage = vec![config];
                if let Some((seq, header_hash)) = *last.lock().unwrap() {
                    let sym = |s: &str| ScVal::Symbol(ScSymbol(s.try_into().unwrap()));
                    let last_ckpt = ScVal::Map(Some(ScMap(
                        vec![
                            ScMapEntry { key: sym("header_hash"), val: ScVal::Bytes(ScBytes(header_hash.to_vec().try_into().unwrap())) },
                            ScMapEntry { key: sym("seq"), val: ScVal::U64(seq) },
                        ]
                        .try_into()
                        .unwrap(),
                    )));
                    storage.push(ScMapEntry { key: caravel_node::stellar_rpc::last_ckpt_key(), val: last_ckpt });
                }
                let data = LedgerEntryData::ContractData(ContractDataEntry {
                    ext: ExtensionPoint::V0,
                    contract: ScAddress::Contract(ContractId(Hash(CONTRACT))),
                    key: ScVal::LedgerKeyContractInstance,
                    durability: ContractDataDurability::Persistent,
                    val: ScVal::ContractInstance(ScContractInstance { executable: ContractExecutable::Wasm(Hash([9; 32])), storage: Some(ScMap(storage.try_into().unwrap())) }),
                });
                let key = req["params"]["keys"][0].clone();
                let entries = vec![json!({ "key": key, "xdr": data.to_xdr_base64(Limits::none()).unwrap(), "lastModifiedLedgerSeq": 10 })];
                Json(json!({ "jsonrpc": "2.0", "id": req["id"], "result": { "entries": entries, "latestLedger": 11 } }))
            }
        }),
    );
    let (l, url) = listener().await;
    serve(l, app);
    url
}

/// Serves the sequencer's blocks, with block `bad` altered.
async fn tampering_proxy(sequencer: String, bad: u64) -> String {
    let app = Router::new().route(
        "/v1/blocks/{h}",
        get(move |UrlPath(h): UrlPath<u64>| {
            let sequencer = sequencer.clone();
            async move {
                let r = reqwest::get(format!("{sequencer}/v1/blocks/{h}"))
                    .await
                    .unwrap();
                let status = r.status();
                let mut v: Value = r.json().await.unwrap_or(Value::Null);
                if h == bad && status.is_success() {
                    let mut raw = unhex(v["record_hex"].as_str().unwrap()).unwrap();
                    *raw.last_mut().unwrap() ^= 1; // state_hash_after
                    v["record_hex"] = hex(&raw).into();
                }
                (
                    axum::http::StatusCode::from_u16(status.as_u16()).unwrap(),
                    Json(v),
                )
            }
        }),
    );
    let (l, url) = listener().await;
    serve(l, app);
    url
}

struct Client {
    http: reqwest::Client,
}

impl Client {
    async fn get(&self, url: &str) -> (u16, Value) {
        let r = self.http.get(url).send().await.unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    async fn internal(&self, url: &str, body: Value) -> (u16, Value) {
        let r = self
            .http
            .post(url)
            .bearer_auth(TOKEN)
            .json(&body)
            .send()
            .await
            .unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    async fn until(&self, url: &str, what: &str, f: impl Fn(u16, &Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(40);
        loop {
            let (s, v) = self.get(url).await;
            if f(s, &v) {
                return v;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}: last {s} {v}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn validators_follow_sign_and_refuse_a_tampered_chain() {
    let dir = tempfile::tempdir().unwrap();
    let lane_text =
        std::fs::read_to_string(root().join("lanes/perps/config/lane.caravel-perps.local.toml"))
            .unwrap()
            .replace("block_time_ms = 1000", "block_time_ms = 200");
    std::fs::write(dir.path().join("lane.toml"), lane_text).unwrap();
    let lane = LaneFile::load(&dir.path().join("lane.toml")).unwrap();
    let lane_id = lane.lane_id();

    // Listeners first: the sequencer and validators name each other.
    let (seq_listener, seq_url) = listener().await;
    let last_accepted = Arc::new(Mutex::new(None));
    let rpc = mock_rpc(last_accepted.clone(), contract_config(&lane, engine_hash())).await;
    let mut urls = Vec::new();
    for (i, seed) in VALIDATORS.iter().enumerate() {
        urls.push(start_validator(dir.path(), i + 1, *seed, &seq_url, Some(&rpc)).await);
    }
    let validators = VALIDATORS
        .iter()
        .zip(&urls)
        .map(|(s, u)| format!("  {{ url = \"{u}\", key = \"{}\" }},\n", g(*s)))
        .collect::<String>();
    let seq_cfg = format!(
        "[sequencer]\nlisten = \"127.0.0.1:0\"\n{}internal_token_env = \"CARAVEL_TEST_TOKEN_V\"\nkey_file = \"seq.key\"\n\n[signers]\nepoch = 1\nthreshold = 2\nvalidators = [\n{validators}]\n",
        common_fields("seq.sqlite")
    );
    std::fs::write(dir.path().join("sequencer.toml"), seq_cfg).unwrap();
    std::fs::write(
        dir.path().join("seq.key"),
        stellar_strkey::ed25519::PrivateKey([SEQUENCER_SEED; 32])
            .to_string()
            .as_str(),
    )
    .unwrap();
    // SAFETY: set once, before the sequencer reads it; no other thread touches the environment.
    unsafe { std::env::set_var("CARAVEL_TEST_TOKEN_V", TOKEN) };
    let cfg = caravel_node::node_config::SequencerConfig::load(&dir.path().join("sequencer.toml"))
        .unwrap();
    let (_app, router) = caravel_node::sequencer::start(PerpsApp, &cfg)
        .await
        .unwrap();
    serve(seq_listener, router);
    let proxy = tampering_proxy(seq_url.clone(), 3).await;
    let bad_validator = start_validator(dir.path(), 4, 0x64, &proxy, None).await;

    // Some activity: two deposits and prices.
    let c = Client {
        http: reqwest::Client::new(),
    };
    let mut acc = [0u8; 32];
    for (i, seed) in [0x41u8, 0x42].into_iter().enumerate() {
        let msg = InboxMsgV1 {
            kind: InboxKind::Deposit,
            index: i as u64,
            lane_account: pk(seed),
            amount: 1_000 * 10_000_000,
            enqueued_at: now_ms() / 1000,
        };
        acc = sha256(&inbox_acc_preimage(&acc, &msg.encode()));
        let (s, _) = c.internal(&format!("{seq_url}/internal/inbox"), json!({ "index": i.to_string(), "msg_hex": hex(&msg.encode()), "acc_after_hex": hex(&acc) })).await;
        assert_eq!(s, 200);
    }
    let mut u = OracleUpdateV1 {
        market_id: 1,
        price: 65_000_000,
        publish_time_ms: now_ms(),
        oracle_key: pk(0x31),
        signature: [0; 64],
    };
    u.signature = key(0x31)
        .sign(&sha256(&u.signing_preimage(&lane_id)))
        .to_bytes();
    assert_eq!(
        c.internal(
            &format!("{seq_url}/internal/oracle"),
            json!({ "update": hex(&u.encode()) })
        )
        .await
        .0,
        200
    );

    // Checkpoint 1 is signed by the real validators, each signature valid.
    let status = c
        .until(
            &format!("{seq_url}/v1/status"),
            "checkpoint 1 signed",
            |_, v| v["checkpoints"]["signed"].is_string(),
        )
        .await;
    assert!(
        status["checkpoints"]["signed"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            >= 1
    );
    let (_, cp) = c.get(&format!("{seq_url}/v1/checkpoints/1")).await;
    let header = unhex(cp["header_hex"].as_str().unwrap()).unwrap();
    let sigs = cp["signatures"].as_array().unwrap();
    assert!(sigs.len() >= 2);
    let mut sorted: Vec<[u8; 32]> = VALIDATORS.iter().map(|s| pk(*s)).collect();
    sorted.sort();
    for s in sigs {
        let i = s["signer_index"].as_u64().unwrap() as usize;
        let sig: [u8; 64] = unhex(s["signature"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        assert!(verify_strict(&sorted[i], &sha256(&header), &sig));
    }
    // Every validator computed the same header and records what it signed.
    for u in &urls {
        // A validator may compute it a moment after the sequencer sealed it.
        let vcp = c
            .until(
                &format!("{u}/v1/checkpoints/1"),
                "checkpoint 1",
                |status, _| status == 200,
            )
            .await;
        assert_eq!(vcp["header_hex"], cp["header_hex"]);
        let st = c
            .until(&format!("{u}/v1/status"), "a signature", |_, v| {
                v["last_signed_seq"].is_string()
            })
            .await;
        assert!(st["halted"].is_null());
        // What it runs, for the deploy tool's host check.
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(
            st["settlement"],
            stellar_strkey::Contract(CONTRACT).to_string().as_str()
        );
        assert_eq!(st["engine_wasm_sha256"], hex(&engine_hash()));
        assert_eq!(st["config_hash"], status["config_hash"]);
        assert_eq!(
            st["network_passphrase"],
            "Test SDF Network ; September 2015"
        );
        assert_eq!(st["release"]["version"], env!("CARGO_PKG_VERSION"));
    }

    // The tampered validator stopped at block 3 and refuses to sign anything.
    let st = c
        .until(
            &format!("{bad_validator}/v1/status"),
            "the tampered validator to halt",
            |_, v| v["halted"].is_string(),
        )
        .await;
    assert_eq!(st["height"], "2");
    let batch = {
        let r = c
            .http
            .get(format!("{seq_url}/internal/checkpoints/pending"))
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap();
        let v: Value = r.json().await.unwrap();
        assert_eq!(v["seq"], "1");
        v["batch"].as_str().unwrap().to_string()
    };
    let r = c
        .http
        .post(format!("{bad_validator}/v1/sign"))
        .json(&json!({ "header": cp["header_hex"], "batch": batch }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 409);
    assert_eq!(r.json::<Value>().await.unwrap()["code"], "HALTED");
    // An honest validator refuses a header it did not compute.
    let mut forged = header.clone();
    forged[200] ^= 1;
    let body = serde_json::to_vec(&json!({ "header": hex(&forged), "batch": batch })).unwrap();
    let post = |headers: Vec<(&'static str, String)>| {
        let mut r = c
            .http
            .post(format!("{}/v1/sign", urls[0]))
            .header("content-type", "application/json")
            .body(body.clone());
        for (k, v) in headers {
            r = r.header(k, v);
        }
        r.send()
    };
    let seq_key = key(SEQUENCER_SEED);
    let now = now_ms() / 1000;
    let r = post(caravel_node::sign_request::sign(&seq_key, now, &body).to_vec())
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 409);
    // It answers only the sequencer (DEC-095): unsigned, signed by another
    // key, or signed long ago, the request is refused before anything runs.
    for headers in [
        vec![],
        caravel_node::sign_request::sign(&key(0x99), now, &body).to_vec(),
        caravel_node::sign_request::sign(&seq_key, now - 600, &body).to_vec(),
    ] {
        let r = post(headers).await.unwrap();
        assert_eq!(r.status().as_u16(), 401);
        assert_eq!(r.json::<Value>().await.unwrap()["code"], "UNSIGNED");
    }

    // Stellar accepts checkpoint 1: validators learn it from RPC and serve the escape proof.
    *last_accepted.lock().unwrap() = Some((1, sha256(&header)));
    let accepted = c
        .until(
            &format!("{}/v1/status", urls[0]),
            "acceptance from Stellar",
            |_, v| v["checkpoints"]["accepted"] == "1",
        )
        .await;
    assert_eq!(accepted["checkpoints"]["accepted"], "1");

    // F-01: both nodes report per-phase timings, in microseconds.
    let phases = |v: &Value| v["perf"]["phases"].clone();
    let vp = phases(&accepted);
    assert_eq!(accepted["perf"]["unit"], "us");
    for phase in [
        "fetch",
        "lock_wait",
        "execute",
        "decode",
        "commit",
        "checkpoint",
        "sign",
    ] {
        assert!(
            vp[phase]["n"].as_u64().unwrap_or(0) > 0,
            "validator phase {phase}: {vp}"
        );
        assert!(vp[phase]["p50_us"].as_u64().unwrap() <= vp[phase]["max_us"].as_u64().unwrap());
    }
    let (_, seq_status) = c.get(&format!("{seq_url}/v1/status")).await;
    let sp = phases(&seq_status);
    for phase in [
        "lock_wait",
        "queue_read",
        "build",
        "execute",
        "decode",
        "commit",
        "seal",
        "publish",
        "sign_collect",
        "seal_to_signed",
    ] {
        assert!(
            sp[phase]["n"].as_u64().unwrap_or(0) > 0,
            "sequencer phase {phase}: {sp}"
        );
    }

    let (s, proof) = c
        .get(&format!("{}/v1/proofs/escape?account={}", urls[0], g(0x41)))
        .await;
    assert_eq!(s, 200, "{proof}");
    assert_eq!(proof["seq"], "1");
    // The sequencer, once told by the relayer, serves the same leaf.
    assert_eq!(
        c.internal(
            &format!("{seq_url}/internal/checkpoints/1/accepted"),
            json!({ "stellar_tx_hash": "ab".repeat(32), "ledger": 10 })
        )
        .await
        .0,
        200
    );
    let (_, from_sequencer) = c
        .get(&format!("{seq_url}/v1/proofs/escape?account={}", g(0x41)))
        .await;
    assert_eq!(from_sequencer["seq"], "1");
    assert_eq!(from_sequencer, proof);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_validator_refuses_a_contract_committed_to_another_engine() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        root().join("lanes/perps/config/lane.caravel-perps.local.toml"),
        dir.path().join("lane.toml"),
    )
    .unwrap();
    let lane = LaneFile::load(&dir.path().join("lane.toml")).unwrap();
    let rpc = mock_rpc(
        Arc::new(Mutex::new(None)),
        contract_config(&lane, [0xEE; 32]),
    )
    .await;
    let path = validator_config(dir.path(), 1, 0x61, "http://127.0.0.1:9", Some(&rpc));
    let cfg = caravel_node::validator::ValidatorConfig::load(&path).unwrap();
    let err = caravel_node::validator::start(PerpsApp, cfg)
        .await
        .err()
        .expect("refused");
    assert!(
        err.to_string().contains("engine_wasm_hash"),
        "unexpected error: {err:#}"
    );
    // The same contract with this node's engine is accepted.
    let rpc = mock_rpc(
        Arc::new(Mutex::new(None)),
        contract_config(&lane, engine_hash()),
    )
    .await;
    let path = validator_config(dir.path(), 2, 0x62, "http://127.0.0.1:9", Some(&rpc));
    let cfg = caravel_node::validator::ValidatorConfig::load(&path).unwrap();
    assert!(caravel_node::validator::start(PerpsApp, cfg).await.is_ok());
}
