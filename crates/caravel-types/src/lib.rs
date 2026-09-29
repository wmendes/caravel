//! Caravel binary encodings (spec §9), domain tags, reason and fatal codes, and
//! fixed-point helpers.
//!
//! Consensus code: the determinism rules in spec §8 apply to everything here.
#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]

/// The spec version this crate implements (spec header, `versions.json`).
pub const SPEC_VERSION: &str = "0.1.0";
