//! The sequencer's core (spec §14.1–§14.3), synchronous so tests drive it
//! with a fake clock. The node wraps it in the block loop and the HTTP API.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use caravel_perps::native::DiagnosticCrypto;
use caravel_types::block::{BlockInputV1, BlockRecordV1};
use caravel_types::checkpoint::CheckpointHeaderV1;
use caravel_types::fatal::BLOCK_LEVEL;
use caravel_types::inbox::{inbox_acc_preimage, InboxMsgV1};
use caravel_types::oracle::OracleUpdateV1;
use caravel_types::receipts::Receipts;
use caravel_types::state::StateV1;

use crate::builder::{self, BuildInput, Built, Source};
use crate::checkpoint::{self, block_hash, sha256, BatchBudget, HeaderIds};
use crate::executor::{ExecError, Metering, WasmExecutor};
use crate::mempool::{self, Mempool, Reject};
use crate::store::{CheckpointRow, CheckpointStatus, Store, StoreError};

/// How blocks are executed. Only `Wasm` is consensus (DEC-002); `Native` is
/// for debugging and refused in production (spec §14.5).
pub enum Executor {
    Wasm(WasmExecutor),
    Native,
}

impl Executor {
    pub fn step(
        &self,
        state: &[u8],
        block: &[u8],
    ) -> Result<(caravel_perps::StepOutput, Metering), ExecError> {
        match self {
            Self::Wasm(w) => w.step(state, block),
            // The diagnostic crypto reports a bad signature as fatal 17
            // instead of panicking, like the Wasm path's trap.
            Self::Native => caravel_perps::step(state, block, &DiagnosticCrypto)
                .map(|o| (o, Metering::default()))
                .map_err(|f| ExecError::Fatal(f.code)),
        }
    }

    pub fn is_wasm(&self) -> bool {
        matches!(self, Self::Wasm(_))
    }
}

#[derive(Clone, Debug)]
pub struct SequencerConfig {
    pub ids: HeaderIds,
    pub checkpoint_every_blocks: u32,
    pub max_batch_bytes: usize,
    pub mempool_max: usize,
    pub mempool_max_per_account: usize,
}

#[derive(Debug)]
pub enum CoreError {
    Store(StoreError),
    /// The stored state or a stored block does not decode.
    Corrupt(&'static str),
    /// Not even an empty block executes: stop and page an operator.
    Halted(String),
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(e) => write!(f, "{e}"),
            Self::Corrupt(what) => write!(f, "corrupt: {what}"),
            Self::Halted(why) => write!(f, "halted: {why}"),
        }
    }
}

impl std::error::Error for CoreError {}

impl From<StoreError> for CoreError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

/// Something an operator should see in the logs (spec §14.1 step 4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Incident {
    /// The block was fatal; the entry was removed and the block rebuilt.
    Quarantined {
        height: u64,
        code: u16,
        entry_index: u32,
        source: String,
        entry_hex: String,
    },
    /// The block ran out of budget; it was rebuilt with fewer user entries.
    BudgetExceeded { height: u64, user_entries: usize },
    /// The relayer reported an inbox message whose `acc_after` is not our fold.
    InboxMismatch { index: u64 },
    /// A sealed checkpoint failed a pre-check; it is not sent for signing.
    PrecheckFailed { seq: u64, reason: String },
}

/// One produced block.
#[derive(Clone, Debug)]
pub struct Produced {
    pub height: u64,
    pub record: BlockRecordV1,
    pub receipts: Receipts,
    pub receipts_bytes: Vec<u8>,
    pub state: Arc<StateV1>,
    pub metering: Metering,
    /// Set when this block sealed a checkpoint.
    pub checkpoint: Option<CheckpointRow>,
    pub incidents: Vec<Incident>,
}

/// Result of `POST /internal/inbox`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InboxReport {
    Added,
    AlreadyKnown,
    /// Out of order: the relayer must send `expected` first.
    Gap {
        expected: u64,
    },
    /// `acc_after` does not match our fold: inbox inclusion is halted.
    Mismatch,
}

