//! `caravel-node check-store`: re-executes every block in a node's store from
//! genesis through the engine Wasm and checks each `state_hash_after`, the
//! stored head state, and every stored checkpoint header (rebuilt byte for
//! byte). Operators use it after a restart or on a copy of another node's store.

use std::path::Path;

use anyhow::{bail, Context, Result};
use caravel_runtime::checkpoint::{self, sha256, HeaderIds};
use caravel_runtime::sequencer::hex;
use caravel_runtime::store::Store;
use caravel_runtime::WasmExecutor;
use caravel_types::block::{BlockInputV1, BlockRecordV1};
use caravel_types::state::StateV1;
use serde::Serialize;

use crate::lane_toml::LaneFile;

#[derive(Serialize, Debug)]
pub struct CheckReport {
    pub ok: bool,
    pub height: u64,
    pub final_state_hash: String,
    pub checkpoints_checked: u64,
    pub host_cpu_insns_max: u64,
}

pub fn check_store(
    lane: &LaneFile,
    db: &Path,
    exec: &WasmExecutor,
    ids: &HeaderIds,
) -> Result<CheckReport> {
    if !db.exists() {
        bail!("no store at {}", db.display());
    }
    let (_, config_bytes, genesis_state) = crate::lane_toml::genesis(lane)?;
    let config_hash = sha256(&config_bytes);
    let store = Store::open(db, &lane.lane_id(), &config_hash, &genesis_state)
        .context("opening the store")?;
    let (head, head_state) = store.head()?;
    let mut state = genesis_state;
    let mut batch: Vec<BlockRecordV1> = Vec::new();
    let mut prev_header = [0u8; 32];
    let mut checkpoints = 0;
    let mut cpu_max = 0;
    let mut height = 0;
    while height < head {
        let to = (height + 500).min(head);
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
            batch.push(record.clone());
            if BlockInputV1::decode(&record.input)
                .map_err(|_| anyhow::anyhow!("block {height} does not decode"))?
                .checkpoint_end
            {
                let st =
                    StateV1::decode(&state).map_err(|_| anyhow::anyhow!("state after {height}"))?;
                let (_, header) = checkpoint::assemble(ids, prev_header, &batch, &st, &state)
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
    if state != head_state {
        bail!("the stored head state differs from re-execution");
    }
    Ok(CheckReport {
        ok: true,
        height,
        final_state_hash: hex(&sha256(&state)),
        checkpoints_checked: checkpoints,
        host_cpu_insns_max: cpu_max,
    })
}
