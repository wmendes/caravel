//! `CheckpointHeaderV1`, the 442-byte header validators sign (spec §9.8).

use crate::codec::{DecodeError, Reader, Writer};

pub const CHECKPOINT_MAGIC: &[u8; 8] = b"CVCKPT01";
pub const CHECKPOINT_VERSION: u16 = 1;
pub const CHECKPOINT_HEADER_LEN: usize = 442;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckpointHeaderV1 {
    pub lane_id: [u8; 32],
    /// `H(network passphrase)`.
    pub network_id: [u8; 32],
    /// `H(ScVal XDR of the settlement contract Address)`.
    pub settlement_addr_hash: [u8; 32],
    pub engine_wasm_hash: [u8; 32],
    /// First checkpoint is 1.
    pub seq: u64,
    /// Zero for seq 1.
    pub prev_header_hash: [u8; 32],
    pub first_block_height: u64,
    pub last_block_height: u64,
    pub last_block_timestamp_ms: u64,
    pub last_block_hash: [u8; 32],
    pub batch_hash: [u8; 32],
    pub state_hash: [u8; 32],
    pub accounts_root: [u8; 32],
    pub account_count: u32,
    pub escape_total: i128,
    pub withdrawals_root: [u8; 32],
    pub withdrawal_count: u32,
    pub withdrawals_total: i128,
    pub inbox_through: u64,
    pub inbox_acc: [u8; 32],
}

impl CheckpointHeaderV1 {
    pub fn encode(&self) -> [u8; CHECKPOINT_HEADER_LEN] {
        let mut w = Writer::with_capacity(CHECKPOINT_HEADER_LEN);
        w.bytes(CHECKPOINT_MAGIC);
        w.u16(CHECKPOINT_VERSION);
        w.bytes(&self.lane_id);
        w.bytes(&self.network_id);
        w.bytes(&self.settlement_addr_hash);
        w.bytes(&self.engine_wasm_hash);
        w.u64(self.seq);
        w.bytes(&self.prev_header_hash);
        w.u64(self.first_block_height);
        w.u64(self.last_block_height);
        w.u64(self.last_block_timestamp_ms);
        w.bytes(&self.last_block_hash);
        w.bytes(&self.batch_hash);
        w.bytes(&self.state_hash);
        w.bytes(&self.accounts_root);
        w.u32(self.account_count);
        w.i128(self.escape_total);
        w.bytes(&self.withdrawals_root);
        w.u32(self.withdrawal_count);
        w.i128(self.withdrawals_total);
        w.u64(self.inbox_through);
        w.bytes(&self.inbox_acc);
        let mut out = [0u8; CHECKPOINT_HEADER_LEN];
        out.copy_from_slice(&w.into_vec());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != CHECKPOINT_HEADER_LEN {
            return Err(DecodeError::BadLength);
        }
        let mut r = Reader::new(bytes);
        r.magic(CHECKPOINT_MAGIC)?;
        if r.u16()? != CHECKPOINT_VERSION {
            return Err(DecodeError::BadVersion);
        }
        let h = Self {
            lane_id: r.array()?,
            network_id: r.array()?,
            settlement_addr_hash: r.array()?,
            engine_wasm_hash: r.array()?,
            seq: r.u64()?,
            prev_header_hash: r.array()?,
            first_block_height: r.u64()?,
            last_block_height: r.u64()?,
            last_block_timestamp_ms: r.u64()?,
            last_block_hash: r.array()?,
            batch_hash: r.array()?,
            state_hash: r.array()?,
            accounts_root: r.array()?,
            account_count: r.u32()?,
            escape_total: r.i128()?,
            withdrawals_root: r.array()?,
            withdrawal_count: r.u32()?,
            withdrawals_total: r.i128()?,
            inbox_through: r.u64()?,
            inbox_acc: r.array()?,
        };
        r.finish()?;
        Ok(h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> CheckpointHeaderV1 {
        CheckpointHeaderV1 {
            lane_id: [1; 32],
            network_id: [2; 32],
            settlement_addr_hash: [3; 32],
            engine_wasm_hash: [4; 32],
            seq: 5,
            prev_header_hash: [6; 32],
            first_block_height: 41,
            last_block_height: 50,
            last_block_timestamp_ms: 1_790_000_050_000,
            last_block_hash: [7; 32],
            batch_hash: [8; 32],
            state_hash: [9; 32],
            accounts_root: [10; 32],
            account_count: 11,
            escape_total: 12,
            withdrawals_root: [13; 32],
            withdrawal_count: 14,
            withdrawals_total: 15,
            inbox_through: 16,
            inbox_acc: [17; 32],
        }
    }

    #[test]
    fn offsets_match_the_spec_table() {
        let b = header().encode();
        let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let i128_at = |o: usize| i128::from_le_bytes(b[o..o + 16].try_into().unwrap());
        assert_eq!(&b[0..8], b"CVCKPT01");
        assert_eq!(u16::from_le_bytes([b[8], b[9]]), 1);
        assert_eq!(&b[10..42], &[1; 32]);
        assert_eq!(&b[42..74], &[2; 32]);
        assert_eq!(&b[74..106], &[3; 32]);
        assert_eq!(&b[106..138], &[4; 32]);
        assert_eq!(u64_at(138), 5);
        assert_eq!(&b[146..178], &[6; 32]);
        assert_eq!(u64_at(178), 41);
        assert_eq!(u64_at(186), 50);
        assert_eq!(u64_at(194), 1_790_000_050_000);
        assert_eq!(&b[202..234], &[7; 32]);
        assert_eq!(&b[234..266], &[8; 32]);
        assert_eq!(&b[266..298], &[9; 32]);
        assert_eq!(&b[298..330], &[10; 32]);
        assert_eq!(u32_at(330), 11);
        assert_eq!(i128_at(334), 12);
        assert_eq!(&b[350..382], &[13; 32]);
        assert_eq!(u32_at(382), 14);
        assert_eq!(i128_at(386), 15);
        assert_eq!(u64_at(402), 16);
        assert_eq!(&b[410..442], &[17; 32]);
    }

    #[test]
    fn round_trip_and_strictness() {
        let b = header().encode();
        assert_eq!(CheckpointHeaderV1::decode(&b), Ok(header()));
        let mut v2 = b;
        v2[8] = 2;
        assert_eq!(
            CheckpointHeaderV1::decode(&v2),
            Err(DecodeError::BadVersion)
        );
        let mut magic = b;
        magic[7] = b'2';
        assert_eq!(
            CheckpointHeaderV1::decode(&magic),
            Err(DecodeError::BadMagic)
        );
        assert_eq!(
            CheckpointHeaderV1::decode(&b[..441]),
            Err(DecodeError::BadLength)
        );
    }
}
