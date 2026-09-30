//! What the runtime needs from a lane's app (M0.5, P-05).
//!
//! Consensus is always the app's engine Wasm (DEC-002). The runtime reads the
//! state frame, blocks, receipts and the commitment itself (`caravel-core`,
//! DEC-052). This trait covers the rest, the parts that need the app:
//! - decoding its state;
//! - its limits and account nonces, to build blocks;
//! - its strict transaction check, for the mempool;
//! - a native run of its engine that names the fatal entry, to quarantine it;
//! - its accounts' escape equity, for proofs;
//! - its feeds, optionally (the perps oracle is one);
//! - text for views.
//!
//! Each app implements it natively, and its node binary links it (DEC-053).

use std::collections::BTreeMap;

use caravel_core::tx::TxEnvelopeV1;

/// The execution output of one `step`: new state bytes and receipts bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StepOutput {
    pub state: Vec<u8>,
    pub receipts: Vec<u8>,
}

/// A fatal from the app's native engine, with the entry that caused it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeFatal {
    pub code: u16,
    /// `caravel_core::codes::fatal::BLOCK_LEVEL` when no entry is at fault.
    pub entry_index: u32,
}

/// The limits the runtime builds blocks within, from the app's genesis config.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LaneLimits {
    pub max_block_bytes: u32,
    pub max_entries_per_block: u32,
    pub max_txs_per_account_per_block: u16,
    pub max_pending_withdrawals: u32,
}

/// A feed update the sequencer holds, the latest per slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedUpdate {
    /// Feeds with the same slot replace each other (perps: the market id).
    pub slot: u32,
    pub publish_time_ms: u64,
    /// The entry payload, exactly as it goes in a block.
    pub bytes: Vec<u8>,
}

pub trait LaneApp: Send + Sync + 'static {
    /// The app's decoded state.
    type State: Send + Sync + 'static;

    /// Decodes the state bytes the engine returned. `None` if they do not decode.
    fn decode_state(&self, bytes: &[u8]) -> Option<Self::State>;

    fn limits(&self, state: &Self::State) -> LaneLimits;

    /// `next_nonce` of every account.
    fn next_nonces(&self, state: &Self::State) -> BTreeMap<[u8; 32], u64>;

    /// `next_nonce` of one account, or `None` if it does not exist.
    fn next_nonce(&self, state: &Self::State, key: &[u8; 32]) -> Option<u64>;

    /// The pending withdrawal queue's length.
    fn pending_withdrawals(&self, state: &Self::State) -> usize;

    /// The nonce an account that a deposit creates starts at (M0: the block
    /// timestamp, spec §11.4).
    fn new_account_nonce(&self, block_timestamp_ms: u64) -> u64 {
        block_timestamp_ms
    }

    /// Whether the engine would decode this transaction: its kind exists and
    /// its body is exactly right. The mempool refuses anything else.
    fn tx_decodes(&self, tx: &TxEnvelopeV1) -> bool;

    /// The engine's genesis, run natively (startup checks and tests).
    fn native_genesis(&self, config: &[u8]) -> Result<Vec<u8>, NativeFatal>;

    /// The engine's `step`, run natively with a crypto that reports a bad
    /// signature as a fatal instead of trapping. The sequencer uses it to find
    /// the entry to quarantine, which the Wasm path does not report.
    fn native_step(&self, state: &[u8], block: &[u8]) -> Result<StepOutput, NativeFatal>;

    /// Whether the engine's receipts decode with the app's own strict rules.
    fn receipts_decode(&self, receipts: &[u8]) -> bool;

    /// Each receipt's events as text, in receipt order (block views).
    fn render_events(&self, receipts: &[u8]) -> Option<Vec<Vec<String>>>;

    /// `(key, escape equity ≥ 0)` for every account, in account order: the
    /// leaves the engine committed as `accounts_root`.
    fn escape_leaves(&self, state: &Self::State) -> Option<Vec<([u8; 32], i128)>>;

    /// What views call a feed entry (perps: "oracle").
    fn feed_label(&self) -> &'static str {
        "feed"
    }

    /// Decodes a feed payload. Apps without feeds keep the default: every feed
    /// entry is refused (the engine treats one as a bad entry).
    fn decode_feed(&self, _bytes: &[u8]) -> Option<FeedUpdate> {
        None
    }

    /// The engine's fatal feed checks (a configured key and a valid signature),
    /// so no feed that would fail the block reaches one.
    fn feed_admissible(&self, _state: &Self::State, _update: &FeedUpdate) -> bool {
        false
    }

    /// Whether to put this update in the next block, with this timestamp.
    fn feed_include(
        &self,
        _state: &Self::State,
        _update: &FeedUpdate,
        _block_timestamp_ms: u64,
    ) -> bool {
        false
    }

    /// A validator's live-check flag for a feed entry (spec §15), if any.
    fn feed_live_flag(&self, _update: &FeedUpdate, _now_ms: u64) -> Option<String> {
        None
    }
}
