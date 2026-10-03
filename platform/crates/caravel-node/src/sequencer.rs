//! The sequencer (spec §14), for any app: the block loop, checkpoint
//! signature collection, and the public, internal and WebSocket APIs.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use caravel_core::block::{BlockInputV1, Entry};
use caravel_core::inbox::InboxMsgV1;
use caravel_runtime::checkpoint::sha256;
use caravel_runtime::mempool::{tx_hash, verify_strict};
use caravel_runtime::sequencer::{
    hex, unhex, Core, Executor, InboxReport, Incident, Produced, SequencerConfig as CoreConfig,
};
use caravel_runtime::store::{CheckpointRow, CheckpointStatus, Store};
use caravel_runtime::views;
use caravel_runtime::WasmExecutor;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, Notify};

use crate::api::{self, ok, ApiError, ApiResult};
use crate::app::NodeApp;
use crate::node_config::{SequencerConfig, Signers};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// What the stream sends after each block or checkpoint change.
pub enum StreamEvent<A: NodeApp> {
    Block {
        produced: Arc<Produced<A::State>>,
        view: A::BlockView,
        config_hash: [u8; 32],
    },
    Checkpoint {
        seq: u64,
        status: &'static str,
    },
}

// By hand: a derive would ask for `A::State: Clone`, but the state is shared.
impl<A: NodeApp> Clone for StreamEvent<A> {
    fn clone(&self) -> Self {
        match self {
            Self::Block {
                produced,
                view,
                config_hash,
            } => Self::Block {
                produced: produced.clone(),
                view: view.clone(),
                config_hash: *config_hash,
            },
            Self::Checkpoint { seq, status } => Self::Checkpoint { seq: *seq, status },
        }
    }
}

/// A running sequencer: the core, the latest state for views, and the stream.
pub struct SequencerNode<A: NodeApp> {
    pub app: A,
    pub core: Mutex<Core<A>>,
    pub snapshot: RwLock<Arc<A::State>>,
    cache: Mutex<A::Cache>,
    pub events: broadcast::Sender<StreamEvent<A>>,
    sealed: Notify,
    halted: Mutex<Option<String>>,
    /// The last block's `step` metering: host metering, not network fees (spec §14.5).
    last_metering: Mutex<(u64, caravel_runtime::Metering)>,
    pub lane_name: String,
    pub block_time_ms: u64,
    pub checkpoint_every_blocks: u64,
    pub signers: Option<Signers>,
    /// Signs each `/v1/sign` request (DEC-095).
    request_key: Option<ed25519_dalek::SigningKey>,
    token: String,
    production: bool,
    identity: api::Identity,
}

impl<A: NodeApp> SequencerNode<A> {
    pub fn store<T>(&self, f: impl FnOnce(&Store) -> T) -> T {
        f(self.core.lock().expect("core lock").store())
    }

    /// The state after the latest block.
    pub fn state(&self) -> Arc<A::State> {
        self.snapshot.read().expect("snapshot lock").clone()
    }

    /// The app's view cache.
    pub fn cache(&self) -> MutexGuard<'_, A::Cache> {
        self.cache.lock().expect("cache lock")
    }

    fn publish(&self, produced: Produced<A::State>, config_hash: [u8; 32]) {
        for i in &produced.incidents {
            match i {
                Incident::Quarantined { .. }
                | Incident::InboxMismatch { .. }
                | Incident::PrecheckFailed { .. } => tracing::error!(?i, "incident"),
                Incident::BudgetExceeded { .. } => tracing::warn!(?i, "incident"),
            }
        }
        let view = self.app.on_block(&mut self.cache(), &produced);
        *self.snapshot.write().expect("snapshot lock") = produced.state.clone();
        *self.last_metering.lock().expect("metering lock") = (produced.height, produced.metering);
        if let Some(cp) = &produced.checkpoint {
            tracing::info!(
                seq = cp.seq,
                first = cp.first_height,
                last = cp.last_height,
                "checkpoint sealed"
            );
            self.sealed.notify_one();
        }
        let checkpoint = produced.checkpoint.as_ref().map(|c| c.seq);
        let _ = self.events.send(StreamEvent::Block {
            produced: Arc::new(produced),
            view,
            config_hash,
        });
        if let Some(seq) = checkpoint {
            let _ = self.events.send(StreamEvent::Checkpoint {
                seq,
                status: "sequenced",
            });
        }
    }
}

