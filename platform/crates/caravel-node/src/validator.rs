//! `caravel-node validator` (spec §15): follows the sequencer, re-executes
//! every block through the engine Wasm, signs only headers it computed
//! itself, and serves blocks, checkpoints and proofs from its own store.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Router;
use caravel_core::block::BlockRecordV1;
use caravel_runtime::checkpoint::{network_id, settlement_addr_hash, sha256, HeaderIds};
use caravel_runtime::sequencer::{hex, Executor};
use caravel_runtime::store::{CheckpointStatus, Store};
use caravel_runtime::validator::{FollowError, Follower, Refusal};
use caravel_runtime::views;
use caravel_runtime::WasmExecutor;
use ed25519_dalek::SigningKey;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{self, ok, ApiError, ApiResult};
use crate::app::NodeApp;
use crate::lane_toml::LaneFile;
use crate::node_config::{check_network, parse_contract, parse_hash};
use crate::replay::{OnChainConfig, ReplaySource, RpcSource};
use crate::sequencer::now_ms;
use crate::stellar_rpc::Rpc;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidatorFile {
    validator: ValidatorSection,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidatorSection {
    listen: String,
    sequencer_url: String,
    /// A file holding the validator's `S...` secret key, relative to this file.
    key_file: PathBuf,
    lane: PathBuf,
    engine_wasm: PathBuf,
    engine_wasm_sha256: String,
    db: PathBuf,
    network_passphrase: String,
    settlement_contract: String,
    /// Stellar RPC, to learn which checkpoint the contract accepted.
    #[serde(default)]
    rpc_url: Option<String>,
    #[serde(default = "default_poll_ms")]
    poll_ms: u64,
    #[serde(default = "default_stellar_poll_secs")]
    stellar_poll_secs: u64,
    #[serde(default)]
    cors_origins: Vec<String>,
    /// The sequencer's `G...` key: `/v1/sign` requests must be signed by it
    /// (DEC-095).
    #[serde(default)]
    sequencer_key: Option<String>,
}

fn default_poll_ms() -> u64 {
    200
}

fn default_stellar_poll_secs() -> u64 {
    10
}

pub struct ValidatorConfig {
    pub listen: String,
    pub sequencer_url: String,
    pub key: SigningKey,
    pub lane: LaneFile,
    pub engine_wasm: PathBuf,
    pub engine_wasm_hash: [u8; 32],
    pub db: PathBuf,
    pub ids: HeaderIds,
    pub settlement_contract: [u8; 32],
    pub network_passphrase: String,
    pub rpc_url: Option<String>,
    pub poll_ms: u64,
    pub stellar_poll_secs: u64,
    pub cors_origins: Vec<String>,
    /// Only requests this key signed may ask for a signature (DEC-095).
    pub sequencer_key: Option<[u8; 32]>,
}

/// Reads an `S...` secret key file.
pub fn read_key_file(path: &Path) -> Result<SigningKey> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading key file {}", path.display()))?;
    let seed = stellar_strkey::ed25519::PrivateKey::from_string(text.trim())
        .map_err(|e| anyhow!("{} is not an S... secret key: {e:?}", path.display()))?;
    Ok(SigningKey::from_bytes(&seed.0))
}

impl ValidatorConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let file: ValidatorFile =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let dir = path.parent().unwrap_or(Path::new("."));
        let v = file.validator;
        check_network(&v.network_passphrase)?;
        let lane = LaneFile::load(&dir.join(&v.lane))?;
        lane.check_node_settings()?;
        let settlement_contract = parse_contract(&v.settlement_contract)?;
        let engine_wasm_hash = parse_hash(&v.engine_wasm_sha256)?;
        lane.check_engine(&engine_wasm_hash)?;
        Ok(Self {
            listen: v.listen,
            sequencer_url: v.sequencer_url.trim_end_matches('/').to_string(),
            key: read_key_file(&dir.join(v.key_file))?,
            lane,
            engine_wasm: dir.join(v.engine_wasm),
            engine_wasm_hash,
            db: dir.join(v.db),
            ids: HeaderIds {
                network_id: network_id(&v.network_passphrase),
                settlement_addr_hash: settlement_addr_hash(&settlement_contract),
                engine_wasm_hash,
            },
            settlement_contract,
            network_passphrase: v.network_passphrase,
            rpc_url: v.rpc_url,
            poll_ms: v.poll_ms,
            stellar_poll_secs: v.stellar_poll_secs.max(1),
            cors_origins: v.cors_origins,
            sequencer_key: v
                .sequencer_key
                .as_deref()
                .map(crate::lane_toml::parse_account)
                .transpose()?,
        })
    }
}

