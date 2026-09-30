//! The validator's core (spec §15), synchronous so tests drive it directly:
//! follow the sequencer by re-executing every block, compute every
//! checkpoint header independently, and sign only a header and batch that
//! match its own, never two different headers for one seq.

use std::sync::Arc;

use caravel_core::batch::BatchV1;
use caravel_core::block::{BlockInputV1, BlockRecordV1, Entry};
use caravel_core::checkpoint::CheckpointHeaderV1;
use caravel_core::receipts::ReceiptsV1;
use caravel_core::state::StateFrameV1;
use ed25519_dalek::{Signer, SigningKey};

use crate::app::LaneApp;
use crate::checkpoint::{self, block_hash, sha256, HeaderIds};
use crate::sequencer::{leaves_json, CoreError, Executor};
use crate::store::{CheckpointRow, CheckpointStatus, Store, StoreError};

/// Live policy (spec §15): only for blocks seen within this long of production.
pub const LIVE_WINDOW_MS: u64 = 10_000;
/// A live block may be at most this far ahead of the validator's clock.
pub const MAX_AHEAD_MS: u64 = 5_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowError {
    /// The record does not continue our chain or does not re-execute to the
    /// same state. The validator stops following and refuses to sign.
    Mismatch(String),
    /// Following stopped earlier; an operator must intervene.
    Halted(String),
    Store(String),
}

impl std::fmt::Display for FollowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mismatch(m) => write!(f, "mismatch: {m}"),
            Self::Halted(m) => write!(f, "halted: {m}"),
            Self::Store(m) => write!(f, "store: {m}"),
        }
    }
}

impl std::error::Error for FollowError {}

impl From<StoreError> for FollowError {
    fn from(e: StoreError) -> Self {
        Self::Store(e.to_string())
    }
}

/// Why a signature was refused (`POST /v1/sign`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Halted(String),
    /// The validator has not computed this checkpoint yet.
    NotCaughtUp {
        seq: u64,
        height: u64,
    },
    BadHeader,
    /// The header differs from the one this validator computed.
    HeaderMismatch,
    /// The batch differs from this validator's records.
    BatchMismatch,
    /// A block in the checkpoint failed a live check and is not cleared.
    Suspicious(Vec<(u64, String)>),
    /// `seq` is below the last signed seq, or the same seq with another header.
    Equivocation {
        seq: u64,
        last_signed: u64,
    },
    Store(String),
}

impl Refusal {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Halted(_) => "HALTED",
            Self::NotCaughtUp { .. } => "NOT_CAUGHT_UP",
            Self::BadHeader => "BAD_HEADER",
            Self::HeaderMismatch => "HEADER_MISMATCH",
            Self::BatchMismatch => "BATCH_MISMATCH",
            Self::Suspicious(_) => "SUSPICIOUS_BLOCK",
            Self::Equivocation { .. } => "ALREADY_SIGNED",
            Self::Store(_) => "STORE",
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Halted(m) => write!(f, "validator halted: {m}"),
            Self::NotCaughtUp { seq, height } => {
                write!(f, "checkpoint {seq} not computed yet (at height {height})")
            }
            Self::BadHeader => write!(f, "header does not decode"),
            Self::HeaderMismatch => {
                write!(f, "header differs from the one this validator computed")
            }
            Self::BatchMismatch => write!(f, "batch differs from this validator's blocks"),
            Self::Suspicious(b) => {
                write!(f, "blocks failed live checks and are not cleared: {b:?}")
            }
            Self::Equivocation { seq, last_signed } => {
                write!(f, "refusing seq {seq}: last signed is {last_signed}")
            }
            Self::Store(m) => write!(f, "store: {m}"),
        }
    }
}

/// One block applied.
#[derive(Debug, Clone)]
pub struct Applied {
    pub height: u64,
    /// Live-check failures for this block (it is followed anyway).
    pub flags: Vec<String>,
    /// Set when the block sealed a checkpoint.
    pub checkpoint: Option<u64>,
}

pub struct Follower<A: LaneApp> {
    app: A,
    exec: Executor,
    store: Store,
    ids: HeaderIds,
    key: SigningKey,
    state_bytes: Vec<u8>,
    state: Arc<A::State>,
    frame: StateFrameV1,
    prev_block_hash: [u8; 32],
    last_header_hash: [u8; 32],
    /// First height of the open batch.
    batch_start: u64,
    halted: Option<String>,
}

