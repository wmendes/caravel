//! `caravel-node`: the sequencer, validator and replay binary (spec §14–§16).

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "caravel-node",
    version,
    about = "Caravel lane node (testnet only)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build GenesisConfigV1 and the genesis state from a lane file, and print
    /// config_hash, genesis_state_hash and sizes as JSON (spec §10.3).
    Genesis {
        /// The lane file, e.g. config/lane.caravel-perps.testnet.toml.
        #[arg(long)]
        config: PathBuf,
        /// Also write the GenesisConfigV1 bytes to this file.
        #[arg(long)]
        config_out: Option<PathBuf>,
        /// Also write the genesis StateV1 bytes to this file.
        #[arg(long)]
        state_out: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Genesis {
            config,
            config_out,
            state_out,
        } => {
            let file = caravel_node::lane_toml::LaneFile::load(&config)?;
            let (report, config_bytes, state) = caravel_node::lane_toml::genesis(&file)?;
            if let Some(path) = config_out {
                std::fs::write(path, &config_bytes)?;
            }
            if let Some(path) = state_out {
                std::fs::write(path, &state)?;
            }
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }
    Ok(())
}
