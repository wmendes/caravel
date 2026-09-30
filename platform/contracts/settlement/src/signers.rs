//! Weighted ed25519 signer sets (spec §13.4, DEC-004). Re-implemented from the
//! design of the Axelar Stellar gateway's `auth.rs`; no code copied (see
//! docs/SOURCES.md).

use alloc::vec::Vec as AllocVec;

use caravel_types::preimage::{signers_hash_preimage, WeightedSigner as RawSigner};
use soroban_sdk::{Bytes, BytesN, Env, Vec};

use crate::types::{Error, Sig, WeightedSigners};

pub const MAX_SIGNERS: u32 = 32;

/// `1 ≤ n ≤ 32`, keys strictly ascending, every weight > 0, `0 < threshold ≤ Σ weights`.
pub fn validate(set: &WeightedSigners) -> Result<(), Error> {
    let n = set.signers.len();
    if n == 0 || n > MAX_SIGNERS || set.threshold == 0 {
        return Err(Error::BadSignerSet);
    }
    let mut total: u64 = 0;
    let mut prev: Option<BytesN<32>> = None;
    for s in set.signers.iter() {
        if s.weight == 0 {
            return Err(Error::BadSignerSet);
        }
        if let Some(p) = &prev {
            if p.to_array() >= s.key.to_array() {
                return Err(Error::BadSignerSet);
            }
        }
        total += u64::from(s.weight);
        prev = Some(s.key);
    }
    if u64::from(set.threshold) > total {
        return Err(Error::BadSignerSet);
    }
    Ok(())
}

/// `H(TAG_SIGNERS || n u32 || Σ(key || weight u32) || threshold u32)`.
pub fn signers_hash(env: &Env, set: &WeightedSigners) -> BytesN<32> {
    let raw: AllocVec<RawSigner> = set
        .signers
        .iter()
        .map(|s| RawSigner {
            key: s.key.to_array(),
            weight: s.weight,
        })
        .collect();
    // n ≤ 32 after validation, so the count always fits.
    let preimage = signers_hash_preimage(&raw, set.threshold).unwrap_or_default();
    env.crypto()
        .sha256(&Bytes::from_slice(env, &preimage))
        .into()
}

/// Checks `sigs` over `msg` against `set`: indexes strictly ascending and in
/// range, every signature valid (an invalid one traps inside `ed25519_verify`),
/// and the summed weight at least the threshold.
pub fn verify(
    env: &Env,
    set: &WeightedSigners,
    msg: &BytesN<32>,
    sigs: &Vec<Sig>,
) -> Result<(), Error> {
    let message: Bytes = msg.clone().into();
    let mut weight: u64 = 0;
    let mut prev: Option<u32> = None;
    for sig in sigs.iter() {
        if prev.is_some_and(|p| sig.signer_index <= p) {
            return Err(Error::BadSignerIndex);
        }
        let signer = set
            .signers
            .get(sig.signer_index)
            .ok_or(Error::BadSignerIndex)?;
        env.crypto()
            .ed25519_verify(&signer.key, &message, &sig.signature);
        weight += u64::from(signer.weight);
        prev = Some(sig.signer_index);
    }
    if weight < u64::from(set.threshold) {
        return Err(Error::BelowThreshold);
    }
    Ok(())
}
