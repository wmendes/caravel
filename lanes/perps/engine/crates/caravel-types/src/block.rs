//! `BlockInputV1` (what the engine executes) and `BlockRecordV1` (spec §9.6).
//!
//! The decoder checks encoding only. Rules that need state or config (entry
//! order, one oracle entry per market, size and count caps) are checked by the
//! engine, which reports them with their own fatal codes (spec §11.2).

use alloc::vec::Vec;

use crate::codec::{DecodeError, EncodeError, Reader, Writer};
use crate::inbox::{InboxMsgV1, INBOX_MSG_LEN};
use crate::oracle::{OracleUpdateV1, ORACLE_UPDATE_LEN};
use crate::tx::LaneTxV1;

pub const BLOCK_MAGIC: &[u8; 8] = b"CVBLKIN1";
pub const BLOCK_HEADER_LEN: usize = 93;
/// `flags` bit 0: the last block of a checkpoint batch. Other bits MUST be 0.
pub const FLAG_CHECKPOINT_END: u8 = 0x01;
/// Bytes before each entry payload: `entry_type u8 · len u32`.
pub const ENTRY_FRAME_LEN: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryType {
    Inbox = 1,
    Oracle = 2,
    User = 3,
}

impl EntryType {
    pub fn from_u8(v: u8) -> Result<Self, DecodeError> {
        match v {
            1 => Ok(Self::Inbox),
            2 => Ok(Self::Oracle),
            3 => Ok(Self::User),
            _ => Err(DecodeError::BadEnum),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    Inbox(InboxMsgV1),
    Oracle(OracleUpdateV1),
    User(LaneTxV1),
}

impl Entry {
    pub fn entry_type(&self) -> EntryType {
        match self {
            Self::Inbox(_) => EntryType::Inbox,
            Self::Oracle(_) => EntryType::Oracle,
            Self::User(_) => EntryType::User,
        }
    }

    pub fn payload_len(&self) -> usize {
        match self {
            Self::Inbox(_) => INBOX_MSG_LEN,
            Self::Oracle(_) => ORACLE_UPDATE_LEN,
            Self::User(tx) => tx.encoded_len(),
        }
    }

    /// Length of the framed entry: `entry_type · len · payload`.
    pub fn framed_len(&self) -> usize {
        ENTRY_FRAME_LEN + self.payload_len()
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
    /// INBOX first, then ORACLE, then USER (checked by the engine).
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
            w.u8(e.entry_type() as u8);
            w.count_u32(e.payload_len())?;
            match e {
                Entry::Inbox(m) => w.bytes(&m.encode()),
                Entry::Oracle(u) => w.bytes(&u.encode()),
                Entry::User(tx) => w.bytes(&tx.encode()),
            }
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
    let entry_type = EntryType::from_u8(r.u8()?)?;
    let len = usize::try_from(r.u32()?).map_err(|_| DecodeError::UnexpectedEnd)?;
    let payload = r.take(len)?;
    Ok(match entry_type {
        EntryType::Inbox => Entry::Inbox(InboxMsgV1::decode(payload)?),
        EntryType::Oracle => Entry::Oracle(OracleUpdateV1::decode(payload)?),
        EntryType::User => Entry::User(LaneTxV1::decode(payload)?),
    })
}

/// `BlockInputV1 bytes || state_hash_after`, the unit stored and batched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockRecordV1 {
    /// The exact `BlockInputV1` bytes, kept raw because hashes and validator
    /// comparisons are over bytes.
    pub input: Vec<u8>,
    /// `H(StateV1 bytes after executing the block)`.
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

    /// Splits a record and checks that its input is a valid `BlockInputV1`.
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
    use crate::tx::{SigScheme, TxBody};

    pub(crate) fn sample_block() -> BlockInputV1 {
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
                Entry::Oracle(OracleUpdateV1 {
                    market_id: 1,
                    price: 6_500_000,
                    publish_time_ms: 1_790_000_000_900,
                    oracle_key: [3; 32],
                    signature: [4; 64],
                }),
                Entry::User(LaneTxV1 {
                    lane_id: [1; 32],
                    account: [2; 32],
                    signer: [2; 32],
                    nonce: 1,
                    expiry_ms: u64::MAX,
                    sig_scheme: SigScheme::Sep53,
                    body: TxBody::Withdraw { amount: 1 },
                    signature: [6; 64],
                }),
            ],
        }
    }

