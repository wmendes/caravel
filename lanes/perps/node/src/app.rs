//! Caravel Perps as a `LaneApp` (M0.5, P-05): what the platform runtime needs
//! from the perps engine, answered with the frozen perps crates. Every rule
//! here is the M0 rule the runtime used to hard-code, so lane #1 behaves byte
//! for byte as before (the golden trace checks it).

use std::collections::BTreeMap;

use caravel_core::checkpoint::CheckpointHeaderV1;
use caravel_core::tx::TxEnvelopeV1;
use caravel_perps::native::{DiagnosticCrypto, NativeCrypto};
use caravel_runtime::checkpoint::{self, sha256, Leaf, LeafError};
use caravel_runtime::mempool::verify_strict;
use caravel_runtime::{FeedUpdate, LaneApp, LaneLimits, NativeFatal, StepOutput};
use caravel_types::oracle::OracleUpdateV1;
use caravel_types::receipts::Receipts;
use caravel_types::state::StateV1;
use caravel_types::tx::LaneTxV1;

/// Oracle entries in a live block must be within this of the validator's
/// clock (spec §15).
pub const ORACLE_WINDOW_MS: u64 = 60_000;

/// The perps app. It holds nothing: its config is in the state.
#[derive(Clone, Copy, Debug, Default)]
pub struct PerpsApp;

impl LaneApp for PerpsApp {
    type State = StateV1;

    fn decode_state(&self, bytes: &[u8]) -> Option<StateV1> {
        StateV1::decode(bytes).ok()
    }

    fn limits(&self, st: &StateV1) -> LaneLimits {
        LaneLimits {
            max_block_bytes: st.config.max_block_bytes,
            max_entries_per_block: st.config.max_entries_per_block,
            max_txs_per_account_per_block: st.config.max_txs_per_account_per_block,
            max_pending_withdrawals: st.config.max_pending_withdrawals,
        }
    }

    fn next_nonces(&self, st: &StateV1) -> BTreeMap<[u8; 32], u64> {
        st.accounts.iter().map(|a| (a.key, a.next_nonce)).collect()
    }

    fn next_nonce(&self, st: &StateV1, key: &[u8; 32]) -> Option<u64> {
        st.accounts
            .iter()
            .find(|a| a.key == *key)
            .map(|a| a.next_nonce)
    }

    fn pending_withdrawals(&self, st: &StateV1) -> usize {
        st.pending.len()
    }

    fn tx_decodes(&self, tx: &TxEnvelopeV1) -> bool {
        tx.encode()
            .is_some_and(|bytes| LaneTxV1::decode(&bytes).is_ok())
    }

    fn native_genesis(&self, config: &[u8]) -> Result<Vec<u8>, NativeFatal> {
        caravel_perps::genesis(config, &NativeCrypto).map_err(|f| NativeFatal {
            code: f.code,
            entry_index: f.entry_index,
        })
    }

    fn native_step(&self, state: &[u8], block: &[u8]) -> Result<StepOutput, NativeFatal> {
        caravel_perps::step(state, block, &DiagnosticCrypto)
            .map(|o| StepOutput {
                state: o.state,
                receipts: o.receipts,
            })
            .map_err(|f| NativeFatal {
                code: f.code,
                entry_index: f.entry_index,
            })
    }

    fn receipts_decode(&self, receipts: &[u8]) -> bool {
        Receipts::decode(receipts).is_ok()
    }

    fn render_events(&self, receipts: &[u8]) -> Option<Vec<Vec<String>>> {
        let r = Receipts::decode(receipts).ok()?;
        Some(
            r.receipts
                .iter()
                .map(|rc| rc.events.iter().map(|e| format!("{e:?}")).collect())
                .collect(),
        )
    }

    fn escape_leaves(&self, st: &StateV1) -> Option<Vec<([u8; 32], i128)>> {
        st.accounts
            .iter()
            .enumerate()
            .map(|(j, a)| {
                caravel_perps::margin::equity(st, j)
                    .ok()
                    .map(|e| (a.key, e.max(0)))
            })
            .collect()
    }

    fn feed_label(&self) -> &'static str {
        "oracle"
    }

    fn decode_feed(&self, bytes: &[u8]) -> Option<FeedUpdate> {
        let u = OracleUpdateV1::decode(bytes).ok()?;
        Some(FeedUpdate {
            slot: u32::from(u.market_id),
            publish_time_ms: u.publish_time_ms,
            bytes: u.encode().to_vec(),
        })
    }

    /// The engine's fatal oracle checks (spec §11.5): a configured key and a
    /// valid signature.
    fn feed_admissible(&self, st: &StateV1, update: &FeedUpdate) -> bool {
        let Ok(u) = OracleUpdateV1::decode(&update.bytes) else {
            return false;
        };
        st.config.oracle_keys.binary_search(&u.oracle_key).is_ok()
            && verify_strict(
                &u.oracle_key,
                &sha256(&u.signing_preimage(&st.lane_id)),
                &u.signature,
            )
    }

    /// The latest update per market goes in when it is for a configured
    /// market, newer than the state's price, and not too far ahead (spec §14.1).
    fn feed_include(&self, st: &StateV1, update: &FeedUpdate, block_timestamp_ms: u64) -> bool {
        let Ok(u) = OracleUpdateV1::decode(&update.bytes) else {
            return false;
        };
        let Some(m) = st
            .config
            .markets
            .iter()
            .position(|p| p.market_id == u.market_id)
        else {
            return false;
        };
        u.publish_time_ms > st.markets[m].oracle_time_ms
            && u.publish_time_ms
                <= block_timestamp_ms.saturating_add(st.config.oracle_max_future_ms)
    }

    fn feed_live_flag(&self, update: &FeedUpdate, now_ms: u64) -> Option<String> {
        let u = OracleUpdateV1::decode(&update.bytes).ok()?;
        (u.publish_time_ms.abs_diff(now_ms) > ORACLE_WINDOW_MS).then(|| {
            format!(
                "oracle update for market {} published at {} is more than {ORACLE_WINDOW_MS} ms from {now_ms}",
                u.market_id, u.publish_time_ms
            )
        })
    }
}

/// A checkpoint's account leaves for a perps state, checked against the header.
pub fn account_leaves(header: &CheckpointHeaderV1, st: &StateV1) -> Result<Vec<Leaf>, LeafError> {
    let escape = PerpsApp.escape_leaves(st).ok_or(LeafError::Decode)?;
    checkpoint::account_leaves(header, &escape)
}
