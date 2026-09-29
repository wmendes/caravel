//! Caravel test harness (test-only, std): a lane simulator that builds signed
//! blocks and checks the §11.9 invariants after each one, the §19.3 engine
//! scenarios, and the golden-vector generator (`cargo gen-vectors`).

pub mod lane;
pub mod random;
pub mod scenarios;

pub use lane::{Executor, Lane, NativeExecutor};
