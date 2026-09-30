//! `BlockInputV1` and `BlockRecordV1` (spec §9.6), for any app.
//!
//! The bytes are the M0 block:
//! `CVBLKIN1 · lane_id · height u64 · timestamp_ms u64 · prev_block_hash ·
//! flags u8 · count u32 · count × (entry_type u8 · len u32 · payload)`.
//!
//! Entry types:
//! - 1 `Inbox`: an `InboxMsgV1`, decoded strictly;
//! - 2 `Feed`: a signed data payload, opaque here. The M0 perps oracle update
//!   is a feed (DEC-052);
//! - 3 `User`: a transaction envelope whose body is opaque.
//!
//! The decoder checks framing only. Entry order, feed rules, size and count
//! caps, and the meaning of bodies are checked by the app's engine (spec §11.2).

use alloc::vec::Vec;

use crate::codec::{DecodeError, EncodeError, Reader, Writer};
use crate::inbox::{InboxMsgV1, INBOX_MSG_LEN};
use crate::tx::TxEnvelopeV1;

pub const BLOCK_MAGIC: &[u8; 8] = b"CVBLKIN1";
pub const BLOCK_HEADER_LEN: usize = 93;
/// `flags` bit 0: the last block of a checkpoint batch. Other bits MUST be 0.
pub const FLAG_CHECKPOINT_END: u8 = 0x01;
/// Bytes before each entry payload: `entry_type u8 · len u32`.
pub const ENTRY_FRAME_LEN: usize = 5;

/// Entry type bytes (spec §9.6).
pub mod entry_type {
    pub const INBOX: u8 = 1;
    pub const FEED: u8 = 2;
    pub const USER: u8 = 3;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Inbox(InboxMsgV1),
    /// The raw feed payload.
    Feed(Vec<u8>),
    User(TxEnvelopeV1),
}

impl Entry {
    pub fn entry_type(&self) -> u8 {
        match self {
            Self::Inbox(_) => entry_type::INBOX,
            Self::Feed(_) => entry_type::FEED,
            Self::User(_) => entry_type::USER,
        }
    }

    pub fn payload_len(&self) -> usize {
        match self {
            Self::Inbox(_) => INBOX_MSG_LEN,
            Self::Feed(p) => p.len(),
            Self::User(tx) => tx.encoded_len(),
        }
    }

    /// Length of the framed entry: `entry_type · len · payload`.
    pub fn framed_len(&self) -> usize {
        ENTRY_FRAME_LEN + self.payload_len()
    }

    /// The payload bytes, as framed in the block.
    pub fn payload(&self) -> Result<Vec<u8>, EncodeError> {
        Ok(match self {
            Self::Inbox(m) => m.encode().to_vec(),
            Self::Feed(p) => p.clone(),
            Self::User(tx) => tx.encode().ok_or(EncodeError)?,
        })
    }
}

/// A block decode failure: in the 93-byte header / overall framing, or inside entry `index`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockDecodeError {
    Header(DecodeError),
    Entry { index: u32, error: DecodeError },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockInputV1 {
    pub lane_id: [u8; 32],
    /// First block is 1.
    pub height: u64,
    pub timestamp_ms: u64,
    /// Zero for height 1.
    pub prev_block_hash: [u8; 32],
    pub checkpoint_end: bool,
    pub entries: Vec<Entry>,
}

impl BlockInputV1 {
    pub fn flags(&self) -> u8 {
        if self.checkpoint_end {
            FLAG_CHECKPOINT_END
        } else {
            0
        }
    }

    pub fn encoded_len(&self) -> usize {
        BLOCK_HEADER_LEN + self.entries.iter().map(Entry::framed_len).sum::<usize>()
    }

    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let mut w = Writer::with_capacity(self.encoded_len());
        w.bytes(BLOCK_MAGIC);
        w.bytes(&self.lane_id);
        w.u64(self.height);
        w.u64(self.timestamp_ms);
        w.bytes(&self.prev_block_hash);
        w.u8(self.flags());
        w.count_u32(self.entries.len())?;
        for e in &self.entries {
            w.u8(e.entry_type());
            w.count_u32(e.payload_len())?;
            w.bytes(&e.payload()?);
        }
        Ok(w.into_vec())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BlockDecodeError> {
        let header = BlockDecodeError::Header;
        let mut r = Reader::new(bytes);
        r.magic(BLOCK_MAGIC).map_err(header)?;
        let lane_id = r.array().map_err(header)?;
        let height = r.u64().map_err(header)?;
        let timestamp_ms = r.u64().map_err(header)?;
        let prev_block_hash = r.array().map_err(header)?;
        let flags = r.u8().map_err(header)?;
        if flags & !FLAG_CHECKPOINT_END != 0 {
            return Err(header(DecodeError::BadFlags));
        }
        let entry_count = r.u32().map_err(header)?;
        let mut entries = Vec::new();
        for index in 0..entry_count {
            let entry =
                decode_entry(&mut r).map_err(|error| BlockDecodeError::Entry { index, error })?;
            entries.push(entry);
        }
        r.finish().map_err(header)?;
        Ok(Self {
            lane_id,
            height,
            timestamp_ms,
            prev_block_hash,
            checkpoint_end: flags & FLAG_CHECKPOINT_END != 0,
            entries,
        })
    }
}

