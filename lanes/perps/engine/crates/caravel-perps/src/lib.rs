//! Caravel Perps engine logic (spec §11): pure, deterministic `genesis` and `step`.
//!
//! `step(state, block) -> (state', receipts)` is the whole lane transition. The
//! same code runs natively (tests, sequencer pre-validation) and as the engine
//! contract Wasm inside `soroban-env-host` (consensus); both MUST produce the
//! same bytes (INV-P5).
//!
//! Consensus code: the determinism rules in spec §8 apply to everything here.
//! Integer arithmetic only, no time, randomness or I/O, sorted containers only.
#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]

extern crate alloc;

mod book;
mod commitment;
mod engine;
mod funding;
mod inbox;
pub mod invariants;
mod liquidation;
pub mod margin;
#[cfg(feature = "native")]
pub mod native;
mod oracle;
mod user;

use alloc::vec::Vec;

pub use caravel_types::fatal::Fatal;

/// A signature failed verification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidSignature;

/// Host crypto the engine needs (spec §11).
pub trait Crypto {
    fn sha256(&self, data: &[u8]) -> [u8; 32];

    /// MUST trap/panic on an invalid signature or key, the same as Soroban's
    /// `ed25519_verify` (spec §8.4).
    fn ed25519_verify(&self, public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]);

    /// The engine calls this. The default traps through [`Crypto::ed25519_verify`].
    /// A diagnostic implementation overrides it to return `Err`, so the
    /// sequencer can find the offending entry (spec §14.1, DEC-023).
    fn check_ed25519(
        &self,
        public_key: &[u8; 32],
        message: &[u8],
        signature: &[u8; 64],
    ) -> Result<(), InvalidSignature> {
        self.ed25519_verify(public_key, message, signature);
        Ok(())
    }
}

/// New state and receipts after one block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepOutput {
    /// `StateV1` bytes.
    pub state: Vec<u8>,
    /// `CVRCPT01` receipts bytes (spec §11.10).
    pub receipts: Vec<u8>,
}

/// Builds the genesis `StateV1` from `GenesisConfigV1` bytes (spec §10.2).
pub fn genesis(config_bytes: &[u8], c: &impl Crypto) -> Result<Vec<u8>, Fatal> {
    engine::genesis(config_bytes, c)
}

/// Executes one `BlockInputV1` against a `StateV1` (spec §11.2).
pub fn step(state_bytes: &[u8], block_bytes: &[u8], c: &impl Crypto) -> Result<StepOutput, Fatal> {
    engine::step(state_bytes, block_bytes, c)
}