pub struct Core {
    exec: Executor,
    store: Store,
    cfg: SequencerConfig,
    config_hash: [u8; 32],
    state_bytes: Vec<u8>,
    state: Arc<StateV1>,
    prev_block_hash: [u8; 32],
    pub mempool: Mempool,
    /// Reported inbox messages not yet in a block, from `state.inbox_through`.
    inbox: Vec<InboxMsgV1>,
    /// Count and fold of every inbox message reported so far.
    inbox_reported: u64,
    inbox_fold: [u8; 32],
    inbox_halted: bool,
    oracle: BTreeMap<u16, OracleUpdateV1>,
    budget: BatchBudget,
    last_header_hash: [u8; 32],
    backpressure: bool,
}

impl Core {
    /// Opens the core on a store, resuming from its head.
    pub fn open(
        exec: Executor,
        store: Store,
        config_hash: [u8; 32],
        cfg: SequencerConfig,
    ) -> Result<Self, CoreError> {
        let (height, state_bytes) = store.head()?;
        let state = StateV1::decode(&state_bytes).map_err(|_| CoreError::Corrupt("head state"))?;
        let prev_block_hash = if height == 0 {
            [0; 32]
        } else {
            let (record, _) = store
                .block(height)?
                .ok_or(CoreError::Corrupt("head block"))?;
            block_hash(&record)
        };
        // The open batch: blocks after the last CHECKPOINT_END.
        let mut budget =
            BatchBudget::new(cfg.max_batch_bytes, state.config.max_block_bytes as usize);
        let batch_start = state.last_commitment.last_block_height + 1;
        if batch_start <= height {
            for (record, _) in store.blocks(batch_start, height)? {
                budget.add(record.input.len());
            }
        }
        let last_header_hash = match state.checkpoint_seq {
            0 => [0; 32],
            seq => sha256(
                &store
                    .checkpoint(seq)?
                    .ok_or(CoreError::Corrupt("last checkpoint"))?
                    .header,
            ),
        };
        let inbox_reported = store.inbox_count()?;
        let inbox_fold = match inbox_reported {
            0 => [0; 32],
            n => store
                .inbox_acc(n - 1)?
                .ok_or(CoreError::Corrupt("inbox acc"))?,
        };
        let inbox = store
            .inbox_from(state.inbox_through, usize::MAX >> 1)?
            .into_iter()
            .map(|(m, _)| m)
            .collect();
        let mut oracle = BTreeMap::new();
        for (market, bytes) in store.oracle_updates()? {
            if let Ok(u) = OracleUpdateV1::decode(&bytes) {
                oracle.insert(market, u);
            }
        }
        let mempool = Mempool::new(cfg.mempool_max, cfg.mempool_max_per_account);
        Ok(Self {
            exec,
            store,
            cfg,
            config_hash,
            state_bytes,
            state: Arc::new(state),
            prev_block_hash,
            mempool,
            inbox,
            inbox_reported,
            inbox_fold,
            inbox_halted: false,
            oracle,
            budget,
            last_header_hash,
            backpressure: false,
        })
    }

    pub fn state(&self) -> Arc<StateV1> {
        self.state.clone()
    }

    pub fn state_bytes(&self) -> &[u8] {
        &self.state_bytes
    }

    pub fn state_hash(&self) -> [u8; 32] {
        sha256(&self.state_bytes)
    }

    pub fn height(&self) -> u64 {
        self.state.height
    }

