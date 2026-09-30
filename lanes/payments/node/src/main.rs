//! `caravel-payments-node`: the Caravel Payments node. The platform's commands
//! (sequencer, validator, replay, check-store, genesis, witness) for the
//! payments app, plus `tx`.

use anyhow::Result;
use caravel_payments_node::{txcli, PaymentsApp};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "caravel-payments-node",
    version,
    about = "Caravel Payments lane node (testnet only)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(flatten)]
    Node(caravel_node::cli::Command),
    /// Sign a payments transaction with an account's key file and submit it.
    Tx(txcli::TxArgs),
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Node(c) => caravel_node::cli::run(PaymentsApp, c),
        Command::Tx(args) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(txcli::run(args)),
    }
}
