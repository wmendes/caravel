//! `caravel-node check-store`: re-executes every block in a node's store from
//! genesis through the engine Wasm and checks each `state_hash_after`, the
//! stored head state, and every stored checkpoint header (rebuilt byte for
//! byte). Operators use it after a restart or on a copy of another node's store.

use std::path::Path;

use anyhow::{bail, Context, Result};
use caravel_core::block::{BlockInputV1, BlockRecordV1};
use caravel_runtime::checkpoint::{self, sha256, HeaderIds};
use caravel_runtime::sequencer::hex;
use caravel_runtime::store::Store;
use caravel_runtime::WasmExecutor;
use serde::Serialize;

use crate::app::NodeApp;
use crate::lane_toml::LaneFile;

#[derive(Serialize, Debug)]
pub struct CheckReport {
    pub ok: bool,
    /// Where re-execution started: 0 (genesis), or a validator's oldest
    /// kept snapshot once its older blocks are dropped (F-08).
    pub from_height: u64,
    pub height: u64,
    pub final_state_hash: String,
    pub checkpoints_checked: u64,
    pub host_cpu_insns_max: u64,
}

pub fn check_store<A: NodeApp>(
    app: &A,
    lane: &LaneFile,
    db: &Path,
    exec: &WasmExecutor,
    ids: &HeaderIds,
) -> Result<CheckReport> {
    if !db.exists() {
        bail!("no store at {}", db.display());
    }
    let (_, config_bytes, genesis_state) = crate::lane_toml::genesis(app, lane)?;
    let config_hash = sha256(&config_bytes);
    let store = Store::open(db, &lane.lane_id(), &config_hash, &genesis_state)
        .context("opening the store")?;
    // The persisted head may be behind the last block (F-07): every block to
    // the tip is re-executed, and the head checked on the way.
    let (head, head_state) = store.head()?;
    let tip = store.tip()?;
    // A validator drops old blocks (F-08): start from the snapshot its
    // first block follows.
    let (mut height, mut state, mut prev_header) = match store.first_block()? {
        Some(first) if first > 1 => {
            let (seq, state) = store
                .snapshot_at(first - 1)?
                .with_context(|| format!("no snapshot at height {} to start from", first - 1))?;
            let header = store
                .checkpoint(seq)?
                .with_context(|| format!("no checkpoint {seq}"))?
                .header;
            (first - 1, state, sha256(&header))
        }
        _ => (0, genesis_state, [0u8; 32]),
    };
    let from_height = height;
    let mut head_ok = head == height && head_state == state;
    let mut batch: Vec<BlockRecordV1> = Vec::new();
    let mut checkpoints = 0;
    let mut cpu_max = 0;
    while height < tip {
        let to = (height + 500).min(tip);
        for (record, _) in store.blocks(height + 1, to)? {
            height += 1;
            let (out, metering) = exec
                .step(&state, &record.input)
                .map_err(|e| anyhow::anyhow!("block {height}: {e:?}"))?;
            cpu_max = cpu_max.max(metering.cpu_insns);
            if sha256(&out.state) != record.state_hash_after {
                bail!("block {height}: state_hash_after differs from re-execution");
            }
            state = out.state;
            if height == head {
                head_ok = state == head_state;
            }
            batch.push(record.clone());
            if BlockInputV1::decode(&record.input)
                .map_err(|_| anyhow::anyhow!("block {height} does not decode"))?
                .checkpoint_end
            {
                app.decode_state(&state)
                    .ok_or_else(|| anyhow::anyhow!("state after {height}"))?;
                let (_, header) = checkpoint::assemble(ids, prev_header, &batch, &state)
                    .map_err(|e| anyhow::anyhow!("checkpoint at {height}: {e:?}"))?;
                if let Some(row) = store.checkpoint(header.seq)? {
                    if row.header != header.encode().to_vec() {
                        bail!(
                            "checkpoint {}: stored header differs from the rebuilt one",
                            header.seq
                        );
                    }
                    checkpoints += 1;
                }
                prev_header = sha256(&header.encode());
                batch.clear();
            }
        }
    }
    if !head_ok {
        bail!("the stored head state differs from re-execution");
    }
    Ok(CheckReport {
        ok: true,
        from_height,
        height,
        final_state_hash: hex(&sha256(&state)),
        checkpoints_checked: checkpoints,
        host_cpu_insns_max: cpu_max,
    })
}