    pub fn config_hash(&self) -> [u8; 32] {
        self.config_hash
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    pub fn inbox_halted(&self) -> bool {
        self.inbox_halted
    }

    pub fn inbox_reported(&self) -> u64 {
        self.inbox_reported
    }

    /// The fold of every reported inbox message (zero before the first).
    pub fn inbox_reported_acc(&self) -> [u8; 32] {
        self.inbox_fold
    }

    pub fn is_wasm(&self) -> bool {
        self.exec.is_wasm()
    }

    /// Back-pressure from the submission queue (spec §14.3): on at ≥ 3 signed
    /// checkpoints waiting for Stellar, off again below 2.
    pub fn set_queue_len(&mut self, signed_waiting: usize) {
        if signed_waiting >= 3 {
            self.backpressure = true;
        } else if signed_waiting < 2 {
            self.backpressure = false;
        }
    }

    pub fn backpressure(&self) -> bool {
        self.backpressure
    }

    /// `POST /v1/tx`: pre-validates and queues a transaction.
    pub fn submit_tx(&mut self, bytes: &[u8], now_ms: u64) -> Result<[u8; 32], Reject> {
        let (hash, tx) = mempool::prevalidate(bytes, &self.state, &self.config_hash, now_ms)?;
        self.mempool.push(hash, tx)?;
        Ok(hash)
    }

    /// `POST /internal/inbox`: records a message as Stellar has it, after
    /// checking `acc_after` against our own fold.
    pub fn report_inbox(
        &mut self,
        msg: InboxMsgV1,
        acc_after: [u8; 32],
    ) -> Result<InboxReport, CoreError> {
        if msg.index < self.inbox_reported {
            return Ok(match self.store.put_inbox(&msg, &acc_after) {
                Ok(()) => InboxReport::AlreadyKnown,
                Err(StoreError::Conflict(_)) => {
                    self.inbox_halted = true;
                    InboxReport::Mismatch
                }
                Err(e) => return Err(e.into()),
            });
        }
        if msg.index > self.inbox_reported {
            return Ok(InboxReport::Gap {
                expected: self.inbox_reported,
            });
        }
        let fold = sha256(&inbox_acc_preimage(&self.inbox_fold, &msg.encode()));
        if fold != acc_after {
            self.inbox_halted = true;
            return Ok(InboxReport::Mismatch);
        }
        self.store.put_inbox(&msg, &acc_after)?;
        self.inbox_reported += 1;
        self.inbox_fold = fold;
        if msg.index >= self.state.inbox_through {
            self.inbox.push(msg);
        }
        Ok(InboxReport::Added)
    }

    /// `POST /internal/oracle`: keeps the newest valid update per market.
    pub fn report_oracle(&mut self, u: OracleUpdateV1) -> Result<bool, CoreError> {
        if !mempool::oracle_ok(&u, &self.state) {
            return Ok(false);
        }
        if self
            .oracle
            .get(&u.market_id)
            .is_some_and(|old| old.publish_time_ms >= u.publish_time_ms)
        {
            return Ok(true);
        }
        self.store.put_oracle(u.market_id, &u.encode())?;
        self.oracle.insert(u.market_id, u);
        Ok(true)
    }

    /// Builds, executes and stores the next block. A fatal block is
    /// diagnosed, the offending entry quarantined, and the block rebuilt at
    /// the same height (spec §14.1).
    pub fn produce_block(&mut self, now_ms: u64) -> Result<Produced, CoreError> {
        let max_block = self.state.config.max_block_bytes as usize;
        let mut user_cap = self.state.config.max_entries_per_block as usize;
        let mut incidents = Vec::new();
        let mut include_inbox = !self.inbox_halted;
        for _attempt in 0..16 {
            let mut byte_budget = self.budget.block_budget();
            if self.backpressure {
                byte_budget = byte_budget.min(max_block / 4);
            }
            let mut built = builder::build(&BuildInput {
                state: &self.state,
                prev_block_hash: self.prev_block_hash,
                now_ms,
                inbox: &self.inbox,
                oracle: &self.oracle,
                mempool: &self.mempool,
                byte_budget,
                user_cap,
                include_inbox,
            });
            let end = builder::checkpoint_end(
                &self.budget,
                built.encoded_len,
                self.cfg.checkpoint_every_blocks,
                self.state.pending.len(),
                self.state.config.max_pending_withdrawals as usize,
            );
            built.block.checkpoint_end = end;
            let bytes = built
                .block
                .encode()
                .map_err(|_| CoreError::Corrupt("block encoding"))?;
            match self.exec.step(&self.state_bytes, &bytes) {
                Ok((out, metering)) => return self.accept(built, bytes, out, metering, incidents),
                Err(ExecError::BudgetExceeded) => {
                    let users = built
                        .sources
                        .iter()
                        .filter(|s| matches!(s, Source::User(_)))
                        .count();
                    incidents.push(Incident::BudgetExceeded {
                        height: built.block.height,
                        user_entries: users,
                    });
                    user_cap = users / 2;
                }
                Err(_) => {
                    let diag = caravel_perps::step(&self.state_bytes, &bytes, &DiagnosticCrypto);
                    match diag {
                        Err(f) if (f.entry_index as usize) < built.sources.len() => {
                            let i = f.entry_index as usize;
                            let source = built.sources[i];
                            incidents.push(Incident::Quarantined {
                                height: built.block.height,
                                code: f.code,
                                entry_index: f.entry_index,
                                source: format!("{source:?}"),
                                entry_hex: hex(&entry_bytes(&built.block, i)),
                            });
                            match source {
                                Source::User(h) => self.mempool.remove(&HashSet::from([h])),
                                Source::Oracle(m) => {
                                    self.oracle.remove(&m);
                                }
                                // Inbox entries cannot be skipped: stop taking them.
                                Source::Inbox(_) => {
                                    self.inbox_halted = true;
                                    include_inbox = false;
                                }
                            }
                        }
                        // A block-level fatal, or one only the Wasm path hits: shrink the block.
                        other => {
                            let code = other.err().map_or(0, |f| f.code);
                            incidents.push(Incident::Quarantined {
                                height: built.block.height,
                                code,
                                entry_index: BLOCK_LEVEL,
                                source: "block".into(),
                                entry_hex: String::new(),
                            });
                            user_cap = built
                                .sources
                                .iter()
                                .filter(|s| matches!(s, Source::User(_)))
                                .count()
                                / 2;
                            if user_cap == 0 {
                                include_inbox = false;
                                self.oracle.clear();
                            }
                        }
                    }
                }
            }
        }
        Err(CoreError::Halted(format!(
            "block {} did not execute after 16 attempts: {incidents:?}",
            self.state.height + 1
        )))
    }

    fn accept(
        &mut self,
        built: Built,
        bytes: Vec<u8>,
        out: caravel_perps::StepOutput,
        metering: Metering,
        mut incidents: Vec<Incident>,
    ) -> Result<Produced, CoreError> {
        let new_state =
            StateV1::decode(&out.state).map_err(|_| CoreError::Corrupt("engine output state"))?;
        let receipts =
            Receipts::decode(&out.receipts).map_err(|_| CoreError::Corrupt("engine receipts"))?;
        let height = built.block.height;
        let record = BlockRecordV1 {
            input: bytes,
            state_hash_after: sha256(&out.state),
        };
        let end = built.block.checkpoint_end;
        let seq = end.then_some(new_state.checkpoint_seq);
        self.store
            .commit_block(height, &record, &out.receipts, &out.state, seq)?;

        self.prev_block_hash = block_hash(&record);
        self.budget.add(record.input.len());
        let mut done = built.included.clone();
        done.extend(built.dropped.iter().copied());
        self.mempool.remove(&done);
        self.inbox.retain(|m| m.index >= new_state.inbox_through);
        self.state_bytes = out.state;
        self.state = Arc::new(new_state);

        let checkpoint = if end {
            let row = self.seal(height, &mut incidents)?;
            self.budget.reset();
            Some(row)
        } else {
            None
        };
        Ok(Produced {
            height,
            record,
            receipts,
            receipts_bytes: out.receipts,
            state: self.state.clone(),
            metering,
            checkpoint,
            incidents,
        })
    }

    /// Assembles the checkpoint the last block sealed and stores it.
    fn seal(
        &mut self,
        last_height: u64,
        incidents: &mut Vec<Incident>,
    ) -> Result<CheckpointRow, CoreError> {
        let first = self.state.last_commitment.last_block_height - self.budget.blocks() as u64 + 1;
        let blocks = self.store.blocks(first, last_height)?;
        let records: Vec<BlockRecordV1> = blocks.iter().map(|(r, _)| r.clone()).collect();
        let (batch, header) = checkpoint::assemble(
            &self.cfg.ids,
            self.last_header_hash,
            &records,
            &self.state,
            &self.state_bytes,
        )
        .map_err(|_| CoreError::Corrupt("checkpoint assembly"))?;
        let decoded: Vec<(BlockRecordV1, Receipts)> = blocks
            .into_iter()
            .map(|(r, rc)| {
                Receipts::decode(&rc)
                    .map(|rc| (r, rc))
                    .map_err(|_| CoreError::Corrupt("stored receipts"))
            })
            .collect::<Result<_, _>>()?;
        let withdrawals = checkpoint::withdrawal_leaves(&header, &decoded)
            .map_err(|_| CoreError::Corrupt("withdrawal leaves"))?;
        let header_bytes = header.encode();
        let row = CheckpointRow {
            seq: header.seq,
            header: header_bytes.to_vec(),
            batch,
            first_height: first,
            last_height,
            withdrawals: leaves_json(&withdrawals),
            status: CheckpointStatus::Sequenced,
            epoch: None,
            sigs: None,
            stellar_tx_hash: None,
            stellar_ledger: None,
        };
        if let Err(reason) = self.precheck(&header, row.batch.len()) {
            incidents.push(Incident::PrecheckFailed {
                seq: header.seq,
                reason,
            });
        }
        self.store.insert_checkpoint(&row)?;
        self.last_header_hash = sha256(&header_bytes);
        Ok(row)
    }

    /// What the contract will check, so validators are never asked to sign a
    /// header Stellar would reject (spec §14.3 step 3).
    pub fn precheck(&self, header: &CheckpointHeaderV1, batch_len: usize) -> Result<(), String> {
        if header.encode().len() != caravel_types::checkpoint::CHECKPOINT_HEADER_LEN {
            return Err("header length".into());
        }
        if header.network_id != self.cfg.ids.network_id
            || header.settlement_addr_hash != self.cfg.ids.settlement_addr_hash
            || header.engine_wasm_hash != self.cfg.ids.engine_wasm_hash
            || header.lane_id != self.state.lane_id
        {
            return Err("identity fields".into());
        }
        if batch_len > caravel_types::batch::MAX_BATCH_BYTES {
            return Err(format!("batch is {batch_len} bytes"));
        }
        let reported = match header.inbox_through {
            0 => Some([0; 32]),
            n => self.store.inbox_acc(n - 1).map_err(|e| e.to_string())?,
        };
        if reported != Some(header.inbox_acc) {
            return Err(format!(
                "inbox_acc for {} messages does not match what the relayer reported from Stellar",
                header.inbox_through
            ));
        }
        // Contract check 8: withdrawals ≤ balance − outstanding − unprocessed,
        // which is cum_deposits(inbox_through) − withdrawals committed before.
        let cum = self
            .store
            .cum_deposits(header.inbox_through)
            .map_err(|e| e.to_string())?;
        if self.state.withdrawals_committed_total > cum {
            return Err(format!(
                "solvency: committed {} > deposits {cum}",
                self.state.withdrawals_committed_total
            ));
        }
        Ok(())
    }

    /// Whether the checkpoint's pre-check passes now (the relayer may have
    /// reported the inbox after the checkpoint was sealed).
    pub fn precheck_row(&self, row: &CheckpointRow) -> Result<(), String> {
        let header = CheckpointHeaderV1::decode(&row.header)
            .map_err(|_| "header does not decode".to_string())?;
        self.precheck(&header, row.batch.len())
    }
}

fn entry_bytes(block: &BlockInputV1, i: usize) -> Vec<u8> {
    use caravel_types::block::Entry;
    match &block.entries[i] {
        Entry::Inbox(m) => m.encode().to_vec(),
        Entry::Oracle(u) => u.encode().to_vec(),
        Entry::User(tx) => tx.encode(),
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `[[key_hex, "amount"], ...]`, the `withdrawals` column.
pub fn leaves_json(leaves: &[checkpoint::Leaf]) -> String {
    let items: Vec<String> = leaves
        .iter()
        .map(|l| format!("[\"{}\",\"{}\"]", hex(&l.key), l.amount))
        .collect();
    format!("[{}]", items.join(","))
}

/// Parses the `withdrawals` column back into leaves.
pub fn parse_leaves(json: &str) -> Option<Vec<checkpoint::Leaf>> {
    let v: Vec<(String, String)> = serde_json::from_str(json).ok()?;
    v.into_iter()
        .enumerate()
        .map(|(i, (k, a))| {
            let key: [u8; 32] = unhex(&k)?.try_into().ok()?;
            Some(checkpoint::Leaf {
                index: i as u32,
                key,
                amount: a.parse().ok()?,
            })
        })
        .collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}