/// A running validator: the follower and where it follows from.
pub struct ValidatorNode<A: NodeApp> {
    pub app: A,
    pub follower: Mutex<Follower<A>>,
    sequencer_url: String,
    sequencer_key: Option<[u8; 32]>,
    lane_name: String,
    identity: api::Identity,
    /// Node-side timings (F-01): block fetch, lock wait, sign.
    perf: Mutex<caravel_runtime::perf::Perf>,
}

impl<A: NodeApp> ValidatorNode<A> {
    fn record(&self, phase: &'static str, took: Duration) {
        self.perf.lock().expect("perf lock").record(phase, took);
    }

    fn with<T>(&self, f: impl FnOnce(&mut Follower<A>) -> T) -> T {
        f(&mut self.follower.lock().expect("follower lock"))
    }
}

/// Opens the store, checks genesis, and starts following. Validators always
/// execute through the Wasm (spec §14.5).
pub async fn start<A: NodeApp>(
    app: A,
    cfg: ValidatorConfig,
) -> Result<(Arc<ValidatorNode<A>>, Router)> {
    let (_, config_bytes, genesis_state) = crate::lane_toml::genesis(&app, &cfg.lane)?;
    let config_hash = sha256(&config_bytes);
    let (exec_cpu_limit, exec_mem_limit) = app.exec_limits(&config_bytes)?;
    let exec = WasmExecutor::from_file(
        &cfg.engine_wasm,
        cfg.engine_wasm_hash,
        exec_cpu_limit,
        exec_mem_limit,
    )
    .context("loading the engine Wasm")?;
    let (wasm_genesis, _) = exec
        .genesis(&config_bytes)
        .map_err(|e| anyhow!("wasm genesis: {e:?}"))?;
    if wasm_genesis != genesis_state {
        bail!("Wasm and native genesis differ");
    }
    if let Some(url) = &cfg.rpc_url {
        let want = OnChainConfig {
            lane_id: cfg.lane.lane_id(),
            engine_wasm_hash: cfg.engine_wasm_hash,
            genesis_state_hash: sha256(&genesis_state),
            config_hash,
        };
        check_contract(url, cfg.settlement_contract, &want).await?;
    }
    if let Some(dir) = cfg.db.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut store = Store::open(&cfg.db, &cfg.lane.lane_id(), &config_hash, &genesis_state)
        .context("opening the store")?;
    // A validator fetches blocks again after a power loss; what it signed
    // is always on disk first (F-07).
    store.set_head_every(crate::head_every(cfg.lane.node.block_time_ms));
    store.set_block_durability(caravel_runtime::store::Durability::Normal)?;
    let follower = Follower::open(
        app.clone(),
        Executor::Wasm(exec),
        store,
        cfg.ids,
        cfg.key.clone(),
    )?;
    tracing::info!(height = follower.height(), key = %views::g_address(&follower.public_key()), "validator starting; genesis state hash {}", hex(&sha256(&genesis_state)));
    let node = Arc::new(ValidatorNode {
        app,
        follower: Mutex::new(follower),
        sequencer_url: cfg.sequencer_url.clone(),
        sequencer_key: cfg.sequencer_key,
        lane_name: cfg.lane.lane.name.clone(),
        identity: api::Identity {
            lane_id: cfg.lane.lane_id(),
            config_hash,
            settlement: cfg.settlement_contract,
            engine_wasm_hash: cfg.engine_wasm_hash,
            network_passphrase: cfg.network_passphrase.clone(),
        },
        perf: Mutex::new(caravel_runtime::perf::Perf::new()),
    });
    let router = router(node.clone(), &cfg.cors_origins);
    tokio::spawn(follow_loop(
        node.clone(),
        cfg.sequencer_url.clone(),
        cfg.poll_ms,
    ));
    tokio::spawn(prune_loop(node.clone()));
    if let Some(url) = cfg.rpc_url.clone() {
        tokio::spawn(stellar_loop(
            node.clone(),
            url,
            cfg.settlement_contract,
            cfg.stellar_poll_secs,
        ));
    } else {
        tracing::warn!("no rpc_url: acceptance on Stellar is unknown, so /v1/proofs/escape has nothing to serve");
    }
    Ok((node, router))
}

