//! `InboxMsgV1`, Stellar → lane messages built by the settlement contract (spec §9.4).

use crate::codec::{DecodeError, Reader, Writer};
use crate::tags::TAG_INBOX;

pub const INBOX_MSG_LEN: usize = 65;
/// `TAG_INBOX || acc || msg`.
pub const INBOX_ACC_PREIMAGE_LEN: usize = 16 + 32 + INBOX_MSG_LEN;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InboxKind {
    Deposit = 0,
    ForcedWithdrawal = 1,
}

impl InboxKind {
    pub fn from_u8(v: u8) -> Result<Self, DecodeError> {
        match v {
            0 => Ok(Self::Deposit),
            1 => Ok(Self::ForcedWithdrawal),
            _ => Err(DecodeError::BadEnum),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InboxMsgV1 {
    pub kind: InboxKind,
    /// 0-based and sequential.
    pub index: u64,
    pub lane_account: [u8; 32],
    pub amount: i128,
    /// Stellar ledger timestamp, seconds.
    pub enqueued_at: u64,
}

impl InboxMsgV1 {
    pub fn encode(&self) -> [u8; INBOX_MSG_LEN] {
        let mut w = Writer::with_capacity(INBOX_MSG_LEN);
        w.u8(self.kind as u8);
        w.u64(self.index);
        w.bytes(&self.lane_account);
        w.i128(self.amount);
        w.u64(self.enqueued_at);
        let mut out = [0u8; INBOX_MSG_LEN];
        out.copy_from_slice(&w.into_vec());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != INBOX_MSG_LEN {
            return Err(DecodeError::BadLength);
        }
        let mut r = Reader::new(bytes);
        let msg = Self {
            kind: InboxKind::from_u8(r.u8()?)?,
            index: r.u64()?,
            lane_account: r.array()?,
            amount: r.i128()?,
            enqueued_at: r.u64()?,
        };
        r.finish()?;
        Ok(msg)
    }
}

/// `TAG_INBOX || acc_n || msg_n`; `acc_{n+1}` is its SHA-256 and `acc_0` is zero (spec §9.4).
pub fn inbox_acc_preimage(
    acc: &[u8; 32],
    msg: &[u8; INBOX_MSG_LEN],
) -> [u8; INBOX_ACC_PREIMAGE_LEN] {
    let mut out = [0u8; INBOX_ACC_PREIMAGE_LEN];
    out[..16].copy_from_slice(TAG_INBOX);
    out[16..48].copy_from_slice(acc);
    out[48..].copy_from_slice(msg);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg() -> InboxMsgV1 {
        InboxMsgV1 {
            kind: InboxKind::Deposit,
            index: 3,
            lane_account: [7; 32],
            amount: 1_000_000_000,
            enqueued_at: 1_790_000_000,
        }
    }

    #[test]
    fn round_trip_and_layout() {
        let b = msg().encode();
        assert_eq!(b[0], 0);
        assert_eq!(u64::from_le_bytes(b[1..9].try_into().unwrap()), 3);
        assert_eq!(&b[9..41], &[7; 32]);
        assert_eq!(
            i128::from_le_bytes(b[41..57].try_into().unwrap()),
            1_000_000_000
        );
        assert_eq!(
            u64::from_le_bytes(b[57..65].try_into().unwrap()),
            1_790_000_000
        );
        assert_eq!(InboxMsgV1::decode(&b), Ok(msg()));
    }

    #[test]
    fn strict_decoding() {
        let mut b = msg().encode();
        b[0] = 2;
        assert_eq!(InboxMsgV1::decode(&b), Err(DecodeError::BadEnum));
        assert_eq!(
            InboxMsgV1::decode(&msg().encode()[..64]),
            Err(DecodeError::BadLength)
        );
        let mut long = msg().encode().to_vec();
        long.push(0);
        assert_eq!(InboxMsgV1::decode(&long), Err(DecodeError::BadLength));
    }

    #[test]
    fn acc_preimage_layout() {
        let p = inbox_acc_preimage(&[0xAA; 32], &msg().encode());
        assert_eq!(&p[..16], b"CARAVEL/INBOX/V1");
        assert_eq!(&p[16..48], &[0xAA; 32]);
        assert_eq!(&p[48..], &msg().encode());
    }
}