    #[test]
    fn round_trip_and_header_layout() {
        let b = sample_block();
        let bytes = b.encode().unwrap();
        assert_eq!(bytes.len(), b.encoded_len());
        assert_eq!(bytes.len(), 93 + (5 + 65) + (5 + 114) + (5 + 197));
        assert_eq!(&bytes[..8], b"CVBLKIN1");
        assert_eq!(u64::from_le_bytes(bytes[40..48].try_into().unwrap()), 2);
        assert_eq!(bytes[88], FLAG_CHECKPOINT_END);
        assert_eq!(u32::from_le_bytes(bytes[89..93].try_into().unwrap()), 3);
        assert_eq!(bytes[93], 1);
        assert_eq!(u32::from_le_bytes(bytes[94..98].try_into().unwrap()), 65);
        assert_eq!(BlockInputV1::decode(&bytes), Ok(b));
    }

    #[test]
    fn empty_block_is_93_bytes() {
        let b = BlockInputV1 {
            entries: Vec::new(),
            checkpoint_end: false,
            ..sample_block()
        };
        let bytes = b.encode().unwrap();
        assert_eq!(bytes.len(), BLOCK_HEADER_LEN);
        assert_eq!(BlockInputV1::decode(&bytes), Ok(b));
    }

    #[test]
    fn errors_are_attributed_to_header_or_entry() {
        let good = sample_block().encode().unwrap();
        let mut bad_magic = good.clone();
        bad_magic[0] = b'X';
        assert_eq!(
            BlockInputV1::decode(&bad_magic),
            Err(BlockDecodeError::Header(DecodeError::BadMagic))
        );

        let mut bad_flags = good.clone();
        bad_flags[88] = 0x03;
        assert_eq!(
            BlockInputV1::decode(&bad_flags),
            Err(BlockDecodeError::Header(DecodeError::BadFlags))
        );

        let mut trailing = good.clone();
        trailing.push(0);
        assert_eq!(
            BlockInputV1::decode(&trailing),
            Err(BlockDecodeError::Header(DecodeError::TrailingBytes))
        );

        // Entry 1 (oracle) claims 113 bytes: its payload is the wrong length.
        let mut short_oracle = good.clone();
        let at = 93 + 5 + 65 + 1;
        short_oracle[at..at + 4].copy_from_slice(&113u32.to_le_bytes());
        assert!(matches!(
            BlockInputV1::decode(&short_oracle),
            Err(BlockDecodeError::Entry { index: 1, .. })
        ));

        // Entry 2 has an unknown entry type.
        let mut bad_type = good.clone();
        bad_type[93 + 70 + 119] = 9;
        assert_eq!(
            BlockInputV1::decode(&bad_type),
            Err(BlockDecodeError::Entry {
                index: 2,
                error: DecodeError::BadEnum
            })
        );

        // entry_count says 4 but only 3 entries follow.
        let mut count = good.clone();
        count[89..93].copy_from_slice(&4u32.to_le_bytes());
        assert_eq!(
            BlockInputV1::decode(&count),
            Err(BlockDecodeError::Entry {
                index: 3,
                error: DecodeError::UnexpectedEnd
            })
        );
    }

    #[test]
    fn record_split_and_block_hash_preimage() {
        let input = sample_block().encode().unwrap();
        let rec = BlockRecordV1 {
            input: input.clone(),
            state_hash_after: [0xEE; 32],
        };
        let bytes = rec.encode();
        assert_eq!(bytes.len(), input.len() + 32);
        assert_eq!(BlockRecordV1::decode(&bytes), Ok(rec));
        assert!(BlockRecordV1::decode(&bytes[..100]).is_err());
        let p = block_hash_preimage(&[1; 32], &[2; 32]);
        assert_eq!(&p[..32], &[1; 32]);
        assert_eq!(&p[32..], &[2; 32]);
    }
}
