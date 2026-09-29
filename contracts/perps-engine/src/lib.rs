//! Caravel Perps engine contract (spec §12): a thin Soroban wrapper around
//! `caravel-perps`. `genesis` and `step` land in T-004.
#![no_std]
#![deny(clippy::float_arithmetic)]

use soroban_sdk::{contract, contractimpl, Bytes, BytesN, Env};

/// Domain tag for the rules identity returned by `version` (spec §12.1).
const TAG_ENGINE: &[u8] = b"CARAVEL/ENGINE/V1";
/// Must match the spec version in `versions.json`.
const SPEC_VERSION: &[u8] = b"0.1.0";

#[contract]
pub struct PerpsEngine;

#[contractimpl]
impl PerpsEngine {
    /// Rules identity: `H("CARAVEL/ENGINE/V1" || spec_version)`.
    pub fn version(env: Env) -> BytesN<32> {
        let mut preimage = Bytes::from_slice(&env, TAG_ENGINE);
        preimage.extend_from_slice(SPEC_VERSION);
        env.crypto().sha256(&preimage).into()
    }
}

#[cfg(test)]
mod test;