/// Refuses to start when the settlement contract on Stellar commits to
/// another lane, engine Wasm, genesis config or genesis state than this node
/// runs (spec §24).
async fn check_contract(url: &str, contract: [u8; 32], want: &OnChainConfig) -> Result<()> {
    let source = RpcSource {
        rpc: Rpc::new(url)?,
        contract,
    };
    let got = source
        .config()
        .await
        .context("reading the settlement contract's config from Stellar RPC")?;
    for (name, got, want) in [
        ("lane_id", got.lane_id, want.lane_id),
        (
            "engine_wasm_hash",
            got.engine_wasm_hash,
            want.engine_wasm_hash,
        ),
        (
            "genesis_state_hash",
            got.genesis_state_hash,
            want.genesis_state_hash,
        ),
        ("config_hash", got.config_hash, want.config_hash),
    ] {
        if got != want {
            bail!(
                "the settlement contract's {name} is {}, but this node has {}",
                hex(&got),
                hex(&want)
            );
        }
    }
    tracing::info!("the settlement contract commits to this lane, engine Wasm and genesis");
    Ok(())
}

pub async fn run<A: NodeApp>(app: A, cfg: ValidatorConfig) -> Result<()> {
    let addr: SocketAddr = cfg
        .listen
        .parse()
        .with_context(|| format!("listen address {:?}", cfg.listen))?;
    let (_node, router) = start(app, cfg).await?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, "listening");
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

