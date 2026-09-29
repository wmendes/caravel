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
    /// Replay the lane from Stellar data only (spec §16) and optionally print
    /// escape or withdrawal proofs for an account.
    Replay {
        /// Stellar RPC URL.
        #[arg(long)]
        rpc: String,
        #[arg(long)]
        network_passphrase: String,
        /// The settlement contract, C...
        #[arg(long)]
        settlement: String,
        /// The lane TOML the genesis config is derived from (as `genesis` does).
        #[arg(long)]
        genesis_config: PathBuf,
        /// The engine Wasm; its hash must be the contract's engine_wasm_hash.
        #[arg(long)]
        engine_wasm: PathBuf,
        /// Print the escape proof of this G... account from the last accepted checkpoint.
        #[arg(long)]
        prove_escape: Option<String>,
        /// Print this G... account's unclaimed withdrawal proofs.
        #[arg(long)]
        prove_withdrawals: Option<String>,
    },
    /// Sign a lane transaction with an account's key file and submit it.
    Tx(caravel_node::txcli::TxArgs),
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
        Command::Replay {
            rpc,
            network_passphrase,
            settlement,
            genesis_config,
            engine_wasm,
            prove_escape,
            prove_withdrawals,
        } => {
            caravel_node::node_config::check_network(&network_passphrase)?;
            let lane = caravel_node::lane_toml::LaneFile::load(&genesis_config)?;
            let (_, config_bytes, _) = caravel_node::lane_toml::genesis(&lane)?;
            let g = caravel_types::config::GenesisConfigV1::decode(&config_bytes)
                .map_err(|_| anyhow::anyhow!("config"))?;
            let wasm = std::fs::read(&engine_wasm)?;
            let wasm_hash = caravel_lane::checkpoint::sha256(&wasm);
            let exec = caravel_lane::sequencer::Executor::Wasm(caravel_lane::WasmExecutor::new(
                wasm,
                wasm_hash,
                g.exec_cpu_limit,
                g.exec_mem_limit,
            )?);
            let contract = caravel_node::node_config::parse_contract(&settlement)?;
            let ids = caravel_lane::checkpoint::HeaderIds {
                network_id: caravel_lane::checkpoint::network_id(&network_passphrase),
                settlement_addr_hash: caravel_lane::checkpoint::settlement_addr_hash(&contract),
                engine_wasm_hash: wasm_hash,
            };
            let src = caravel_node::replay::RpcSource {
                rpc: caravel_node::stellar_rpc::Rpc::new(&rpc)?,
                contract,
            };
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            rt.block_on(async {
                let outcome =
                    match caravel_node::replay::replay(&src, &lane, &exec, wasm_hash, &ids).await {
                        Ok(o) => o,
                        Err(e) => {
                            println!(
                                "{}",
                                serde_json::json!({ "ok": false, "error": format!("{e:#}") })
                            );
                            std::process::exit(2);
                        }
                    };
                println!(
                    "{}",
                    serde_json::to_string_pretty(&caravel_node::replay::report(&outcome))?
                );
                if let Some(a) = prove_escape {
                    let key = caravel_lane::views::parse_g(&a)
                        .ok_or_else(|| anyhow::anyhow!("--prove-escape needs a G... account"))?;
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&caravel_node::replay::escape_proof(
                            &outcome, &key
                        )?)?
                    );
                }
                if let Some(a) = prove_withdrawals {
                    let key = caravel_lane::views::parse_g(&a).ok_or_else(|| {
                        anyhow::anyhow!("--prove-withdrawals needs a G... account")
                    })?;
                    println!(
                        "{}",
                        serde_json::to_string_pretty(
                            &caravel_node::replay::withdrawal_proofs(&src, &outcome, &key).await?
                        )?
                    );
                }
                anyhow::Ok(())
            })?;
        }
        Command::Tx(args) => {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(caravel_node::txcli::run(args))?;
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
