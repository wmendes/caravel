//! Caravel lane runtime (spec §14), for any app: executes the app's engine
//! Wasm through `soroban-env-host`, builds blocks and batches, assembles
//! checkpoints, stores everything in SQLite, and serves read-only views. What
//! it needs from the app is the `LaneApp` trait (M0.5, P-05).

pub mod app;
pub mod builder;
pub mod checkpoint;
pub mod executor;
pub mod mempool;
pub mod sequencer;
pub mod store;
pub mod validator;
pub mod views;

pub use app::{FeedUpdate, LaneApp, LaneLimits, NativeFatal, StepOutput};
pub use executor::{ExecError, Metering, WasmExecutor};