/// Opens the store and executor, starts the block and signer loops, and
/// returns the node and its router (the caller serves it).
pub async fn start<A: NodeApp>(
    app: A,
    cfg: &SequencerConfig,
) -> Result<(Arc<SequencerNode<A>>, Router)> {
    let (_, config_bytes, genesis_state) = crate::lane_toml::genesis(&app, &cfg.lane)?;
    let config_hash = sha256(&config_bytes);
    let lane_id = cfg.lane.lane_id();
    let (exec_cpu_limit, exec_mem_limit) = app.exec_limits(&config_bytes)?;
    let exec = if cfg.native {
        tracing::warn!("native executor: debugging only, not consensus (spec §14.5)");
        Executor::Native
    } else {
        let w = WasmExecutor::from_file(
            &cfg.engine_wasm,
            cfg.engine_wasm_hash,
            exec_cpu_limit,
            exec_mem_limit,
        )
        .context("loading the engine Wasm")?;
        // The Wasm genesis must give the same state as the native one.
        let (wasm_genesis, _) = w
            .genesis(&config_bytes)
            .map_err(|e| anyhow::anyhow!("wasm genesis: {e:?}"))?;
        anyhow::ensure!(
            wasm_genesis == genesis_state,
            "Wasm and native genesis differ"
        );
        Executor::Wasm(w)
    };
    if let Some(dir) = cfg.db.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut store = Store::open(&cfg.db, &lane_id, &config_hash, &genesis_state)
        .context("opening the store")?;
    // After a rotation (`[signers] epoch` raised), checkpoints signed by the
    // old set and not accepted yet are signed again by the new one: an admin
    // rotation makes older epochs invalid at once (spec §13.2).
    if let Some(s) = &cfg.signers {
        let seqs = store.unsign_before_epoch(s.epoch)?;
        if !seqs.is_empty() {
            tracing::warn!(
                epoch = s.epoch,
                ?seqs,
                "checkpoints signed under an older epoch go back for signatures"
            );
        }
    }
    let core_cfg = CoreConfig {
        ids: cfg.header_ids(),
        checkpoint_every_blocks: cfg.lane.node.checkpoint_every_blocks as u32,
        max_batch_bytes: cfg.lane.node.max_batch_bytes as usize,
        mempool_max: cfg.mempool_max,
        mempool_max_per_account: cfg.mempool_max_per_account,
    };
    let core = Core::open(app.clone(), exec, store, config_hash, core_cfg)?;
    tracing::info!(height = core.height(), state_hash = %hex(&core.state_hash()), lane = %cfg.lane.lane.name, template = A::TEMPLATE, "sequencer starting");
    if cfg.signers.is_none() {
        tracing::warn!("no [signers]: checkpoints are sealed but never signed");
    }
    let mut cache = A::Cache::default();
    let warm_started = std::time::Instant::now();
    match app.warm(
        &mut cache,
        &cfg.db,
        core.store(),
        &core.state(),
        core.height(),
    ) {
        Ok(()) => tracing::info!(
            ms = warm_started.elapsed().as_millis() as u64,
            "view cache warmed"
        ),
        Err(e) => tracing::warn!("view cache not warmed, it starts empty: {e:#}"),
    }
    let snapshot = RwLock::new(core.state());
    let (events, _) = broadcast::channel(1024);
    let node = Arc::new(SequencerNode {
        app,
        core: Mutex::new(core),
        snapshot,
        cache: Mutex::new(cache),
        events,
        sealed: Notify::new(),
        halted: Mutex::new(None),
        last_metering: Mutex::new((0, caravel_runtime::Metering::default())),
        lane_name: cfg.lane.lane.name.clone(),
        block_time_ms: cfg.lane.node.block_time_ms,
        checkpoint_every_blocks: cfg.lane.node.checkpoint_every_blocks,
        signers: cfg.signers.clone(),
        request_key: cfg.key.clone(),
        token: cfg.internal_token.clone(),
        production: cfg.production,
        identity: api::Identity {
            lane_id: cfg.lane.lane_id(),
            config_hash,
            settlement: cfg.settlement_contract,
            engine_wasm_hash: cfg.engine_wasm_hash,
            network_passphrase: cfg.network_passphrase.clone(),
        },
    });
    let router = router(node.clone(), &cfg.cors_origins);
    tokio::spawn(block_loop(node.clone(), config_hash));
    tokio::spawn(signer_loop(node.clone()));
    tokio::spawn(prune_loop(node.clone()));
    Ok((node, router))
}

