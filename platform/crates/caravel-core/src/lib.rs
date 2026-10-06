//! Caravel platform formats (spec §9), shared by every lane, whatever app it runs.
//!
//! These are the M0 bytes, read without knowing the app:
//! - A block entry is `Inbox`, `Feed` (a signed data payload; the perps oracle
//!   update is one) or `User`. The body of a user transaction is opaque.
//! - A receipt's events are typed, length-framed fields.
//! - A state has a fixed frame (the header fields every lane keeps) and the
//!   160-byte commitment at its end.
//!
//! What is inside a transaction body, an event or the state body is the app's.
//! Its engine decodes it strictly (spec §11.2).
//!
//! Copied from the frozen perps crates (lanes/perps/engine, DEC-051) and
//! reinterpreted, never re-encoded (DEC-052). The compatibility tests in
//! `lanes/perps/node` check that every frozen vector reads and re-encodes to
//! the same bytes.
//!
//! Consensus code: the determinism rules in spec §8 apply to everything here.
#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]

extern crate alloc;

pub mod batch;
pub mod block;
pub mod checkpoint;
pub mod codec;
pub mod codes;
pub mod fixed;
pub mod inbox;
pub mod merkle;
pub mod preimage;
pub mod receipts;
pub mod state;
pub mod step;
pub mod tags;
pub mod tx;
pub mod wide;
