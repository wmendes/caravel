//! The SDK state layout (spec §20.4.2, DEC-060): the `StateFrameV1` prefix,
//! the embedded `AppGenesisV1`, the accounts, the app's globals, the pending
//! withdrawals and the `CommitmentV1` trailer.

use alloc::vec::Vec;

use caravel_core::codec::{DecodeError, EncodeError, Reader, Writer};
use caravel_core::state::{CommitmentV1, COMMITMENT_LEN};

use crate::genesis::AppGenesisV1;

/// `AppAccountV1.flags` bit 0.
pub const FLAG_SYSTEM: u8 = 0x01;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionKeyV1 {
    pub key: [u8; 32],
    pub expires_at_ms: u64,
    pub permissions: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppAccountV1 {
    pub key: [u8; 32],
    pub flags: u8,
    pub next_nonce: u64,
    pub balance: i128,
    /// Sorted by key.
    pub session_keys: Vec<SessionKeyV1>,
    pub txs_this_block: u16,
    /// The app's per-account data.
    pub ext: Vec<u8>,
}

impl AppAccountV1 {
    pub fn is_system(&self) -> bool {
        self.flags & FLAG_SYSTEM != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingV1 {
    pub key: [u8; 32],
    pub amount: i128,
}

/// An SDK app's state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SdkState {
    /// The app's state magic.
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
    pub config: AppGenesisV1,
    pub accounts: Vec<AppAccountV1>,
    pub app_globals: Vec<u8>,
    pub pending: Vec<PendingV1>,
    pub last_commitment: CommitmentV1,
}

impl SdkState {
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let mut w = Writer::new();
        w.bytes(&self.magic);
        w.bytes(&self.lane_id);
        w.bytes(&self.config_hash);
        w.u64(self.height);
        w.bytes(&self.last_block_input_hash);
        w.u64(self.last_timestamp_ms);
        w.u64(self.checkpoint_seq);
        w.u64(self.inbox_through);
        w.bytes(&self.inbox_acc);
        w.u64(self.app_word);
        w.i128(self.deposits_credited_total);
        w.i128(self.withdrawals_committed_total);
        w.u8(self.app_flags);
        self.config.encode_to(&mut w)?;
        w.count_u32(self.accounts.len())?;
        for a in &self.accounts {
            w.bytes(&a.key);
            w.u8(a.flags);
            w.u64(a.next_nonce);
            w.i128(a.balance);
            w.count_u8(a.session_keys.len())?;
            for s in &a.session_keys {
                w.bytes(&s.key);
                w.u64(s.expires_at_ms);
                w.u8(s.permissions);
            }
            w.u16(a.txs_this_block);
            w.count_u16(a.ext.len())?;
            w.bytes(&a.ext);
        }
        w.count_u32(self.app_globals.len())?;
        w.bytes(&self.app_globals);
        w.count_u32(self.pending.len())?;
        for p in &self.pending {
            w.bytes(&p.key);
            w.i128(p.amount);
        }
        w.bytes(&self.last_commitment.encode());
        Ok(w.into_vec())
    }

    /// Strict: the app's magic, canonical ordering, nothing after the commitment.
    pub fn decode(bytes: &[u8], magic: &[u8; 8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        r.magic(magic)?;
        let lane_id = r.array()?;
        let config_hash = r.array()?;
        let height = r.u64()?;
        let last_block_input_hash = r.array()?;
        let last_timestamp_ms = r.u64()?;
        let checkpoint_seq = r.u64()?;
        let inbox_through = r.u64()?;
        let inbox_acc = r.array()?;
        let app_word = r.u64()?;
        let deposits_credited_total = r.i128()?;
        let withdrawals_committed_total = r.i128()?;
        let app_flags = r.u8()?;
        let config = AppGenesisV1::decode_from(&mut r)?;
        let mut accounts = Vec::new();
        for _ in 0..r.u32()? {
            let key = r.array()?;
            let flags = r.u8()?;
            if flags & !FLAG_SYSTEM != 0 {
                return Err(DecodeError::BadFlags);
            }
            let next_nonce = r.u64()?;
            let balance = r.i128()?;
            let mut session_keys: Vec<SessionKeyV1> = Vec::new();
            for _ in 0..r.u8()? {
                let s = SessionKeyV1 {
                    key: r.array()?,
                    expires_at_ms: r.u64()?,
                    permissions: r.u8()?,
                };
                if session_keys.last().is_some_and(|p| p.key >= s.key) {
                    return Err(DecodeError::Inconsistent);
                }
                session_keys.push(s);
            }
            let txs_this_block = r.u16()?;
            let ext_len = usize::from(r.u16()?);
            let ext = r.take(ext_len)?.to_vec();
            accounts.push(AppAccountV1 {
                key,
                flags,
                next_nonce,
                balance,
                session_keys,
                txs_this_block,
                ext,
            });
        }
        let len = usize::try_from(r.u32()?).map_err(|_| DecodeError::UnexpectedEnd)?;
        let app_globals = r.take(len)?.to_vec();
        let mut pending = Vec::new();
        for _ in 0..r.u32()? {
            pending.push(PendingV1 {
                key: r.array()?,
                amount: r.i128()?,
            });
        }
        let last_commitment = CommitmentV1::decode(r.take(COMMITMENT_LEN)?)?;
        r.finish()?;
        Ok(Self {
            magic: *magic,
            lane_id,
            config_hash,
            height,
            last_block_input_hash,
            last_timestamp_ms,
            checkpoint_seq,
            inbox_through,
            inbox_acc,
            app_word,
            deposits_credited_total,
            withdrawals_committed_total,
            app_flags,
            config,
            accounts,
            app_globals,
            pending,
            last_commitment,
        })
    }
}