/// `sequencer`: start and serve until Ctrl-C.
pub async fn run<A: NodeApp>(app: A, cfg: SequencerConfig) -> Result<()> {
    let addr: SocketAddr = cfg
        .listen
        .parse()
        .with_context(|| format!("listen address {:?}", cfg.listen))?;
    let (_node, router) = start(app, &cfg).await?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, "listening");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}

async fn block_loop<A: NodeApp>(app: Arc<SequencerNode<A>>, config_hash: [u8; 32]) {
    let mut tick = tokio::time::interval(Duration::from_millis(app.block_time_ms));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let a = app.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut core = a.core.lock().expect("core lock");
            let waiting = core
                .store()
                .checkpoints_with(CheckpointStatus::Signed)
                .map_or(0, |v| v.len());
            core.set_queue_len(waiting);
            core.produce_block(now_ms())
        })
        .await;
        match result {
            Ok(Ok(produced)) => app.publish(produced, config_hash),
            Ok(Err(e)) => {
                tracing::error!("block production halted: {e}");
                *app.halted.lock().expect("halted lock") = Some(e.to_string());
                return;
            }
            Err(e) => {
                tracing::error!("block task failed: {e}");
                *app.halted.lock().expect("halted lock") = Some(e.to_string());
                return;
            }
        }
    }
}

/// Prunes old snapshots and accepted batches (DEC-105), a bounded pass at a
/// time, between blocks.
async fn prune_loop<A: NodeApp>(app: Arc<SequencerNode<A>>) {
    loop {
        tokio::time::sleep(crate::PRUNE_EVERY).await;
        let a = app.clone();
        let r = tokio::task::spawn_blocking(move || {
            a.core
                .lock()
                .expect("core lock")
                .store_mut()
                .prune(crate::PRUNE_ROWS)
        })
        .await;
        crate::log_pruned(r);
    }
}

// --- Checkpoint signatures (spec §14.3 step 4) -------------------------------------

async fn signer_loop<A: NodeApp>(app: Arc<SequencerNode<A>>) {
    let Some(signers) = app.signers.clone() else {
        return;
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("http client");
    let mut warned: BTreeSet<u64> = BTreeSet::new();
    loop {
        tokio::select! {
            _ = app.sealed.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
        }
        // The lowest checkpoint still waiting for signatures, in seq order.
        let next = {
            let core = app.core.lock().expect("core lock");
            match core.store().checkpoints_with(CheckpointStatus::Sequenced) {
                Ok(rows) => rows.into_iter().next().map(|row| {
                    let check = core.precheck_row(&row);
                    (row, check)
                }),
                Err(e) => {
                    tracing::error!("reading checkpoints: {e}");
                    None
                }
            }
        };
        let Some((row, check)) = next else { continue };
        if let Err(reason) = check {
            if warned.insert(row.seq) {
                tracing::warn!(seq = row.seq, %reason, "checkpoint pre-check fails; not asking validators to sign yet");
            }
            continue;
        }
        match collect(
            &client,
            app.request_key.as_ref(),
            &signers,
            &row.header,
            &row.batch,
            &prior_signatures(&row),
        )
        .await
        {
            Ok(sigs) => {
                let json = serde_json::to_string(&sigs).expect("json");
                let stored = app.core.lock().expect("core lock").store_mut().set_signed(
                    row.seq,
                    signers.epoch,
                    &json,
                );
                match stored {
                    Ok(()) => {
                        tracing::info!(seq = row.seq, signatures = sigs.len(), "checkpoint signed");
                        let _ = app.events.send(StreamEvent::Checkpoint {
                            seq: row.seq,
                            status: "signed",
                        });
                        app.sealed.notify_one();
                    }
                    Err(e) => tracing::error!(seq = row.seq, "storing signatures: {e}"),
                }
            }
            Err(e) => tracing::warn!(seq = row.seq, "collecting signatures: {e}; retrying"),
        }
    }
}

/// Signatures a checkpoint already holds from before a rotation (the row
/// went back for signatures, see `Store::unsign_before_epoch`).
fn prior_signatures(row: &CheckpointRow) -> Vec<[u8; 64]> {
    api::sigs_value(row)
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s["signature"].as_str().and_then(unhex))
        .filter_map(|s| s.try_into().ok())
        .collect()
}

