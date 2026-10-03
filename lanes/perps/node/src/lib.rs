//! Caravel Perps on the platform (M0.5, spec §20.3): the perps app for the
//! runtime and the node, and the `caravel-perps-node` binary.
//!
//! - [`app::PerpsApp`]: the perps `LaneApp`, what the runtime needs from the
//!   engine (P-05), and its `NodeApp` ([`node`]): the lane file, the account
//!   view, `/v1/markets*`, the stream and the oracle feed route (P-06);
//! - [`lane_file`]: a perps lane file to `GenesisConfigV1`, in the platform
//!   layout (DEC-054) or the M0 one;
//! - [`views`]: the perps JSON views;
//! - [`history`]: oracle-price candles and recent fills, kept beside the store;
//! - [`txcli`]: `caravel-perps-node tx`.

pub mod app;
pub mod history;
pub mod lane_file;
pub mod node;
pub mod txcli;
pub mod views;

pub use app::PerpsApp;