impl<A: LaneApp> Follower<A> {
    pub fn open(
        app: A,
        exec: Executor,
        store: Store,
        ids: HeaderIds,
        key: SigningKey,
    ) -> Result<Self, CoreError> {
        let (height, state_bytes) = store.head()?;
        let state = app
            .decode_state(&state_bytes)
            .ok_or(CoreError::Corrupt("head state"))?;
        let frame =
            StateFrameV1::read(&state_bytes).map_err(|_| CoreError::Corrupt("head state"))?;
        let prev_block_hash = if height == 0 {
            [0; 32]
        } else {
            block_hash(
                &store
                    .block(height)?
                    .ok_or(CoreError::Corrupt("head block"))?
                    .0,
            )
        };
        let last_header_hash = match frame.checkpoint_seq {
            0 => [0; 32],
            seq => sha256(
                &store
                    .checkpoint(seq)?
                    .ok_or(CoreError::Corrupt("last checkpoint"))?
                    .header,
            ),
        };
        let batch_start = frame.last_commitment.last_block_height + 1;
        Ok(Self {
            app,
            exec,
            store,
            ids,
            key,
            state_bytes,
            state: Arc::new(state),
            frame,
            prev_block_hash,
            last_header_hash,
            batch_start,
            halted: None,
        })
    }

