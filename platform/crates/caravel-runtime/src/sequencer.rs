//! The sequencer's core (spec §14.1–§14.3), synchronous so tests drive it
//! with a fake clock. The node wraps it in the block loop and the HTTP API.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use caravel_core::block::{BlockInputV1, BlockRecordV1};
use caravel_core::checkpoint::CheckpointHeaderV1;
use caravel_core::codes::fatal::BLOCK_LEVEL;
use caravel_core::inbox::{inbox_acc_preimage, InboxMsgV1};
use caravel_core::receipts::ReceiptsV1;
use caravel_core::state::StateFrameV1;

use crate::app::{FeedUpdate, LaneApp, StepOutput};
use crate::builder::{self, BuildInput, Built, Source};
use crate::checkpoint::{self, block_hash, sha256, BatchBudget, HeaderIds};
use crate::executor::{ExecError, Metering, WasmExecutor};
use crate::mempool::{self, Mempool, Reject};
use crate::store::{CheckpointRow, CheckpointStatus, Store, StoreError};

/// How blocks are executed. Only `Wasm` is consensus (DEC-002); `Native` is
/// the app's native engine, for debugging and tests, and refused in
/// production (spec §14.5).
pub enum Executor {
    Wasm(WasmExecutor),
    Native,
}

impl Executor {
    pub fn step<A: LaneApp>(
        &self,
        app: &A,
        state: &[u8],
        block: &[u8],
    ) -> Result<(StepOutput, Metering), ExecError> {
        match self {
            Self::Wasm(w) => w.step(state, block),
            // The app's diagnostic run reports a bad signature as a fatal
            // instead of panicking, like the Wasm path's trap.
            Self::Native => app
                .native_step(state, block)
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
pub struct Produced<S> {
    pub height: u64,
    pub record: BlockRecordV1,
    pub receipts: ReceiptsV1,
    pub receipts_bytes: Vec<u8>,
    pub state: Arc<S>,
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

pub struct Core<A: LaneApp> {
    app: A,
    exec: Executor,
    store: Store,
    cfg: SequencerConfig,
    config_hash: [u8; 32],
    state_bytes: Vec<u8>,
    state: Arc<A::State>,
    frame: StateFrameV1,
    prev_block_hash: [u8; 32],
    pub mempool: Mempool,
    /// Reported inbox messages not yet in a block, from `state.inbox_through`.
    inbox: Vec<InboxMsgV1>,
    /// Count and fold of every inbox message reported so far.
    inbox_reported: u64,
    inbox_fold: [u8; 32],
    inbox_halted: bool,
    feeds: BTreeMap<u32, FeedUpdate>,
    budget: BatchBudget,
    last_header_hash: [u8; 32],
    backpressure: bool,
}

impl<A: LaneApp> Core<A> {
    /// Opens the core on a store, resuming from its head.
    pub fn open(
        app: A,
        exec: Executor,
        store: Store,
        config_hash: [u8; 32],
        cfg: SequencerConfig,
    ) -> Result<Self, CoreError> {
        let (height, state_bytes) = store.head()?;
        let state = app
            .decode_state(&state_bytes)
            .ok_or(CoreError::Corrupt("head state"))?;
        let frame =
            StateFrameV1::read(&state_bytes).map_err(|_| CoreError::Corrupt("head state"))?;
        let limits = app.limits(&state);
        let prev_block_hash = if height == 0 {
            [0; 32]
        } else {
            let (record, _) = store
                .block(height)?
                .ok_or(CoreError::Corrupt("head block"))?;
            block_hash(&record)
        };
        // The open batch: blocks after the last CHECKPOINT_END.
        let mut budget = BatchBudget::new(cfg.max_batch_bytes, limits.max_block_bytes as usize);
        let batch_start = frame.last_commitment.last_block_height + 1;
        if batch_start <= height {
            for (record, _) in store.blocks(batch_start, height)? {
                budget.add(record.input.len());
            }
        }
        let last_header_hash = match frame.checkpoint_seq {
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
            .inbox_from(frame.inbox_through, usize::MAX >> 1)?
            .into_iter()
            .map(|(m, _)| m)
            .collect();
        let mut feeds = BTreeMap::new();
        for (slot, bytes) in store.feed_updates()? {
            if let Some(u) = app.decode_feed(&bytes) {
                feeds.insert(slot, u);
            }
        }
        let mempool = Mempool::new(cfg.mempool_max, cfg.mempool_max_per_account);
        Ok(Self {
            app,
            exec,
            store,
            cfg,
            config_hash,
            state_bytes,
            state: Arc::new(state),
            frame,
            prev_block_hash,
            mempool,
            inbox,
            inbox_reported,
            inbox_fold,
            inbox_halted: false,
            feeds,
            budget,
            last_header_hash,
            backpressure: false,
        })
    }

    pub fn app(&self) -> &A {
        &self.app
    }

    pub fn state(&self) -> Arc<A::State> {
        self.state.clone()
    }

    /// The platform's view of the head state.
    pub fn frame(&self) -> &StateFrameV1 {
        &self.frame
    }

    pub fn state_bytes(&self) -> &[u8] {
        &self.state_bytes
    }

    pub fn state_hash(&self) -> [u8; 32] {
        sha256(&self.state_bytes)
    }

    pub fn height(&self) -> u64 {
        self.frame.height
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
        let (hash, tx) = mempool::prevalidate(
            &self.app,
            bytes,
            &self.state,
            &self.frame,
            &self.config_hash,
            now_ms,
        )?;
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
        if msg.index >= self.frame.inbox_through {
            self.inbox.push(msg);
        }
        Ok(InboxReport::Added)
    }

    /// `POST /internal/<feed>`: keeps the newest admissible update per slot.
    /// `false` if the app cannot decode it or would not accept it.
    pub fn report_feed(&mut self, bytes: &[u8]) -> Result<bool, CoreError> {
        let Some(u) = self.app.decode_feed(bytes) else {
            return Ok(false);
        };
        if !self.app.feed_admissible(&self.state, &u) {
            return Ok(false);
        }
        if self
            .feeds
            .get(&u.slot)
            .is_some_and(|old| old.publish_time_ms >= u.publish_time_ms)
        {
            return Ok(true);
        }
        self.store.put_feed(u.slot, &u.bytes)?;
        self.feeds.insert(u.slot, u);
        Ok(true)
    }

    /// Builds, executes and stores the next block. A fatal block is
    /// diagnosed, the offending entry quarantined, and the block rebuilt at
    /// the same height (spec §14.1).
    pub fn produce_block(&mut self, now_ms: u64) -> Result<Produced<A::State>, CoreError> {
        let limits = self.app.limits(&self.state);
        let max_block = limits.max_block_bytes as usize;
        let mut user_cap = limits.max_entries_per_block as usize;
        let mut incidents = Vec::new();
        let mut include_inbox = !self.inbox_halted;
        for _attempt in 0..16 {
            let mut byte_budget = self.budget.block_budget();
            if self.backpressure {
                byte_budget = byte_budget.min(max_block / 4);
            }
            let mut built = builder::build(&BuildInput {
                app: &self.app,
                state: &self.state,
                frame: &self.frame,
                prev_block_hash: self.prev_block_hash,
                now_ms,
                inbox: &self.inbox,
                feeds: &self.feeds,
                mempool: &self.mempool,
                byte_budget,
                user_cap,
                include_inbox,
            });
            let end = builder::checkpoint_end(
                &self.budget,
                built.encoded_len,
                self.cfg.checkpoint_every_blocks,
                self.app.pending_withdrawals(&self.state),
                limits.max_pending_withdrawals as usize,
            );
            built.block.checkpoint_end = end;
            let bytes = built
                .block
                .encode()
                .map_err(|_| CoreError::Corrupt("block encoding"))?;
            match self.exec.step(&self.app, &self.state_bytes, &bytes) {
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
                    let diag = self.app.native_step(&self.state_bytes, &bytes);
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
                                Source::Feed(slot) => {
                                    self.feeds.remove(&slot);
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
                                self.feeds.clear();
                            }
                        }
                    }
                }
            }
        }
        Err(CoreError::Halted(format!(
            "block {} did not execute after 16 attempts: {incidents:?}",
            self.frame.height + 1
        )))
    }

    fn accept(
        &mut self,
        built: Built,
        bytes: Vec<u8>,
        out: StepOutput,
        metering: Metering,
        mut incidents: Vec<Incident>,
    ) -> Result<Produced<A::State>, CoreError> {
        let new_state = self
            .app
            .decode_state(&out.state)
            .ok_or(CoreError::Corrupt("engine output state"))?;
        let new_frame = StateFrameV1::read(&out.state)
            .map_err(|_| CoreError::Corrupt("engine output state"))?;
        if !self.app.receipts_decode(&out.receipts) {
            return Err(CoreError::Corrupt("engine receipts"));
        }
        let receipts =
            ReceiptsV1::decode(&out.receipts).map_err(|_| CoreError::Corrupt("engine receipts"))?;
        let height = built.block.height;
        let record = BlockRecordV1 {
            input: bytes,
            state_hash_after: sha256(&out.state),
        };
        let end = built.block.checkpoint_end;
        let seq = end.then_some(new_frame.checkpoint_seq);
        self.store
            .commit_block(height, &record, &out.receipts, &out.state, seq)?;

        self.prev_block_hash = block_hash(&record);
        self.budget.add(record.input.len());
        let mut done = built.included.clone();
        done.extend(built.dropped.iter().copied());
        self.mempool.remove(&done);
        self.inbox.retain(|m| m.index >= new_frame.inbox_through);
        self.state_bytes = out.state;
        self.state = Arc::new(new_state);
        self.frame = new_frame;

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
        let first = self.frame.last_commitment.last_block_height - self.budget.blocks() as u64 + 1;
        let blocks = self.store.blocks(first, last_height)?;
        let records: Vec<BlockRecordV1> = blocks.iter().map(|(r, _)| r.clone()).collect();
        let (batch, header) = checkpoint::assemble(
            &self.cfg.ids,
            self.last_header_hash,
            &records,
            &self.state_bytes,
        )
        .map_err(|_| CoreError::Corrupt("checkpoint assembly"))?;
        let decoded: Vec<(BlockRecordV1, ReceiptsV1)> = blocks
            .into_iter()
            .map(|(r, rc)| {
                ReceiptsV1::decode(&rc)
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
            batch_len: batch.len(),
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
        if header.encode().len() != caravel_core::checkpoint::CHECKPOINT_HEADER_LEN {
            return Err("header length".into());
        }
        if header.network_id != self.cfg.ids.network_id
            || header.settlement_addr_hash != self.cfg.ids.settlement_addr_hash
            || header.engine_wasm_hash != self.cfg.ids.engine_wasm_hash
            || header.lane_id != self.frame.lane_id
        {
            return Err("identity fields".into());
        }
        if batch_len > caravel_core::batch::MAX_BATCH_BYTES {
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
        if self.frame.withdrawals_committed_total > cum {
            return Err(format!(
                "solvency: committed {} > deposits {cum}",
                self.frame.withdrawals_committed_total
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
    block.entries[i].payload().unwrap_or_default()
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