/// Asks the validators to sign and returns signatures sorted by signer index
/// once their weight reaches the threshold. Each one is verified first.
///
/// A signature in `prior` counts for every validator of this set whose key it
/// verifies under: ed25519 signatures are deterministic and the header does
/// not hold the epoch, so a validator that stays in the set after a rotation
/// is not asked again. It would refuse anyway once it has signed a later seq
/// (spec §15).
async fn collect(
    client: &reqwest::Client,
    request_key: Option<&ed25519_dalek::SigningKey>,
    signers: &Signers,
    header: &[u8],
    batch: &[u8],
    prior: &[[u8; 64]],
) -> Result<Vec<Value>> {
    let header_hash = sha256(header);
    let mut got: BTreeMap<u32, (u32, String)> = BTreeMap::new();
    for v in &signers.validators {
        if let Some(sig) = prior
            .iter()
            .find(|s| verify_strict(&v.key, &header_hash, s))
        {
            got.insert(v.index, (v.weight, hex(sig)));
        }
    }
    let body =
        serde_json::to_vec(&json!({ "header": hex(header), "batch": hex(batch) })).expect("json");
    // Signed once for every validator (DEC-095).
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let signed = request_key.map(|k| crate::sign_request::sign(k, now, &body));
    let requests = signers
        .validators
        .iter()
        .filter(|v| !got.contains_key(&v.index))
        .map(|v| {
            let mut req = client
                .post(format!("{}/v1/sign", v.url))
                .header("content-type", "application/json")
                .body(body.clone());
            for (name, value) in signed.iter().flatten() {
                req = req.header(*name, value);
            }
            let req = req.send();
            async move { (v, req.await) }
        });
    for (v, resp) in futures_util::future::join_all(requests).await {
        let resp = match resp {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                tracing::warn!(validator = %v.url, status = %r.status(), "validator refused to sign");
                continue;
            }
            Err(e) => {
                tracing::warn!(validator = %v.url, "validator unreachable: {e}");
                continue;
            }
        };
        let Ok(reply) = resp.json::<SignReply>().await else {
            continue;
        };
        let key_ok = views::parse_g(&reply.signer_key) == Some(v.key);
        let sig: Option<[u8; 64]> =
            caravel_runtime::sequencer::unhex(&reply.signature).and_then(|s| s.try_into().ok());
        match sig {
            Some(sig) if key_ok && verify_strict(&v.key, &header_hash, &sig) => {
                got.insert(v.index, (v.weight, reply.signature));
            }
            _ => tracing::warn!(validator = %v.url, "validator returned a bad signature"),
        }
    }
    let weight: u64 = got.values().map(|(w, _)| u64::from(*w)).sum();
    anyhow::ensure!(
        weight >= u64::from(signers.threshold),
        "signature weight {weight} below threshold {}",
        signers.threshold
    );
    Ok(got
        .into_iter()
        .map(|(i, (_, s))| json!({ "signer_index": i, "signature": s }))
        .collect())
}

