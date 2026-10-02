//! JSON helpers and the read endpoints that the sequencer and validators
//! both serve from their own stores (spec §14.4, §15): blocks, checkpoints
//! and proofs. The validators' copies are the independent proof source for
//! the escape hatch.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use caravel_core::checkpoint::CheckpointHeaderV1;
use caravel_runtime::checkpoint;
use caravel_runtime::sequencer::{hex, parse_leaves};
use caravel_runtime::store::{CheckpointRow, CheckpointStatus, Store};
use caravel_runtime::views;
use caravel_runtime::LaneApp;
use serde::Serialize;
use serde_json::{json, Value};

pub type ApiResult = Result<Response, ApiError>;

/// `{error, code}` with a status.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub error: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, error: impl Into<String>) -> Self {
        Self {
            status,
            code,
            error: error.into(),
        }
    }

    pub fn bad_request(code: &'static str, error: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, error)
    }

    pub fn not_found(what: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "NOT_FOUND", what)
    }

    pub fn internal(e: impl std::fmt::Display) -> Self {
        tracing::error!("internal error: {e}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL",
            "internal error",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "error": self.error, "code": self.code })),
        )
            .into_response()
    }
}

pub fn ok<T: Serialize>(v: T) -> ApiResult {
    Ok(Json(v).into_response())
}

/// What a node runs, in its `/v1/status`: the deploy tool checks a host
/// against the lane file and the chain with it.
#[derive(Clone, Debug)]
pub struct Identity {
    pub lane_id: [u8; 32],
    pub config_hash: [u8; 32],
    pub settlement: [u8; 32],
    pub engine_wasm_hash: [u8; 32],
    pub network_passphrase: String,
}

impl Identity {
    /// Adds the identity fields to a status object.
    pub fn extend(&self, status: &mut Value) {
        let fields = json!({
            "lane_id": hex(&self.lane_id),
            "config_hash": hex(&self.config_hash),
            "settlement": stellar_strkey::Contract(self.settlement).to_string().as_str(),
            "engine_wasm_sha256": hex(&self.engine_wasm_hash),
            "network_passphrase": self.network_passphrase,
            "release": release(),
        });
        if let (Value::Object(s), Value::Object(f)) = (status, fields) {
            s.extend(f);
        }
    }
}

/// This binary: its version, and the commit CI built it from (`CARAVEL_COMMIT`
/// at build time; `null` for a local build).
pub fn release() -> Value {
    json!({ "version": env!("CARGO_PKG_VERSION"), "commit": option_env!("CARAVEL_COMMIT").filter(|c| !c.is_empty()) })
}

pub fn parse_account(s: &str) -> Result<[u8; 32], ApiError> {
    views::parse_g(s).ok_or_else(|| ApiError::bad_request("BAD_ACCOUNT", "expected a G... account"))
}

pub fn unhex(s: &str, what: &'static str) -> Result<Vec<u8>, ApiError> {
    caravel_runtime::sequencer::unhex(s.trim_start_matches("0x"))
        .ok_or_else(|| ApiError::bad_request("BAD_HEX", format!("{what} is not hex")))
}

pub fn block_json<A: LaneApp>(app: &A, store: &Store, height: u64) -> ApiResult {
    let (record, receipts) = store
        .block(height)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("no block {height}")))?;
    ok(views::block(app, &record, &receipts)
        .ok_or_else(|| ApiError::internal("stored block does not decode"))?)
}

/// Signatures as stored: `[{signer_index, signature}]`.
pub fn sigs_value(row: &CheckpointRow) -> Value {
    row.sigs
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Array(vec![]))
}

pub fn checkpoint_value(row: &CheckpointRow) -> Result<Value, ApiError> {
    let header = CheckpointHeaderV1::decode(&row.header)
        .map_err(|_| ApiError::internal("stored header does not decode"))?;
    Ok(json!({
        "seq": row.seq.to_string(),
        "status": row.status.as_str(),
        "header_hex": hex(&row.header),
        "header_hash": hex(&checkpoint::sha256(&row.header)),
        "header": views::header(&header),
        "batch_hash": hex(&header.batch_hash),
        "batch_bytes": row.batch.len(),
        "first_block_height": row.first_height.to_string(),
        "last_block_height": row.last_height.to_string(),
        "epoch": row.epoch.map(|e| e.to_string()),
        "signatures": sigs_value(row),
        "stellar_tx_hash": row.stellar_tx_hash,
        "stellar_ledger": row.stellar_ledger,
    }))
}

pub fn checkpoint_json(store: &Store, seq: u64) -> ApiResult {
    let row = store
        .checkpoint(seq)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("no checkpoint {seq}")))?;
    ok(checkpoint_value(&row)?)
}

