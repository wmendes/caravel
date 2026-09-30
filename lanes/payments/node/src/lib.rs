//! Caravel Payments on the platform (M0.5 P-10, spec §20.4.4): the payments
//! app for the runtime and the node, and the `caravel-payments-node` binary.
//!
//! - [`app::PaymentsApp`]: its `LaneApp` and `NodeApp`;
//! - [`lane_file`]: a payments lane file to `AppGenesisV1`;
//! - [`views`]: the account view;
//! - [`txcli`]: `caravel-payments-node tx`.

pub mod app;
pub mod lane_file;
pub mod txcli;
pub mod views;

pub use app::PaymentsApp;