#[derive(Deserialize)]
struct SignReply {
    signer_key: String,
    signature: String,
}

// --- Routes (spec §14.4) ---------------------------------------------------------------

pub fn router<A: NodeApp>(app: Arc<SequencerNode<A>>, cors_origins: &[String]) -> Router {
    let public = Router::new()
        .route("/v1/tx", post(post_tx::<A>))
        .route("/v1/status", get(status::<A>))
        .route("/v1/accounts/{account}", get(account::<A>))
        .route("/v1/blocks/{height}", get(block::<A>))
        .route("/v1/checkpoints/{seq}", get(checkpoint::<A>))
        .route("/v1/proofs/withdrawals", get(withdrawal_proofs::<A>))
        .route("/v1/proofs/escape", get(escape_proof::<A>))
        .route("/v1/stream", get(stream::<A>))
        .merge(app.app.routes());
    let mut internal = Router::new()
        .route("/internal/inbox", post(internal_inbox::<A>))
        .route("/internal/checkpoints/pending", get(internal_pending::<A>))
        .route(
            "/internal/checkpoints/{seq}/accepted",
            post(internal_accepted::<A>),
        );
    if let Some(feed) = app.app.feed_api() {
        internal = internal.route(
            &format!("/internal/{}", feed.route),
            post(internal_feed::<A>),
        );
    }
    let mut r = public.merge(internal).with_state(app);
    if !cors_origins.is_empty() {
        use tower_http::cors::{AllowOrigin, CorsLayer};
        let origins: Vec<axum::http::HeaderValue> =
            cors_origins.iter().filter_map(|o| o.parse().ok()).collect();
        r = r.layer(
            CorsLayer::new()
                .allow_origin(AllowOrigin::list(origins))
                .allow_methods([axum::http::Method::GET, axum::http::Method::POST])
                .allow_headers([axum::http::header::CONTENT_TYPE]),
        );
    }
    r
}

type AppState<A> = State<Arc<SequencerNode<A>>>;

#[derive(Deserialize)]
struct TxJson {
    tx: String,
}

async fn post_tx<A: NodeApp>(
    State(app): AppState<A>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    let raw = match headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        Some(ct) if ct.starts_with("application/octet-stream") => body.to_vec(),
        _ => {
            let j: TxJson = serde_json::from_slice(&body).map_err(|_| {
                ApiError::bad_request(
                    "DECODE",
                    "expected {\"tx\": \"<hex>\"} or application/octet-stream",
                )
            })?;
            api::unhex(&j.tx, "tx")?
        }
    };
    let result = app
        .core
        .lock()
        .expect("core lock")
        .submit_tx(&raw, now_ms());
    match result {
        Ok(hash) => Ok((
            StatusCode::ACCEPTED,
            Json(json!({ "tx_hash": hex(&hash), "status": "queued" })),
        )
            .into_response()),
        Err(r) => Err(ApiError::bad_request(r.code(), r.message())),
    }
}

