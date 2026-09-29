//! Caravel settlement contract (spec §13): USDC vault, inbox, checkpoints,
//! withdrawals, freeze and escape. The contract body lands in T-006.
#![no_std]
#![deny(clippy::float_arithmetic)]

use soroban_sdk::contract;

#[contract]
pub struct Settlement;