/// Catch-up and live follow: fetch `/v1/blocks/{h}` from our height on.
async fn follow_loop<A: NodeApp>(app: Arc<ValidatorNode<A>>, sequencer: String, poll_ms: u64) {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("http client");
    loop {
        let (height, halted) = app.with(|f| (f.height(), f.halted().map(str::to_string)));
        if halted.is_some() {
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }
        let next = height + 1;
        let t_fetch = std::time::Instant::now();
        let fetched = fetch_block(&http, &sequencer, next).await;
        let record = match fetched {
            Ok(Some(r)) => {
                app.record("fetch", t_fetch.elapsed());
                r
            }
            Ok(None) => {
                tokio::time::sleep(Duration::from_millis(poll_ms)).await;
                continue;
            }
            Err(e) => {
                tracing::warn!("fetching block {next}: {e}");
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        let a = app.clone();
        let result = tokio::task::spawn_blocking(move || {
            let t_lock = std::time::Instant::now();
            let mut f = a.follower.lock().expect("follower lock");
            a.record("lock_wait", t_lock.elapsed());
            f.apply(&record, now_ms())
        })
        .await;
        match result {
            Ok(Ok(applied)) => {
                for flag in &applied.flags {
                    tracing::error!(
                        height = applied.height,
                        "live check failed, block marked suspicious: {flag}"
                    );
                }
                if let Some(seq) = applied.checkpoint {
                    tracing::info!(seq, height = applied.height, "checkpoint computed");
                }
            }
            Ok(Err(FollowError::Mismatch(why))) => tracing::error!(
                "STOPPED FOLLOWING, refusing to sign until an operator intervenes: {why}"
            ),
            Ok(Err(e)) => {
                tracing::error!("applying block {next}: {e}");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(e) => tracing::error!("apply task failed: {e}"),
        }
    }
}

async fn fetch_block(
    http: &reqwest::Client,
    sequencer: &str,
    height: u64,
) -> Result<Option<BlockRecordV1>> {
    let resp = http
        .get(format!("{sequencer}/v1/blocks/{height}"))
        .send()
        .await?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        bail!("status {}", resp.status());
    }
    let v: Value = resp.json().await?;
    let raw = caravel_runtime::sequencer::unhex(
        v["record_hex"]
            .as_str()
            .ok_or_else(|| anyhow!("no record_hex"))?,
    )
    .ok_or_else(|| anyhow!("record_hex is not hex"))?;
    Ok(Some(
        BlockRecordV1::decode(&raw).map_err(|e| anyhow!("record does not decode: {e:?}"))?,
    ))
}

/// Polls the contract's `LastCkpt` and marks our matching checkpoints accepted.
async fn stellar_loop<A: NodeApp>(
    app: Arc<ValidatorNode<A>>,
    url: String,
    contract: [u8; 32],
    every_secs: u64,
) {
    let rpc = match Rpc::new(&url) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("rpc client: {e}");
            return;
        }
    };
    loop {
        match rpc.last_checkpoint(&contract).await {
            Ok(Some(last)) if last.seq > 0 => {
                let matched = app.with(|f| f.observe_accepted(last.seq, &last.header_hash));
                match matched {
                    Ok(true) => {}
                    Ok(false) => tracing::debug!(
                        seq = last.seq,
                        "Stellar accepted a checkpoint this validator has not computed (yet)"
                    ),
                    Err(e) => tracing::error!("recording acceptance: {e}"),
                }
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("reading last_checkpoint from Stellar: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(every_secs)).await;
    }
}

/// Prunes old snapshots and accepted batches (DEC-105), a bounded pass at a time.
async fn prune_loop<A: NodeApp>(app: Arc<ValidatorNode<A>>) {
    loop {
        tokio::time::sleep(crate::PRUNE_EVERY).await;
        let a = app.clone();
        let r =
            tokio::task::spawn_blocking(move || a.with(|f| f.store_mut().prune(crate::PRUNE_ROWS)))
                .await;
        crate::log_pruned(r);
    }
}

// --- Routes (spec §15) ---------------------------------------------------------------------

pub fn router<A: NodeApp>(app: Arc<ValidatorNode<A>>, cors_origins: &[String]) -> Router {
    let mut r = Router::new()
        .route("/v1/sign", post(sign::<A>))
        .route("/v1/status", get(status::<A>))
        .route("/v1/blocks/{height}", get(block::<A>))
        .route("/v1/checkpoints/{seq}", get(checkpoint::<A>))
        .route("/v1/proofs/withdrawals", get(withdrawal_proofs::<A>))
        .route("/v1/proofs/escape", get(escape_proof::<A>))
        .with_state(app);
    if !cors_origins.is_empty() {
        use tower_http::cors::{AllowOrigin, CorsLayer};
        let origins: Vec<axum::http::HeaderValue> =
            cors_origins.iter().filter_map(|o| o.parse().ok()).collect();
        r = r.layer(
            CorsLayer::new()
                .allow_origin(AllowOrigin::list(origins))
                .allow_methods([axum::http::Method::GET]),
        );
    }
    r
}

type AppState<A> = State<Arc<ValidatorNode<A>>>;

#[derive(Deserialize)]
struct SignJson {
    header: String,
    batch: String,
}

async fn sign<A: NodeApp>(
    State(app): AppState<A>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult {
    if let Some(key) = &app.sequencer_key {
        let h = |n| headers.get(n).and_then(|v| v.to_str().ok());
        let now = now_ms() / 1000;
        if let Err(why) = crate::sign_request::verify(
            key,
            h(crate::sign_request::TIME_HEADER),
            h(crate::sign_request::SIGNATURE_HEADER),
            &body,
            now,
        ) {
            tracing::warn!("refused a /v1/sign request: {why}");
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "UNSIGNED",
                why.to_string(),
            ));
        }
    }
    let j: SignJson = serde_json::from_slice(&body)
        .map_err(|e| ApiError::bad_request("BAD_JSON", e.to_string()))?;
    let header = api::unhex(&j.header, "header")?;
    let batch = api::unhex(&j.batch, "batch")?;
    let a = app.clone();
    let t_sign = std::time::Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        a.with(|f| f.sign(&header, &batch).map(|sig| (sig, f.public_key())))
    })
    .await
    .map_err(ApiError::internal)?;
    app.record("sign", t_sign.elapsed());
    match result {
        Ok((sig, key)) => {
            ok(json!({ "signer_key": views::g_address(&key), "signature": hex(&sig) }))
        }
        Err(r) => {
            let status = match r {
                Refusal::NotCaughtUp { .. } => StatusCode::SERVICE_UNAVAILABLE,
                Refusal::BadHeader => StatusCode::BAD_REQUEST,
                Refusal::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
                _ => StatusCode::CONFLICT,
            };
            tracing::warn!("refused to sign: {r}");
            Err(ApiError::new(status, r.code(), r.to_string()))
        }
    }
}

