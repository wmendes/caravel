//! Caravel Perps on the platform runtime (M0.5, spec §20.3).
//!
//! P-05 and P-06 move the perps parts of the node here (the `LaneApp` and
//! `NodeApp` implementations, views, routes and the node binary). For now the
//! crate holds the format compatibility tests (P-04): the platform's generic
//! readers in `caravel-core` against the frozen perps codecs.
