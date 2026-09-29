//! Hash preimages shared by the engine, nodes and the settlement contract.
//!
//! This crate never hashes: callers pass these bytes to their own SHA-256
//! (native `sha2`, or `env.crypto().sha256` in a contract).

use alloc::vec::Vec;

use crate::codec::{EncodeError, Writer};
use crate::tags::{
    MERKLE_LEAF_PREFIX, TAG_ACCT_LEAF, TAG_ENGINE, TAG_LANE_ID, TAG_ROTATE, TAG_SIGNERS,
    TAG_WDL_LEAF,
};

/// `0x00 || TAG_ACCT_LEAF || lane_id || seq || j || key || escape_equity`.
pub const ACCOUNT_LEAF_PREIMAGE_LEN: usize = 1 + 15 + 32 + 8 + 4 + 32 + 16;
/// `0x00 || TAG_WDL_LEAF || lane_id || seq || i || key || amount`.
pub const WITHDRAWAL_LEAF_PREIMAGE_LEN: usize = 1 + 14 + 32 + 8 + 4 + 32 + 16;

/// `TAG_LANE_ID || utf8(lane_name)`; `lane_id` is its SHA-256 (spec §9.1).
pub fn lane_id_preimage(lane_name: &str) -> Vec<u8> {
    let mut w = Writer::with_capacity(TAG_LANE_ID.len() + lane_name.len());
    w.bytes(TAG_LANE_ID);
    w.bytes(lane_name.as_bytes());
    w.into_vec()
}

/// `"CARAVEL/ENGINE/V1" || spec_version`; the engine's `version()` returns its SHA-256 (spec §12.1).
pub fn engine_version_preimage(spec_version: &str) -> Vec<u8> {
    let mut w = Writer::with_capacity(TAG_ENGINE.len() + spec_version.len());
    w.bytes(TAG_ENGINE);
    w.bytes(spec_version.as_bytes());
    w.into_vec()
}

fn leaf_preimage<const N: usize>(
    tag: &[u8],
    lane_id: &[u8; 32],
    seq: u64,
    index: u32,
    key: &[u8; 32],
    amount: i128,
) -> [u8; N] {
    let mut w = Writer::with_capacity(N);
    w.u8(MERKLE_LEAF_PREFIX);
    w.bytes(tag);
    w.bytes(lane_id);
    w.u64(seq);
    w.u32(index);
    w.bytes(key);
    w.i128(amount);
    let mut out = [0u8; N];
    out.copy_from_slice(&w.into_vec());
    out
}

/// Account leaf for the escape root (spec §11.8).
pub fn account_leaf_preimage(
    lane_id: &[u8; 32],
    seq: u64,
    index: u32,
    key: &[u8; 32],
    escape_equity: i128,
) -> [u8; ACCOUNT_LEAF_PREIMAGE_LEN] {
    leaf_preimage(TAG_ACCT_LEAF, lane_id, seq, index, key, escape_equity)
}

/// Withdrawal leaf for the withdrawals root (spec §11.8, §13.5).
pub fn withdrawal_leaf_preimage(
    lane_id: &[u8; 32],
    seq: u64,
    index: u32,
    key: &[u8; 32],
    amount: i128,
) -> [u8; WITHDRAWAL_LEAF_PREIMAGE_LEN] {
    leaf_preimage(TAG_WDL_LEAF, lane_id, seq, index, key, amount)
}

/// One weighted validator key (spec §13.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeightedSigner {
    pub key: [u8; 32],
    pub weight: u32,
}

/// `TAG_SIGNERS || n u32 || Σ(key || weight u32) || threshold u32`; `signers_hash` is its SHA-256 (spec §13.4).
pub fn signers_hash_preimage(
    signers: &[WeightedSigner],
    threshold: u32,
) -> Result<Vec<u8>, EncodeError> {
    let mut w = Writer::with_capacity(TAG_SIGNERS.len() + 8 + 36 * signers.len());
    w.bytes(TAG_SIGNERS);
    w.count_u32(signers.len())?;
    for s in signers {
        w.bytes(&s.key);
        w.u32(s.weight);
    }
    w.u32(threshold);
    Ok(w.into_vec())
}

/// `TAG_ROTATE || lane_id || network_id || settlement_addr_hash || new_epoch u64 || signers_hash(new)`,
/// the message the current signers sign to rotate (spec §13.4). `new_epoch` is `Epoch + 1`.
pub fn rotate_message_preimage(
    lane_id: &[u8; 32],
    network_id: &[u8; 32],
    settlement_addr_hash: &[u8; 32],
    new_epoch: u64,
    new_signers_hash: &[u8; 32],
) -> [u8; 17 + 32 * 4 + 8] {
    let mut out = [0u8; 17 + 32 * 4 + 8];
    let mut w = Writer::with_capacity(out.len());
    w.bytes(TAG_ROTATE);
    w.bytes(lane_id);
    w.bytes(network_id);
    w.bytes(settlement_addr_hash);
    w.u64(new_epoch);
    w.bytes(new_signers_hash);
    out.copy_from_slice(&w.into_vec());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaf_layouts() {
        let a = account_leaf_preimage(&[1; 32], 2, 3, &[4; 32], -5);
        assert_eq!(a.len(), 108);
        assert_eq!(a[0], 0x00);
        assert_eq!(&a[1..16], b"CARAVEL/ACCT/V1");
        assert_eq!(&a[16..48], &[1; 32]);
        assert_eq!(u64::from_le_bytes(a[48..56].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(a[56..60].try_into().unwrap()), 3);
        assert_eq!(&a[60..92], &[4; 32]);
        assert_eq!(i128::from_le_bytes(a[92..108].try_into().unwrap()), -5);

        let w = withdrawal_leaf_preimage(&[1; 32], 2, 3, &[4; 32], 6);
        assert_eq!(w.len(), 107);
        assert_eq!(&w[1..15], b"CARAVEL/WDL/V1");
    }

    #[test]
    fn signer_and_rotation_layouts() {
        let s = [
            WeightedSigner {
                key: [1; 32],
                weight: 1,
            },
            WeightedSigner {
                key: [2; 32],
                weight: 1,
            },
        ];
        let p = signers_hash_preimage(&s, 2).unwrap();
        assert_eq!(p.len(), 18 + 4 + 2 * 36 + 4);
        assert_eq!(&p[..18], b"CARAVEL/SIGNERS/V1");
        assert_eq!(u32::from_le_bytes(p[18..22].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(p[p.len() - 4..].try_into().unwrap()), 2);

        let r = rotate_message_preimage(&[1; 32], &[2; 32], &[3; 32], 7, &[4; 32]);
        assert_eq!(&r[..17], b"CARAVEL/ROTATE/V1");
        assert_eq!(u64::from_le_bytes(r[113..121].try_into().unwrap()), 7);
        assert_eq!(&r[121..], &[4; 32]);
    }

    #[test]
    fn lane_and_engine_ids() {
        assert_eq!(
            lane_id_preimage("caravel-perps-testnet-0"),
            b"CARAVEL/LANE/V1caravel-perps-testnet-0".to_vec()
        );
        assert_eq!(
            engine_version_preimage("0.1.0"),
            b"CARAVEL/ENGINE/V10.1.0".to_vec()
        );
    }
}
