//! `caravel-payments-node`: the Caravel Payments node. The platform's commands
//! (sequencer, validator, replay, check-store, genesis, witness, export-proofs)
//! and the deploy commands (plan, apply) for the payments app, plus `tx`.

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
    #[command(flatten)]
    Deploy(caravel_deploy::cli::Command),
    /// Sign a payments transaction with an account's key file and submit it.
    Tx(txcli::TxArgs),
    /// The protocol the `caravel` CLI drives this binary with (M0.6).
    #[command(hide = true)]
    Plugin {
        #[command(subcommand)]
        cmd: caravel_node::plugin::Command,
    },
}

/// The lane file `caravel init payments` starts from.
const EXAMPLE: &str = include_str!("../../config/init/lane.toml");

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Node(c) => caravel_node::cli::run(PaymentsApp, c),
        Command::Deploy(c) => caravel_deploy::cli::run(PaymentsApp, c),
        Command::Plugin { cmd } => {
            caravel_node::plugin::run(&PaymentsApp, cmd, EXAMPLE, txcli::plugin_body)
        }
        Command::Tx(args) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(txcli::run(args)),
    }
}
