//! Caravel Perps engine logic (spec §11): pure, deterministic `genesis` and `step`.
//!
//! Consensus code: the determinism rules in spec §8 apply to everything here.
#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]
