//! `caravel-perps-node`: the Caravel Perps node (spec §14–§16). The platform's
//! commands (sequencer, validator, replay, check-store, genesis, witness,
//! export-proofs) and the deploy commands (plan, apply) for
//! the perps app, plus `tx`.

use anyhow::Result;
use caravel_perps_node::{txcli, PerpsApp};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "caravel-perps-node",
    version,
    about = "Caravel Perps lane node (testnet only)"
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
    /// Sign a lane transaction with an account's key file and submit it.
    Tx(txcli::TxArgs),
    /// The protocol the `caravel` CLI drives this binary with (M0.6).
    #[command(hide = true)]
    Plugin {
        #[command(subcommand)]
        cmd: caravel_node::plugin::Command,
    },
}

/// The lane file `caravel init perps` starts from.
const EXAMPLE: &str = include_str!("../../config/init/lane.toml");

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Node(c) => caravel_node::cli::run(PerpsApp, c),
        Command::Deploy(c) => caravel_deploy::cli::run(PerpsApp, c),
        Command::Plugin { cmd } => {
            caravel_node::plugin::run(&PerpsApp, cmd, EXAMPLE, txcli::plugin_body)
        }
        Command::Tx(args) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(txcli::run(args)),
    }
}
