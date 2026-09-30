//! The Caravel node library, for any app (M0.5, DEC-053): lane files, the
//! sequencer (T-007), the validator (T-008), the replay verifier (T-010) and
//! the commands every app's node binary has. An app implements
//! [`app::NodeApp`] and its binary calls [`cli::run`].

pub mod api;
pub mod app;
pub mod check;
pub mod cli;
pub mod lane_toml;
pub mod node_config;
pub mod replay;
pub mod sequencer;
pub mod stellar_rpc;
pub mod validator;
pub mod witness;

pub use app::{FeedApi, NodeApp};
