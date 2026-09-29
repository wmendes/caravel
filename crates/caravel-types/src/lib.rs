//! Caravel binary encodings (spec §9), domain tags, reason and fatal codes, and
//! fixed-point helpers.
//!
//! Consensus code: the determinism rules in spec §8 apply to everything here.
//! All formats are fixed-width little-endian; decoders are strict (INV-D6).
//! This crate never hashes or signs: it builds the exact byte strings, and the
//! engine, nodes and contracts hash them with their own SHA-256.
#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]

extern crate alloc;
#[cfg(any(test, feature = "gen-vectors"))]
extern crate std;

pub mod batch;
pub mod block;
pub mod checkpoint;
pub mod codec;
pub mod codes;
pub mod config;
pub mod fatal;
pub mod fixed;
pub mod inbox;
pub mod oracle;
pub mod preimage;
pub mod receipts;
pub mod state;
pub mod step;
pub mod tags;
pub mod tx;

#[cfg(any(test, feature = "gen-vectors"))]
pub mod vectors;

pub use codec::{DecodeError, EncodeError};
pub use fatal::Fatal;

/// The spec version this crate implements (spec header, `versions.json`).
pub const SPEC_VERSION: &str = "0.1.0";
