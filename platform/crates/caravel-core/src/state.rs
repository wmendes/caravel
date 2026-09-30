//! The state frame every lane keeps (spec §9.10, DEC-052).
//!
//! An engine's state bytes are app-defined, except for two parts the platform
//! reads without knowing the app:
//! - the first 209 bytes: `magic[8] · lane_id · config_hash · height u64 ·
//!   last_block_input_hash · last_timestamp_ms u64 · checkpoint_seq u64 ·
//!   inbox_through u64 · inbox_acc · app_word u64 ·
//!   deposits_credited_total i128 · withdrawals_committed_total i128 ·
//!   app_flags u8`;
//! - the last 160 bytes: the `CommitmentV1` of the last checkpoint.
//!
//! This is the M0 perps `StateV1` layout, reinterpreted: its `next_order_id`
//! (offset 168) and `flags` (offset 208) are the app-reserved `app_word` and
//! `app_flags`. The magic is the app's.

use crate::codec::{DecodeError, Reader, Writer};

pub const FRAME_PREFIX_LEN: usize = 209;
pub const COMMITMENT_LEN: usize = 160;
/// Offset of `app_word` (M0 perps: `next_order_id`).
pub const APP_WORD_OFFSET: usize = 168;
/// Offset of `app_flags` (M0 perps: `flags`).
pub const APP_FLAGS_OFFSET: usize = 208;

/// What the last checkpoint committed to (spec §11.8), the last 160 bytes of the state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommitmentV1 {
    pub seq: u64,
    pub last_block_height: u64,
    pub accounts_root: [u8; 32],
    pub account_count: u32,
    pub escape_total: i128,
    pub withdrawals_root: [u8; 32],
    pub withdrawal_count: u32,
    pub withdrawals_total: i128,
    pub inbox_through: u64,
    pub inbox_acc: [u8; 32],
}

impl CommitmentV1 {
    pub fn encode(&self) -> [u8; COMMITMENT_LEN] {
        let mut w = Writer::with_capacity(COMMITMENT_LEN);
        w.u64(self.seq);
        w.u64(self.last_block_height);
        w.bytes(&self.accounts_root);
        w.u32(self.account_count);
        w.i128(self.escape_total);
        w.bytes(&self.withdrawals_root);
        w.u32(self.withdrawal_count);
        w.i128(self.withdrawals_total);
        w.u64(self.inbox_through);
        w.bytes(&self.inbox_acc);
        let mut out = [0u8; COMMITMENT_LEN];
        out.copy_from_slice(&w.into_vec());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let c = Self {
            seq: r.u64()?,
            last_block_height: r.u64()?,
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
        Ok(c)
    }
}

/// The platform's view of a state: its frame and commitment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StateFrameV1 {
    pub magic: [u8; 8],
    pub lane_id: [u8; 32],
    pub config_hash: [u8; 32],
    pub height: u64,
    pub last_block_input_hash: [u8; 32],
    pub last_timestamp_ms: u64,
    pub checkpoint_seq: u64,
    pub inbox_through: u64,
    pub inbox_acc: [u8; 32],
    pub app_word: u64,
    pub deposits_credited_total: i128,
    pub withdrawals_committed_total: i128,
    pub app_flags: u8,
    pub last_commitment: CommitmentV1,
}

impl StateFrameV1 {
    /// Reads the frame of `state`. It checks only that the state is long enough
    /// to hold both parts; the app decodes the rest.
    pub fn read(state: &[u8]) -> Result<Self, DecodeError> {
        if state.len() < FRAME_PREFIX_LEN + COMMITMENT_LEN {
            return Err(DecodeError::UnexpectedEnd);
        }
        let mut r = Reader::new(&state[..FRAME_PREFIX_LEN]);
        let frame = Self {
            magic: r.array()?,
            lane_id: r.array()?,
            config_hash: r.array()?,
            height: r.u64()?,
            last_block_input_hash: r.array()?,
            last_timestamp_ms: r.u64()?,
            checkpoint_seq: r.u64()?,
            inbox_through: r.u64()?,
            inbox_acc: r.array()?,
            app_word: r.u64()?,
            deposits_credited_total: r.i128()?,
            withdrawals_committed_total: r.i128()?,
            app_flags: r.u8()?,
            last_commitment: CommitmentV1::decode(&state[state.len() - COMMITMENT_LEN..])?,
        };
        r.finish()?;
        Ok(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_are_the_m0_layout() {
        let mut state = alloc::vec![0u8; FRAME_PREFIX_LEN + 50 + COMMITMENT_LEN];
        state[..8].copy_from_slice(b"CVSTATE1");
        state[72..80].copy_from_slice(&7u64.to_le_bytes()); // height
        state[120..128].copy_from_slice(&3u64.to_le_bytes()); // checkpoint_seq
        state[APP_WORD_OFFSET..APP_WORD_OFFSET + 8].copy_from_slice(&11u64.to_le_bytes());
        state[APP_FLAGS_OFFSET] = 1;
        let c = CommitmentV1 {
            seq: 3,
            last_block_height: 6,
            account_count: 2,
            escape_total: 99,
            ..CommitmentV1::default()
        };
        let n = state.len();
        state[n - COMMITMENT_LEN..].copy_from_slice(&c.encode());
        let f = StateFrameV1::read(&state).unwrap();
        assert_eq!(
            (
                &f.magic,
                f.height,
                f.checkpoint_seq,
                f.app_word,
                f.app_flags
            ),
            (b"CVSTATE1", 7, 3, 11, 1)
        );
        assert_eq!(f.last_commitment, c);
        assert_eq!(
            StateFrameV1::read(&state[..300]),
            Err(DecodeError::UnexpectedEnd)
        );
    }
}
