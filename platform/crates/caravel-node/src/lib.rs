//! The Caravel node library, for any app (M0.5, DEC-053): lane files, the
//! sequencer (T-007), the validator (T-008), the replay verifier (T-010) and
//! the commands every app's node binary has. An app implements
//! [`app::NodeApp`] and its binary calls [`cli::run`].

pub mod api;
pub mod app;
pub mod check;
pub mod cli;
pub mod lane_toml;
pub mod node_config;
pub mod plugin;
pub mod replay;
pub mod scval;
pub mod sequencer;
pub mod sign_request;
pub mod stellar_rpc;
pub mod validator;
pub mod witness;

pub use app::{FeedApi, NodeApp};

/// About how often a node writes its head, the full state (F-07): the blocks
/// since are re-executed when it starts again.
pub(crate) const HEAD_EVERY_MS: u64 = 10_000;

/// The head cadence in blocks for `block_time_ms`.
pub(crate) fn head_every(block_time_ms: u64) -> u64 {
    (HEAD_EVERY_MS / block_time_ms.max(1)).max(1)
}

/// How often a node prunes its store (DEC-105).
pub(crate) const PRUNE_EVERY: std::time::Duration = std::time::Duration::from_secs(30);
/// Rows of each kind one prune pass may drop.
pub(crate) const PRUNE_ROWS: usize = 200;

pub(crate) fn log_pruned(
    r: Result<
        caravel_runtime::store::Result<caravel_runtime::store::Pruned>,
        tokio::task::JoinError,
    >,
) {
    match r {
        Ok(Ok(p)) if p.snapshots + p.batches > 0 => {
            tracing::info!(snapshots = p.snapshots, batches = p.batches, "store pruned")
        }
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::warn!("pruning the store: {e}"),
        Err(e) => tracing::warn!("prune task: {e}"),
    }
}
