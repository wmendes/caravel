//! Caravel's deploy tool (spec §20.3): a lane file's `[env.<name>]` tables
//! say where and how a lane runs; `plan` compares that with what Stellar and
//! the host have; `apply` makes them match; `destroy` winds the lane down.
//!
//! - [`manifest`]: the `[env.<name>]` schema, with keys named as Stellar CLI
//!   identities and secrets refused.
//! - [`address`]: contract addresses known before deploying.
//! - [`plan`]: the pure diff from the lane file's deployment to steps and
//!   problems.
//!
//! There is no state file. The lane file and the chain are the only truth,
//! and each node reports what it runs (`/v1/status`).

pub mod address;
pub mod manifest;
pub mod plan;
pub mod versions;
