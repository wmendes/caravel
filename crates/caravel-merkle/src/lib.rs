//! Merkle trees for Caravel checkpoints (spec §9.9), shared by the engine, the
//! nodes and the settlement contract.
//!
//! Consensus code: the determinism rules in spec §8 apply to everything here.
#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]

/// Maximum tree depth (spec §9.9).
pub const MAX_DEPTH: u32 = 20;
