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
    /// Re-execute a node's whole store through the engine Wasm and check every
    /// block and checkpoint header; prints a JSON report.
    CheckStore {
        /// The sequencer config whose store to check.
        #[arg(long)]
        config: PathBuf,
    },
    /// Run a validator (spec §15): follow, re-execute, sign, serve proofs.
    Validator {
        /// The validator config, e.g. config/validator-1.local.toml.
        #[arg(long)]
        config: PathBuf,
    },
    /// Clear live-check flags on blocks up to a height, after an operator
    /// has looked at them, so the validator signs again.
    ValidatorClear {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        through: u64,
    },
    /// Run the sequencer (spec §14): blocks, checkpoints and the API.
    Sequencer {
        /// The sequencer config, e.g. config/sequencer.local.toml.
        #[arg(long)]
        config: PathBuf,
    },
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

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::CheckStore { config } => {
            let cfg = caravel_node::node_config::SequencerConfig::load_offline(&config)?;
            let (_, config_bytes, _) = caravel_node::lane_toml::genesis(&cfg.lane)?;
            let g = caravel_types::config::GenesisConfigV1::decode(&config_bytes)
                .map_err(|_| anyhow::anyhow!("config"))?;
            let exec = caravel_lane::WasmExecutor::from_file(
                &cfg.engine_wasm,
                cfg.engine_wasm_hash,
                g.exec_cpu_limit,
                g.exec_mem_limit,
            )?;
            let report =
                caravel_node::check::check_store(&cfg.lane, &cfg.db, &exec, &cfg.header_ids())?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::Validator { config } => {
            init_logging();
            let cfg = caravel_node::validator::ValidatorConfig::load(&config)?;
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(caravel_node::validator::run(cfg))?;
        }
        Command::ValidatorClear { config, through } => {
            let cfg = caravel_node::validator::ValidatorConfig::load(&config)?;
            let (_, config_bytes, genesis_state) = caravel_node::lane_toml::genesis(&cfg.lane)?;
            let config_hash = caravel_lane::checkpoint::sha256(&config_bytes);
            let mut store = caravel_lane::store::Store::open(
                &cfg.db,
                &cfg.lane.lane_id(),
                &config_hash,
                &genesis_state,
            )?;
            let n = store.clear_flags(through)?;
            println!("cleared {n} flagged block(s) up to height {through}; restart the validator if it was halted");
        }
        Command::Sequencer { config } => {
            init_logging();
            let cfg = caravel_node::node_config::SequencerConfig::load(&config)?;
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(caravel_node::sequencer::run(cfg))?;
        }
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