async fn status<A: NodeApp>(State(app): AppState<A>) -> ApiResult {
    let core = app.core.lock().expect("core lock");
    let st = core.frame();
    let store = core.store();
    let last = |s| store.last_checkpoint_with(s).map_err(ApiError::internal);
    let seq = |v: Option<u64>| v.map(|s| s.to_string());
    let (mh, m) = *app.last_metering.lock().expect("metering lock");
    let metering = json!({ "height": mh.to_string(), "cpu_insns": m.cpu_insns.to_string(), "mem_bytes": m.mem_bytes.to_string(), "note": "soroban-env-host metering of the last step call, not network fees" });
    let mut body = json!({
        "lane_id": hex(&st.lane_id),
        "config_hash": hex(&core.config_hash()),
        "lane_name": app.lane_name,
        "template": A::TEMPLATE,
        "height": st.height.to_string(),
        "state_hash": hex(&core.state_hash()),
        "last_block_timestamp_ms": st.last_timestamp_ms.to_string(),
        "block_time_ms": app.block_time_ms,
        "checkpoint_every_blocks": app.checkpoint_every_blocks,
        "executor": if core.is_wasm() { "wasm" } else { "native" },
        "production": app.production,
        "halted": *app.halted.lock().expect("halted lock"),
        "checkpoints": {
            "sequenced": seq(store.last_checkpoint_seq().map_err(ApiError::internal)?),
            "signed": seq(last(CheckpointStatus::Signed)?.max(last(CheckpointStatus::Accepted)?)),
            "accepted": seq(last(CheckpointStatus::Accepted)?),
        },
        "inbox": {
            "reported": core.inbox_reported().to_string(),
            "reported_acc": hex(&core.inbox_reported_acc()),
            "processed": st.inbox_through.to_string(),
            "halted": core.inbox_halted(),
        },
        "mempool": core.mempool.len(),
        "host_metering": metering,
        "backpressure": core.backpressure(),
        "signers": app.signers.as_ref().map(|s| json!({
            "epoch": s.epoch.to_string(),
            "threshold": s.threshold,
            "validators": s.validators.iter().map(|v| json!({ "index": v.index, "url": v.url, "key": views::g_address(&v.key), "weight": v.weight })).collect::<Vec<_>>(),
        })),
    });
    app.identity.extend(&mut body);
    ok(body)
}

async fn account<A: NodeApp>(State(app): AppState<A>, Path(account): Path<String>) -> ApiResult {
    let key = api::parse_account(&account)?;
    let view = app
        .app
        .account(&app.state(), &key)
        .ok_or_else(|| ApiError::not_found("no lane account for this key"))?;
    let mut body = serde_json::to_value(view).map_err(ApiError::internal)?;
    // The nonce a new transaction takes: after the account's queued ones
    // (read under one lock, so a block can't land in between).
    let pending = {
        let core = app.core.lock().expect("core lock");
        let next = app.app.next_nonce(&core.state(), &key);
        next.map(|n| n.max(core.mempool.next_queued_nonce(&key).unwrap_or(0)))
    };
    if let (Some(o), Some(p)) = (body.as_object_mut(), pending) {
        o.insert("pending_nonce".into(), json!(p.to_string()));
    }
    ok(body)
}

async fn block<A: NodeApp>(State(app): AppState<A>, Path(height): Path<u64>) -> ApiResult {
    app.store(|s| api::block_json(&app.app, s, height))
}

async fn checkpoint<A: NodeApp>(State(app): AppState<A>, Path(seq): Path<u64>) -> ApiResult {
    app.store(|s| api::checkpoint_json(s, seq))
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
    app.store(|s| api::withdrawal_proofs(s, &key))
}

async fn escape_proof<A: NodeApp>(
    State(app): AppState<A>,
    Query(q): Query<AccountQuery>,
) -> ApiResult {
    let key = api::parse_account(&q.account)?;
    app.store(|s| {
        let accepted = s
            .last_checkpoint_with(CheckpointStatus::Accepted)
            .map_err(ApiError::internal)?;
        api::escape_proof(&app.app, s, &key, accepted)
    })
}

// --- Internal API (relayer → sequencer, bearer token) ------------------------------------

fn authorized<A: NodeApp>(app: &SequencerNode<A>, headers: &HeaderMap) -> Result<(), ApiError> {
    if api::token_ok(headers, &app.token) {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED",
            "bearer token required",
        ))
    }
}

#[derive(Deserialize)]
struct InboxJson {
    index: String,
    msg_hex: String,
    acc_after_hex: String,
}