/// Withdrawal leaves of `account` in checkpoints accepted on Stellar, with
/// proofs. Whether one was already claimed is on Stellar (`is_claimed`).
pub fn withdrawal_proofs(store: &Store, account: &[u8; 32]) -> ApiResult {
    let mut out = Vec::new();
    for row in store
        .checkpoints_with(CheckpointStatus::Accepted)
        .map_err(ApiError::internal)?
    {
        let header =
            CheckpointHeaderV1::decode(&row.header).map_err(|_| ApiError::internal("header"))?;
        if header.withdrawal_count == 0 {
            continue;
        }
        let leaves = parse_leaves(&row.withdrawals)
            .ok_or_else(|| ApiError::internal("withdrawal leaves"))?;
        if !leaves.iter().any(|l| l.key == *account) {
            continue;
        }
        let hashes = checkpoint::withdrawal_hashes(&header, &leaves);
        out.extend(views::proofs_for(&header, &leaves, &hashes, account));
    }
    ok(json!({ "account": views::g_address(account), "withdrawals": out }))
}

/// The account's escape leaf in the last checkpoint accepted on Stellar.
/// `accepted` is that checkpoint's seq as this node knows it; the contract's
/// `last_checkpoint()` is the authority.
pub fn escape_proof<A: LaneApp>(
    app: &A,
    store: &Store,
    account: &[u8; 32],
    accepted: Option<u64>,
) -> ApiResult {
    let seq =
        accepted.ok_or_else(|| ApiError::not_found("no checkpoint accepted on Stellar yet"))?;
    let (header, leaves, hashes) = escape_tree(app, store, seq)?;
    let proof = views::proofs_for(&header, &leaves, &hashes, account)
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::not_found("the account has no leaf in that checkpoint"))?;
    ok(
        json!({ "seq": proof.seq, "index": proof.index, "account": proof.account, "equity": proof.amount, "proof": proof.proof }),
    )
}

/// Everything a lane's users can exit with, with proofs (`export-proofs`):
/// every escape leaf of checkpoint `seq`, and every withdrawal leaf of every
/// accepted checkpoint. Whether a withdrawal was already claimed is on
/// Stellar (`is_claimed`).
pub fn exit_proofs<A: LaneApp>(app: &A, store: &Store, seq: u64) -> Result<Value, ApiError> {
    let (header, leaves, hashes) = escape_tree(app, store, seq)?;
    let escape: Vec<Value> = views::all_proofs(&header, &leaves, &hashes)
        .into_iter()
        .map(|p| json!({ "index": p.index, "account": p.account, "equity": p.amount, "proof": p.proof }))
        .collect();
    let mut withdrawals = Vec::new();
    for row in store
        .checkpoints_with(CheckpointStatus::Accepted)
        .map_err(ApiError::internal)?
    {
        let h =
            CheckpointHeaderV1::decode(&row.header).map_err(|_| ApiError::internal("header"))?;
        if h.withdrawal_count == 0 {
            continue;
        }
        let leaves = parse_leaves(&row.withdrawals)
            .ok_or_else(|| ApiError::internal("withdrawal leaves"))?;
        let hashes = checkpoint::withdrawal_hashes(&h, &leaves);
        withdrawals.extend(views::all_proofs(&h, &leaves, &hashes));
    }
    Ok(json!({
        "seq": seq.to_string(),
        "header_hash": hex(&checkpoint::sha256(&header.encode())),
        "escape_total": header.escape_total.to_string(),
        "escape": escape,
        "withdrawals": withdrawals,
    }))
}

/// A checkpoint's header, its escape leaves and their hashes.
type EscapeTree = (CheckpointHeaderV1, Vec<checkpoint::Leaf>, Vec<[u8; 32]>);

/// Checkpoint `seq`'s escape tree.
fn escape_tree<A: LaneApp>(app: &A, store: &Store, seq: u64) -> Result<EscapeTree, ApiError> {
    let row = store
        .checkpoint(seq)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("no checkpoint {seq}")))?;
    let header =
        CheckpointHeaderV1::decode(&row.header).map_err(|_| ApiError::internal("header"))?;
    let (_, state) = store
        .snapshot(seq)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal("missing snapshot"))?;
    let escape = app
        .decode_state(&state)
        .and_then(|st| app.escape_leaves(&st))
        .ok_or_else(|| ApiError::internal("snapshot"))?;
    let leaves = checkpoint::account_leaves(&header, &escape)
        .map_err(|_| ApiError::internal("account leaves do not match the header"))?;
    let hashes = checkpoint::account_hashes(&header, &leaves);
    Ok((header, leaves, hashes))
}

/// Constant-time comparison for the bearer token.
pub fn token_ok(headers: &axum::http::HeaderMap, token: &str) -> bool {
    let Some(v) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let Some(given) = v.strip_prefix("Bearer ") else {
        return false;
    };
    let (a, b) = (given.as_bytes(), token.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
