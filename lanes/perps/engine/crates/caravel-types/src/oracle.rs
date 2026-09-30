//! `OracleUpdateV1`, a signed price for one market (spec §9.5).

use crate::codec::{DecodeError, Reader, Writer};
use crate::tags::TAG_ORACLE;

pub const ORACLE_UPDATE_LEN: usize = 114;
/// Bytes covered by the signature: `market_id · price · publish_time_ms`.
pub const ORACLE_SIGNED_LEN: usize = 18;
/// `TAG_ORACLE || lane_id || bytes[0..18]`.
pub const ORACLE_SIGNING_PREIMAGE_LEN: usize = 17 + 32 + ORACLE_SIGNED_LEN;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OracleUpdateV1 {
    pub market_id: u16,
    /// USDC stroops per lot (spec §11.1).
    pub price: i64,
    pub publish_time_ms: u64,
    pub oracle_key: [u8; 32],
    /// ed25519 over `H(TAG_ORACLE || lane_id || bytes[0..18])`.
    pub signature: [u8; 64],
}

impl OracleUpdateV1 {
    /// `bytes[0..18]`, the signed fields.
    pub fn signed_fields(&self) -> [u8; ORACLE_SIGNED_LEN] {
        let mut out = [0u8; ORACLE_SIGNED_LEN];
        out[..2].copy_from_slice(&self.market_id.to_le_bytes());
        out[2..10].copy_from_slice(&self.price.to_le_bytes());
        out[10..].copy_from_slice(&self.publish_time_ms.to_le_bytes());
        out
    }

    /// `TAG_ORACLE || lane_id || bytes[0..18]`; the oracle key signs its SHA-256.
    pub fn signing_preimage(&self, lane_id: &[u8; 32]) -> [u8; ORACLE_SIGNING_PREIMAGE_LEN] {
        let mut out = [0u8; ORACLE_SIGNING_PREIMAGE_LEN];
        out[..17].copy_from_slice(TAG_ORACLE);
        out[17..49].copy_from_slice(lane_id);
        out[49..].copy_from_slice(&self.signed_fields());
        out
    }

    pub fn encode(&self) -> [u8; ORACLE_UPDATE_LEN] {
        let mut w = Writer::with_capacity(ORACLE_UPDATE_LEN);
        w.bytes(&self.signed_fields());
        w.bytes(&self.oracle_key);
        w.bytes(&self.signature);
        let mut out = [0u8; ORACLE_UPDATE_LEN];
        out.copy_from_slice(&w.into_vec());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != ORACLE_UPDATE_LEN {
            return Err(DecodeError::BadLength);
        }
        let mut r = Reader::new(bytes);
        let u = Self {
            market_id: r.u16()?,
            price: r.i64()?,
            publish_time_ms: r.u64()?,
            oracle_key: r.array()?,
            signature: r.array()?,
        };
        r.finish()?;
        Ok(u)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upd() -> OracleUpdateV1 {
        OracleUpdateV1 {
            market_id: 3,
            price: -5,
            publish_time_ms: 1_790_000_000_123,
            oracle_key: [8; 32],
            signature: [9; 64],
        }
    }

    #[test]
    fn round_trip_and_layout() {
        let b = upd().encode();
        assert_eq!(u16::from_le_bytes([b[0], b[1]]), 3);
        assert_eq!(i64::from_le_bytes(b[2..10].try_into().unwrap()), -5);
        assert_eq!(&b[18..50], &[8; 32]);
        assert_eq!(&b[50..], &[9; 64]);
        assert_eq!(OracleUpdateV1::decode(&b), Ok(upd()));
        assert_eq!(
            OracleUpdateV1::decode(&b[..113]),
            Err(DecodeError::BadLength)
        );
    }

    #[test]
    fn signing_preimage_excludes_key_and_signature() {
        let p = upd().signing_preimage(&[0x11; 32]);
        assert_eq!(&p[..17], b"CARAVEL/ORACLE/V1");
        assert_eq!(&p[17..49], &[0x11; 32]);
        assert_eq!(&p[49..], &upd().encode()[..18]);
    }
}
