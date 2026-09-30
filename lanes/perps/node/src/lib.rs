//! Caravel Perps on the platform runtime (M0.5, spec §20.3).
//!
//! - [`app::PerpsApp`]: the perps `LaneApp`, what the runtime needs from the
//!   engine (P-05);
//! - [`views`]: the perps JSON views (accounts, markets, books, fills).
//!
//! P-06 adds the `NodeApp` side (routes, lane-file section) and the binary.
//! The format compatibility tests (P-04) check the platform's generic readers
//! in `caravel-core` against the frozen perps codecs.

pub mod app;
pub mod views;

pub use app::PerpsApp;