fn decode_entry(r: &mut Reader<'_>) -> Result<Entry, DecodeError> {
    let entry_type = r.u8()?;
    let len = usize::try_from(r.u32()?).map_err(|_| DecodeError::UnexpectedEnd)?;
    let payload = r.take(len)?;
    Ok(match entry_type {
        entry_type::INBOX => Entry::Inbox(InboxMsgV1::decode(payload)?),
        entry_type::FEED => Entry::Feed(payload.to_vec()),
        entry_type::USER => Entry::User(TxEnvelopeV1::decode(payload)?),
        _ => return Err(DecodeError::BadEnum),
    })
}

/// `BlockInputV1 bytes || state_hash_after`, the unit stored and batched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockRecordV1 {
    /// The exact `BlockInputV1` bytes, kept raw because hashes and validator
    /// comparisons are over bytes.
    pub input: Vec<u8>,
    /// `H(state bytes after executing the block)`.
    pub state_hash_after: [u8; 32],
}

impl BlockRecordV1 {
    pub fn encoded_len(&self) -> usize {
        self.input.len() + 32
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(self.encoded_len());
        w.bytes(&self.input);
        w.bytes(&self.state_hash_after);
        w.into_vec()
    }

    /// Splits a record and checks that its input is a well-framed `BlockInputV1`.
    pub fn decode(bytes: &[u8]) -> Result<Self, BlockDecodeError> {
        let split = bytes
            .len()
            .checked_sub(32)
            .filter(|n| *n >= BLOCK_HEADER_LEN)
            .ok_or(BlockDecodeError::Header(DecodeError::UnexpectedEnd))?;
        let (input, hash) = bytes.split_at(split);
        BlockInputV1::decode(input)?;
        let mut state_hash_after = [0u8; 32];
        state_hash_after.copy_from_slice(hash);
        Ok(Self {
            input: input.to_vec(),
            state_hash_after,
        })
    }

    pub fn decode_input(&self) -> Result<BlockInputV1, BlockDecodeError> {
        BlockInputV1::decode(&self.input)
    }
}

/// `input_hash || state_hash_after`; `block_hash` is its SHA-256 (spec §9.6).
pub fn block_hash_preimage(input_hash: &[u8; 32], state_hash_after: &[u8; 32]) -> [u8; 64] {
    let mut out = [0u8; 64];
    out[..32].copy_from_slice(input_hash);
    out[32..].copy_from_slice(state_hash_after);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbox::InboxKind;
    use crate::tx::SigScheme;

    fn sample() -> BlockInputV1 {
        BlockInputV1 {
            lane_id: [1; 32],
            height: 2,
            timestamp_ms: 1_790_000_001_000,
            prev_block_hash: [5; 32],
            checkpoint_end: true,
            entries: alloc::vec![
                Entry::Inbox(InboxMsgV1 {
                    kind: InboxKind::Deposit,
                    index: 0,
                    lane_account: [2; 32],
                    amount: 10_000_000,
                    enqueued_at: 9,
                }),
                Entry::Feed(alloc::vec![7; 114]),
                Entry::User(TxEnvelopeV1 {
                    lane_id: [1; 32],
                    account: [2; 32],
                    signer: [2; 32],
                    nonce: 1,
                    expiry_ms: u64::MAX,
                    kind: 16,
                    sig_scheme: SigScheme::RawEd25519,
                    body: alloc::vec![3; 56],
                    signature: [6; 64],
                }),
            ],
        }
    }

    #[test]
    fn round_trip_and_framing() {
        let b = sample();
        let bytes = b.encode().unwrap();
        assert_eq!(bytes.len(), b.encoded_len());
        assert_eq!(BlockInputV1::decode(&bytes).unwrap(), b);
        let record = BlockRecordV1 {
            input: bytes.clone(),
            state_hash_after: [8; 32],
        };
        assert_eq!(BlockRecordV1::decode(&record.encode()).unwrap(), record);
    }

    #[test]
    fn bad_framing_is_rejected() {
        let bytes = sample().encode().unwrap();
        let mut v = bytes.clone();
        v[88] = 0x02; // a reserved flag bit
        assert_eq!(
            BlockInputV1::decode(&v),
            Err(BlockDecodeError::Header(DecodeError::BadFlags))
        );
        let mut v = bytes.clone();
        v[BLOCK_HEADER_LEN] = 4; // entry type 4 does not exist
        assert_eq!(
            BlockInputV1::decode(&v),
            Err(BlockDecodeError::Entry {
                index: 0,
                error: DecodeError::BadEnum
            })
        );
        let mut v = bytes;
        v.push(0);
        assert_eq!(
            BlockInputV1::decode(&v),
            Err(BlockDecodeError::Header(DecodeError::TrailingBytes))
        );
    }
}
