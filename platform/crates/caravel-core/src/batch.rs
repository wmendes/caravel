//! `BatchV1`, the data-availability payload posted on Stellar (spec §9.7), for
//! any app: the same bytes as M0, with blocks read by the generic block decoder.
//!
//! The decoder checks encoding only. Chain rules (consecutive heights, the
//! first `prev_block_hash`, `CHECKPOINT_END` only on the last block) need hashes
//! and are checked by validators and replay.

use alloc::vec::Vec;

use crate::block::{BlockDecodeError, BlockRecordV1};
use crate::codec::{DecodeError, EncodeError, Reader, Writer};

pub const BATCH_MAGIC: &[u8; 8] = b"CVBATCH1";
pub const BATCH_HEADER_LEN: usize = 52;
/// Batch size cap so a checkpoint transaction fits Stellar's size limit (spec §3.3).
pub const MAX_BATCH_BYTES: usize = 96_000;
/// Bytes a block adds to a batch besides its input: `len u32` + `state_hash_after`.
pub const BATCH_PER_BLOCK_OVERHEAD: usize = 4 + 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchV1 {
    pub lane_id: [u8; 32],
    pub checkpoint_seq: u64,
    pub blocks: Vec<BlockRecordV1>,
}

/// A batch decode failure: in the batch framing, or inside block `index`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchDecodeError {
    Framing(DecodeError),
    Block { index: u32, error: BlockDecodeError },
}

impl BatchV1 {
    pub fn encoded_len(&self) -> usize {
        BATCH_HEADER_LEN
            + self
                .blocks
                .iter()
                .map(|b| 4 + b.encoded_len())
                .sum::<usize>()
    }

    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let mut w = Writer::with_capacity(self.encoded_len());
        w.bytes(BATCH_MAGIC);
        w.bytes(&self.lane_id);
        w.u64(self.checkpoint_seq);
        w.count_u32(self.blocks.len())?;
        for b in &self.blocks {
            w.count_u32(b.encoded_len())?;
            w.bytes(&b.input);
            w.bytes(&b.state_hash_after);
        }
        Ok(w.into_vec())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BatchDecodeError> {
        let framing = BatchDecodeError::Framing;
        let mut r = Reader::new(bytes);
        r.magic(BATCH_MAGIC).map_err(framing)?;
        let lane_id = r.array().map_err(framing)?;
        let checkpoint_seq = r.u64().map_err(framing)?;
        let block_count = r.u32().map_err(framing)?;
        let mut blocks = Vec::new();
        for index in 0..block_count {
            let len = usize::try_from(r.u32().map_err(framing)?)
                .map_err(|_| framing(DecodeError::UnexpectedEnd))?;
            let record = r.take(len).map_err(framing)?;
            blocks.push(
                BlockRecordV1::decode(record)
                    .map_err(|error| BatchDecodeError::Block { index, error })?,
            );
        }
        r.finish().map_err(framing)?;
        Ok(Self {
            lane_id,
            checkpoint_seq,
            blocks,
        })
    }
}