async fn internal_inbox<A: NodeApp>(
    State(app): AppState<A>,
    headers: HeaderMap,
    Json(j): Json<InboxJson>,
) -> ApiResult {
    authorized(&app, &headers)?;
    let msg = InboxMsgV1::decode(&api::unhex(&j.msg_hex, "msg_hex")?)
        .map_err(|_| ApiError::bad_request("DECODE", "msg_hex is not an InboxMsgV1"))?;
    if j.index != msg.index.to_string() {
        return Err(ApiError::bad_request(
            "INDEX",
            "index does not match the message",
        ));
    }
    let acc: [u8; 32] = api::unhex(&j.acc_after_hex, "acc_after_hex")?
        .try_into()
        .map_err(|_| ApiError::bad_request("BAD_HEX", "acc_after_hex is not 32 bytes"))?;
    let report = app
        .core
        .lock()
        .expect("core lock")
        .report_inbox(msg, acc)
        .map_err(ApiError::internal)?;
    match report {
        InboxReport::Added => ok(json!({ "status": "added" })),
        InboxReport::AlreadyKnown => ok(json!({ "status": "known" })),
        InboxReport::Gap { expected } => Ok((
            StatusCode::CONFLICT,
            Json(json!({ "error": format!("send index {expected} first"), "code": "INBOX_GAP", "expected": expected.to_string() })),
        )
            .into_response()),
        InboxReport::Mismatch => {
            tracing::error!(
                index = msg.index,
                "inbox acc_after does not match the sequencer's fold; inbox inclusion halted"
            );
            Err(ApiError::new(
                StatusCode::CONFLICT,
                "INBOX_MISMATCH",
                "acc_after does not match; inbox inclusion is halted",
            ))
        }
    }
}

#[derive(Deserialize)]
struct FeedJson {
    update: String,
}

/// `POST /internal/{feed}`: keeps the newest admitted update per feed slot.
async fn internal_feed<A: NodeApp>(
    State(app): AppState<A>,
    headers: HeaderMap,
    Json(j): Json<FeedJson>,
) -> ApiResult {
    authorized(&app, &headers)?;
    let feed = app
        .app
        .feed_api()
        .ok_or_else(|| ApiError::not_found("this lane has no feeds"))?;
    let bytes = api::unhex(&j.update, "update")?;
    if app.app.decode_feed(&bytes).is_none() {
        return Err(ApiError::bad_request("DECODE", feed.not_decoded));
    }
    let accepted = app
        .core
        .lock()
        .expect("core lock")
        .report_feed(&bytes)
        .map_err(ApiError::internal)?;
    if accepted {
        ok(json!({ "status": "queued" }))
    } else {
        Err(ApiError::bad_request(feed.refused_code, feed.refused))
    }
}

async fn internal_pending<A: NodeApp>(State(app): AppState<A>, headers: HeaderMap) -> ApiResult {
    authorized(&app, &headers)?;
    let row = app
        .store(|s| s.checkpoints_with(CheckpointStatus::Signed))
        .map_err(ApiError::internal)?
        .into_iter()
        .next();
    match row {
        None => Ok(StatusCode::NO_CONTENT.into_response()),
        Some(row) => ok(json!({
            "seq": row.seq.to_string(),
            "header": hex(&row.header),
            "batch": hex(&row.batch),
            "epoch": row.epoch.map(|e| e.to_string()),
            "sigs": api::sigs_value(&row),
        })),
    }
}

#[derive(Deserialize)]
struct AcceptedJson {
    stellar_tx_hash: String,
    ledger: u32,
}

async fn internal_accepted<A: NodeApp>(
    State(app): AppState<A>,
    headers: HeaderMap,
    Path(seq): Path<u64>,
    Json(j): Json<AcceptedJson>,
) -> ApiResult {
    authorized(&app, &headers)?;
    let result = app
        .core
        .lock()
        .expect("core lock")
        .store_mut()
        .set_accepted(seq, &j.stellar_tx_hash, j.ledger);
    match result {
        Ok(()) => {
            tracing::info!(seq, tx = %j.stellar_tx_hash, ledger = j.ledger, "checkpoint accepted on Stellar");
            let _ = app.events.send(StreamEvent::Checkpoint {
                seq,
                status: "accepted",
            });
            ok(json!({ "status": "accepted" }))
        }
        Err(caravel_runtime::store::StoreError::Conflict(why)) => {
            Err(ApiError::new(StatusCode::CONFLICT, "CONFLICT", why))
        }
        Err(e) => Err(ApiError::internal(e)),
    }
}

