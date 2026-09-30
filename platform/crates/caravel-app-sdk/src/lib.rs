//! Caravel app SDK (M0.5 P-08, spec §20.4, DEC-060): write a lane's app as
//! a few hooks on the standard lane pipeline.
//!
//! The SDK owns what every lane does the same way:
//! - the `AppGenesisV1` genesis config and the SDK state layout;
//! - block checks, deposits (with account creation, slot reuse and bounces)
//!   and forced withdrawals;
//! - the transaction envelope, signatures (raw ed25519 and SEP-53), nonces,
//!   rate limits and session keys;
//! - withdrawals, commitments and receipts.
//!
//! An app implements [`AppEngine`]: its kinds, its permission bits, how it
//! applies its kinds, and what an account may withdraw or escape with. The
//! standard paths repeat the frozen perps engine's rules (spec §11), which
//! `lanes/perps/node/tests/sdk_conformance.rs` checks.
//!
//! Consensus code: the determinism rules in spec §8 apply to everything here.
#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]

extern crate alloc;

pub mod app;
pub mod crypto;
pub mod genesis;
pub mod pipeline;
pub mod state;
#[cfg(feature = "testapp")]
pub mod testapp;

use alloc::vec::Vec;

pub use app::{AppCtx, AppEngine};
pub use crypto::Crypto;
pub use genesis::{template_id, AccessMode, AppGenesisV1};
pub use pipeline::{genesis, step};
pub use state::{AppAccountV1, PendingV1, SdkState, SessionKeyV1};

pub use caravel_core::codes::fatal::BLOCK_LEVEL;

/// A fatal error: the block is invalid (spec §8.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fatal {
    pub code: u16,
    /// The offending entry, or [`BLOCK_LEVEL`].
    pub entry_index: u32,
}

impl Fatal {
    pub fn block(code: u16) -> Self {
        Self {
            code,
            entry_index: BLOCK_LEVEL,
        }
    }

    pub fn entry(code: u16, entry_index: u32) -> Self {
        Self { code, entry_index }
    }
}

pub type Res<T> = Result<T, Fatal>;

/// Maps an arithmetic failure in a state transition to `ARITHMETIC_OVERFLOW`.
pub trait OrFatal<T> {
    fn or_fatal(self, entry: u32) -> Res<T>;
}

impl<T> OrFatal<T> for Option<T> {
    fn or_fatal(self, entry: u32) -> Res<T> {
        self.ok_or(Fatal::entry(
            caravel_core::codes::fatal::ARITHMETIC_OVERFLOW,
            entry,
        ))
    }
}

impl<T, E> OrFatal<T> for Result<T, E> {
    fn or_fatal(self, entry: u32) -> Res<T> {
        self.map_err(|_| Fatal::entry(caravel_core::codes::fatal::ARITHMETIC_OVERFLOW, entry))
    }
}

/// New state and receipts after one block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepOutput {
    pub state: Vec<u8>,
    /// `CVRCPT01` receipts.
    pub receipts: Vec<u8>,
}

/// SDK-INV1 (spec §20.4.3): what accounts hold, plus the app's own holdings,
/// plus the pending queue, equals what the lane holds on Stellar.
pub fn conserved(st: &SdkState, app_held: i128) -> bool {
    let balances = st
        .accounts
        .iter()
        .try_fold(0i128, |s, a| s.checked_add(a.balance));
    let pending = st
        .pending
        .iter()
        .try_fold(0i128, |s, p| s.checked_add(p.amount));
    let held = st
        .deposits_credited_total
        .checked_sub(st.withdrawals_committed_total);
    match (balances, pending, held) {
        (Some(b), Some(p), Some(h)) => {
            b.checked_add(p).and_then(|x| x.checked_add(app_held)) == Some(h)
        }
        _ => false,
    }
}