async fn status<A: NodeApp>(State(app): AppState<A>) -> ApiResult {
    app.with(|f| {
        let store = f.store();
        let last = |s| store.last_checkpoint_with(s).map_err(ApiError::internal);
        let flags = store.flags_in(0, u64::MAX >> 1).map_err(ApiError::internal)?;
        let mut body = json!({
            "role": "validator",
            "lane_name": app.lane_name,
            "template": A::TEMPLATE,
            "key": views::g_address(&f.public_key()),
            "sequencer_url": app.sequencer_url,
            "height": f.height().to_string(),
            "state_hash": hex(&f.state_hash()),
            "halted": f.halted(),
            "suspicious_blocks": flags.iter().map(|(h, r)| json!({ "height": h.to_string(), "reason": r })).collect::<Vec<_>>(),
            "checkpoints": {
                "computed": store.last_checkpoint_seq().map_err(ApiError::internal)?.map(|s| s.to_string()),
                "accepted": last(CheckpointStatus::Accepted)?.map(|s| s.to_string()),
            },
            "last_signed_seq": store.last_signed_seq().map_err(ApiError::internal)?.map(|s| s.to_string()),
            "perf": crate::sequencer::perf_json(&f.perf, &app.perf.lock().expect("perf lock")),
        });
        app.identity.extend(&mut body);
        ok(body)
    })
}

async fn block<A: NodeApp>(State(app): AppState<A>, UrlPath(height): UrlPath<u64>) -> ApiResult {
    app.with(|f| api::block_json(&app.app, f.store(), height))
}

async fn checkpoint<A: NodeApp>(State(app): AppState<A>, UrlPath(seq): UrlPath<u64>) -> ApiResult {
    app.with(|f| api::checkpoint_json(f.store(), seq))
}

#[derive(Deserialize)]
struct AccountQuery {
    account: String,
}

async fn withdrawal_proofs<A: NodeApp>(
    State(app): AppState<A>,
    Query(q): Query<AccountQuery>,
) -> ApiResult {
    let key = api::parse_account(&q.account)?;
    app.with(|f| api::withdrawal_proofs(f.store(), &key))
}

async fn escape_proof<A: NodeApp>(
    State(app): AppState<A>,
    Query(q): Query<AccountQuery>,
) -> ApiResult {
    let key = api::parse_account(&q.account)?;
    app.with(|f| {
        let accepted = f
            .store()
            .last_checkpoint_with(CheckpointStatus::Accepted)
            .map_err(ApiError::internal)?;
        api::escape_proof(&app.app, f.store(), &key, accepted)
    })
}