    pub fn height(&self) -> u64 {
        self.frame.height
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

    pub fn state_hash(&self) -> [u8; 32] {
        sha256(&self.state_bytes)
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    pub fn halted(&self) -> Option<&str> {
        self.halted.as_deref()
    }

    fn halt(&mut self, why: String) -> FollowError {
        self.halted = Some(why.clone());
        FollowError::Mismatch(why)
    }

    /// Applies the next `BlockRecordV1` from the sequencer. `live` means it
    /// was received within [`LIVE_WINDOW_MS`] of production, so the live
    /// policy checks apply (they are skipped during catch-up).
    pub fn apply(&mut self, record: &BlockRecordV1, now_ms: u64) -> Result<Applied, FollowError> {
        if let Some(why) = &self.halted {
            return Err(FollowError::Halted(why.clone()));
        }
        let input = record
            .decode_input()
            .map_err(|_| self.halt("block input does not decode".into()))?;
        let height = self.frame.height + 1;
        if input.height != height {
            return Err(self.halt(format!("expected height {height}, got {}", input.height)));
        }
        // 1. It continues our chain.
        if input.prev_block_hash != self.prev_block_hash {
            return Err(self.halt(format!(
                "block {height}: prev_block_hash does not match our chain"
            )));
        }
        // 2. Execute it ourselves; 3. same state.
        let (out, _) = match self.exec.step(&self.app, &self.state_bytes, &record.input) {
            Ok(o) => o,
            Err(e) => return Err(self.halt(format!("block {height} does not execute: {e:?}"))),
        };
        let state_hash = sha256(&out.state);
        if state_hash != record.state_hash_after {
            return Err(self.halt(format!(
                "block {height}: state_hash_after differs from re-execution"
            )));
        }
        let Some(new_state) = self.app.decode_state(&out.state) else {
            return Err(self.halt("engine output state does not decode".into()));
        };
        let Ok(new_frame) = StateFrameV1::read(&out.state) else {
            return Err(self.halt("engine output state does not decode".into()));
        };
        let flags = if now_ms.saturating_sub(input.timestamp_ms) <= LIVE_WINDOW_MS {
            live_checks(&self.app, &input, now_ms)
        } else {
            Vec::new()
        };
        // 4. Persist (our own receipts), then any flags.
        let own = BlockRecordV1 {
            input: record.input.clone(),
            state_hash_after: state_hash,
        };
        let seq = input.checkpoint_end.then_some(new_frame.checkpoint_seq);
        self.store
            .commit_block(height, &own, &out.receipts, &out.state, seq)?;
        for f in &flags {
            self.store.flag_block(height, f)?;
        }
        self.prev_block_hash = block_hash(&own);
        self.state_bytes = out.state;
        self.state = Arc::new(new_state);
        self.frame = new_frame;
        let checkpoint = if input.checkpoint_end {
            Some(self.compute_checkpoint(height)?)
        } else {
            None
        };
        Ok(Applied {
            height,
            flags,
            checkpoint,
        })
    }

    /// Builds our own header and batch for the checkpoint `height` sealed.
    fn compute_checkpoint(&mut self, height: u64) -> Result<u64, FollowError> {
        let blocks = self.store.blocks(self.batch_start, height)?;
        let records: Vec<BlockRecordV1> = blocks.iter().map(|(r, _)| r.clone()).collect();
        let (batch, header) = checkpoint::assemble(
            &self.ids,
            self.last_header_hash,
            &records,
            &self.state_bytes,
        )
        .map_err(|e| FollowError::Store(format!("assembling checkpoint: {e:?}")))?;
        let decoded: Vec<(BlockRecordV1, ReceiptsV1)> = blocks
            .into_iter()
            .map(|(r, rc)| {
                ReceiptsV1::decode(&rc)
                    .map(|rc| (r, rc))
                    .map_err(|_| FollowError::Store("stored receipts".into()))
            })
            .collect::<Result<_, _>>()?;
        let leaves = checkpoint::withdrawal_leaves(&header, &decoded)
            .map_err(|_| FollowError::Store("withdrawal leaves".into()))?;
        let header_bytes = header.encode();
        self.store.insert_checkpoint(&CheckpointRow {
            seq: header.seq,
            header: header_bytes.to_vec(),
            batch,
            first_height: self.batch_start,
            last_height: height,
            withdrawals: leaves_json(&leaves),
            status: CheckpointStatus::Sequenced,
            epoch: None,
            sigs: None,
            stellar_tx_hash: None,
            stellar_ledger: None,
        })?;
        self.last_header_hash = sha256(&header_bytes);
        self.batch_start = height + 1;
        Ok(header.seq)
    }

    /// `POST /v1/sign`: signs `H(header)` only if header and batch are
    /// exactly what this validator computed, and never a second header for
    /// a seq. The signature is recorded before it is returned.
    pub fn sign(&mut self, header: &[u8], batch: &[u8]) -> Result<[u8; 64], Refusal> {
        if let Some(why) = &self.halted {
            return Err(Refusal::Halted(why.clone()));
        }
        let h = CheckpointHeaderV1::decode(header).map_err(|_| Refusal::BadHeader)?;
        let Some(own) = self
            .store
            .checkpoint(h.seq)
            .map_err(|e| Refusal::Store(e.to_string()))?
        else {
            return Err(Refusal::NotCaughtUp {
                seq: h.seq,
                height: self.frame.height,
            });
        };
        if own.header != header {
            return Err(Refusal::HeaderMismatch);
        }
        let theirs = BatchV1::decode(batch).map_err(|_| Refusal::BatchMismatch)?;
        let ours = self
            .store
            .blocks(own.first_height, own.last_height)
            .map_err(|e| Refusal::Store(e.to_string()))?;
        if theirs.blocks.len() != ours.len()
            || theirs.blocks.iter().zip(&ours).any(|(a, (b, _))| a != b)
            || batch != own.batch.as_slice()
        {
            return Err(Refusal::BatchMismatch);
        }
        let flags = self
            .store
            .flags_in(own.first_height, own.last_height)
            .map_err(|e| Refusal::Store(e.to_string()))?;
        if !flags.is_empty() {
            return Err(Refusal::Suspicious(flags));
        }
        let header_hash = sha256(header);
        if let Some(last) = self
            .store
            .last_signed_seq()
            .map_err(|e| Refusal::Store(e.to_string()))?
        {
            let same_again = h.seq == last
                && self
                    .store
                    .signed(last)
                    .map_err(|e| Refusal::Store(e.to_string()))?
                    == Some(header_hash);
            if h.seq < last || (h.seq == last && !same_again) {
                return Err(Refusal::Equivocation {
                    seq: h.seq,
                    last_signed: last,
                });
            }
        }
        // Persist before returning: this is what makes equivocation impossible.
        self.store
            .record_signed(h.seq, &header_hash)
            .map_err(|e| Refusal::Store(e.to_string()))?;
        Ok(self.key.sign(&header_hash).to_bytes())
    }

    /// Marks our checkpoints up to `seq` accepted when Stellar's
    /// `LastCkpt` shows `header_hash` for `seq` and it matches ours.
    pub fn observe_accepted(
        &mut self,
        seq: u64,
        header_hash: &[u8; 32],
    ) -> Result<bool, StoreError> {
        match self.store.checkpoint(seq)? {
            Some(row) if sha256(&row.header) == *header_hash => {
                self.store.mark_accepted_through(seq)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

/// The live policy checks for one block (spec §15): the block's time, and
/// each feed entry's, through the app's feed rule.
pub fn live_checks<A: LaneApp>(app: &A, input: &BlockInputV1, now_ms: u64) -> Vec<String> {
    let mut flags = Vec::new();
    if input.timestamp_ms > now_ms + MAX_AHEAD_MS {
        flags.push(format!("block timestamp {} is more than {MAX_AHEAD_MS} ms ahead of this validator's clock {now_ms}", input.timestamp_ms));
    }
    for e in &input.entries {
        if let Entry::Feed(bytes) = e {
            if let Some(flag) = app
                .decode_feed(bytes)
                .and_then(|u| app.feed_live_flag(&u, now_ms))
            {
                flags.push(flag);
            }
        }
    }
    flags
}
