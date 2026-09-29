//! Caravel lane runtime (spec §14): executes the engine Wasm through
//! `soroban-env-host`, builds blocks and batches, assembles checkpoints,
//! stores everything in SQLite, and serves read-only views.

pub mod builder;
pub mod checkpoint;
pub mod executor;
pub mod mempool;
pub mod sequencer;
pub mod store;
pub mod validator;
pub mod views;

pub use executor::{ExecError, Metering, WasmExecutor};
