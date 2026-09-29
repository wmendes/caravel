//! `caravel-node witness`: the input for the T-012 witness transaction, one
//! `step` call to the engine contract on Stellar, and the exact bytes the
//! executor returns for it. On Stellar the transaction's return value must be
//! byte-equal (spec §20.1 T-012): the same Wasm gives the same output on
//! the network and on every node.

use anyhow::{anyhow, Result};
use caravel_lane::checkpoint::sha256;
use caravel_lane::sequencer::hex;
use caravel_lane::WasmExecutor;
use caravel_types::block::{BlockInputV1, Entry};
use caravel_types::inbox::{InboxKind, InboxMsgV1};
use caravel_types::step::StepEnvelope;
use serde_json::{json, Value};

use crate::lane_toml::LaneFile;

/// Genesis state plus one `CHECKPOINT_END` block with a deposit, so the call
/// creates an account and computes a commitment.
pub fn witness(lane: &LaneFile, exec: &WasmExecutor, depositor: [u8; 32], timestamp_ms: u64) -> Result<Value> {
    let (_, config_bytes, _) = crate::lane_toml::genesis(lane)?;
    let (state, _) = exec.genesis(&config_bytes).map_err(|e| anyhow!("genesis: {e:?}"))?;
    let block = BlockInputV1 {
        lane_id: lane.lane_id(),
        height: 1,
        timestamp_ms,
        prev_block_hash: [0; 32],
        checkpoint_end: true,
        entries: vec![Entry::Inbox(InboxMsgV1 { kind: InboxKind::Deposit, index: 0, lane_account: depositor, amount: 100 * 10_000_000, enqueued_at: timestamp_ms / 1000 })],
    }
    .encode()
    .map_err(|_| anyhow!("block encoding"))?;
    let (out, metering) = exec.step(&state, &block).map_err(|e| anyhow!("step: {e:?}"))?;
    let expected = StepEnvelope { state: out.state, receipts: out.receipts }.encode().map_err(|_| anyhow!("envelope"))?;
    Ok(json!({
        "state_hex": hex(&state),
        "block_hex": hex(&block),
        "expected_hex": hex(&expected),
        "expected_sha256": hex(&sha256(&expected)),
        "state_bytes": state.len(),
        "block_bytes": block.len(),
        "host_metering": { "cpu_insns": metering.cpu_insns, "mem_bytes": metering.mem_bytes },
    }))
}