// --- WebSocket stream ----------------------------------------------------------------------

/// `{blocks, account}` plus the app's fields.
#[derive(Deserialize, Default, Clone)]
#[serde(bound = "S: DeserializeOwned")]
struct Subscription<S> {
    #[serde(default)]
    blocks: bool,
    account: Option<String>,
    #[serde(flatten)]
    app: S,
}

async fn stream<A: NodeApp>(State(app): AppState<A>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| serve_stream(app, socket))
}

async fn serve_stream<A: NodeApp>(app: Arc<SequencerNode<A>>, mut socket: WebSocket) {
    let mut rx = app.events.subscribe();
    let mut sub = Subscription::<A::Subscription>::default();
    let mut account: Option<[u8; 32]> = None;
    loop {
        tokio::select! {
            msg = socket.recv() => match msg {
                Some(Ok(Message::Text(t))) => match serde_json::from_str::<Subscription<A::Subscription>>(t.as_str()) {
                    Ok(s) => {
                        account = s.account.as_deref().and_then(views::parse_g);
                        sub = s;
                        let _ = socket.send(Message::Text(json!({ "type": "subscribed" }).to_string().into())).await;
                    }
                    Err(_) => {
                        let _ = socket.send(Message::Text(json!({ "type": "error", "error": app.app.subscription_hint() }).to_string().into())).await;
                    }
                },
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                _ => {}
            },
            ev = rx.recv() => {
                let ev = match ev {
                    Ok(ev) => ev,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        let _ = socket.send(Message::Text(json!({ "type": "lagged", "missed": n }).to_string().into())).await;
                        continue;
                    }
                    Err(_) => return,
                };
                for m in messages(&app.app, &ev, &sub, account.as_ref()) {
                    if socket.send(Message::Text(m.to_string().into())).await.is_err() {
                        return;
                    }
                }
            }
        }
    }
}

/// The stream messages one event gives a subscriber: `block` and
/// `checkpoint`, the account's `receipt`s, and the app's (perps: `fill`,
/// `book` and `account`), in the order the app gives.
fn messages<A: NodeApp>(
    app: &A,
    ev: &StreamEvent<A>,
    sub: &Subscription<A::Subscription>,
    account: Option<&[u8; 32]>,
) -> Vec<Value> {
    let mut out = Vec::new();
    match ev {
        StreamEvent::Checkpoint { seq, status } => {
            out.push(json!({ "type": "checkpoint", "seq": seq.to_string(), "status": status }))
        }
        StreamEvent::Block {
            produced,
            view,
            config_hash,
        } => {
            let p = produced.as_ref();
            let input = BlockInputV1::decode(&p.record.input).ok();
            if sub.blocks {
                out.push(json!({
                    "type": "block",
                    "height": p.height.to_string(),
                    "timestamp_ms": input.as_ref().map(|b| b.timestamp_ms.to_string()),
                    "entries": input.as_ref().map_or(0, |b| b.entries.len()),
                    "checkpoint_end": input.as_ref().is_some_and(|b| b.checkpoint_end),
                    "state_hash": hex(&p.record.state_hash_after),
                }));
            }
            let mut receipts = Vec::new();
            if let (Some(key), Some(input)) = (account, &input) {
                let events = app.render_events(&p.receipts_bytes).unwrap_or_default();
                for (i, rc) in p.receipts.receipts.iter().enumerate() {
                    if let Some(Entry::User(tx)) = input.entries.get(rc.entry_index as usize) {
                        if tx.account == *key {
                            receipts.push(json!({
                                "type": "receipt",
                                "height": p.height.to_string(),
                                "tx_hash": hex(&tx_hash(tx, config_hash)),
                                "nonce": tx.nonce.to_string(),
                                "code": rc.code,
                                "events": events.get(i).cloned().unwrap_or_default(),
                            }));
                        }
                    }
                }
            }
            out.extend(app.stream(p, view, &sub.app, account, receipts));
        }
    }
    out
}
