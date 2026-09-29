//! Caravel lane runtime (spec §14.5): executes the engine Wasm through
//! `soroban-env-host`, builds blocks and batches, and stores them.

pub mod executor;

pub use executor::{ExecError, Metering, WasmExecutor};
