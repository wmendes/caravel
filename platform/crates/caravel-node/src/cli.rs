//! The node commands every app's binary has (spec §14–§16, DEC-053): the
//! sequencer, validator, replay, check-store, genesis and witness. An app's
//! binary flattens [`Command`] into its own CLI and calls [`run`]:
//!
//! ```ignore
//! #[derive(clap::Subcommand)]
//! enum Cmd {
//!     #[command(flatten)]
//!     Node(caravel_node::cli::Command),
//!     // the app's own commands
//! }
//! ```

use std::path::PathBuf;

use anyhow::Result;
use clap::Subcommand;

use crate::app::NodeApp;

#[derive(Subcommand, Debug)]
pub enum Command {
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
        /// The lane file the genesis config is derived from (as `genesis` does).
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
    /// Print the witness `step` input (T-012) and the exact output the
    /// executor gives for it, as JSON.
    Witness {
        /// The lane file.
        #[arg(long)]
        lane: PathBuf,
        /// The engine Wasm of record.
        #[arg(long)]
        engine_wasm: PathBuf,
        /// The G... account the block deposits to.
        #[arg(long)]
        depositor: String,
        /// Block timestamp (ms); defaults to now.
        #[arg(long)]
        timestamp_ms: Option<u64>,
    },
    /// Run a validator (spec §15): follow, re-execute, sign, serve proofs.
    Validator {
        /// The validator config, e.g. lanes/perps/config/validator-1.local.toml.
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
        /// The sequencer config, e.g. lanes/perps/config/sequencer.local.toml.
        #[arg(long)]
        config: PathBuf,
    },
    /// Build the genesis config and state from a lane file, and print
    /// config_hash, genesis_state_hash and sizes as JSON (spec §10.3).
    Genesis {
        /// The lane file, e.g. lanes/perps/config/lane.caravel-perps.testnet.toml.
        #[arg(long)]
        config: PathBuf,
        /// Also write the genesis config bytes to this file.
        #[arg(long)]
        config_out: Option<PathBuf>,
        /// Also write the genesis state bytes to this file.
        #[arg(long)]
        state_out: Option<PathBuf>,
    },
}

pub fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Runs one node command for `app`.
pub fn run<A: NodeApp>(app: A, command: Command) -> Result<()> {
    match command {
        Command::CheckStore { config } => {
            let cfg = crate::node_config::SequencerConfig::load_offline(&config)?;
            let (_, config_bytes, _) = crate::lane_toml::genesis(&app, &cfg.lane)?;
            let (cpu, mem) = app.exec_limits(&config_bytes)?;
            let exec = caravel_runtime::WasmExecutor::from_file(
                &cfg.engine_wasm,
                cfg.engine_wasm_hash,
                cpu,
                mem,
            )?;
            let report =
                crate::check::check_store(&app, &cfg.lane, &cfg.db, &exec, &cfg.header_ids())?;
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
            crate::node_config::check_network(&network_passphrase)?;
            let lane = crate::lane_toml::LaneFile::load(&genesis_config)?;
            let (_, config_bytes, _) = crate::lane_toml::genesis(&app, &lane)?;
            let (cpu, mem) = app.exec_limits(&config_bytes)?;
            let wasm = std::fs::read(&engine_wasm)?;
            let wasm_hash = caravel_runtime::checkpoint::sha256(&wasm);
            let exec = caravel_runtime::sequencer::Executor::Wasm(
                caravel_runtime::WasmExecutor::new(wasm, wasm_hash, cpu, mem)?,
            );
            let contract = crate::node_config::parse_contract(&settlement)?;
            let ids = caravel_runtime::checkpoint::HeaderIds {
                network_id: caravel_runtime::checkpoint::network_id(&network_passphrase),
                settlement_addr_hash: caravel_runtime::checkpoint::settlement_addr_hash(&contract),
                engine_wasm_hash: wasm_hash,
            };
            let src = crate::replay::RpcSource {
                rpc: crate::stellar_rpc::Rpc::new(&rpc)?,
                contract,
            };
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            rt.block_on(async {
                let outcome =
                    match crate::replay::replay(&app, &src, &lane, &exec, wasm_hash, &ids).await {
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
                    serde_json::to_string_pretty(&crate::replay::report(&outcome))?
                );
                if let Some(a) = prove_escape {
                    let key = caravel_runtime::views::parse_g(&a)
                        .ok_or_else(|| anyhow::anyhow!("--prove-escape needs a G... account"))?;
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&crate::replay::escape_proof(
                            &app, &outcome, &key
                        )?)?
                    );
                }
                if let Some(a) = prove_withdrawals {
                    let key = caravel_runtime::views::parse_g(&a).ok_or_else(|| {
                        anyhow::anyhow!("--prove-withdrawals needs a G... account")
                    })?;
                    println!(
                        "{}",
                        serde_json::to_string_pretty(
                            &crate::replay::withdrawal_proofs(&src, &outcome, &key).await?
                        )?
                    );
                }
                anyhow::Ok(())
            })?;
        }
        Command::Witness {
            lane,
            engine_wasm,
            depositor,
            timestamp_ms,
        } => {
            let lane = crate::lane_toml::LaneFile::load(&lane)?;
            let (_, config_bytes, _) = crate::lane_toml::genesis(&app, &lane)?;
            let (cpu, mem) = app.exec_limits(&config_bytes)?;
            let wasm = std::fs::read(&engine_wasm)?;
            let hash = caravel_runtime::checkpoint::sha256(&wasm);
            lane.check_engine(&hash)?;
            let exec = caravel_runtime::WasmExecutor::new(wasm, hash, cpu, mem)?;
            let key = crate::lane_toml::parse_account(&depositor)?;
            let ts = timestamp_ms.unwrap_or_else(crate::sequencer::now_ms);
            println!(
                "{}",
                serde_json::to_string_pretty(&crate::witness::witness(
                    &app, &lane, &exec, key, ts
                )?)?
            );
        }
        Command::Validator { config } => {
            init_logging();
            let cfg = crate::validator::ValidatorConfig::load(&config)?;
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(crate::validator::run(app, cfg))?;
        }
        Command::ValidatorClear { config, through } => {
            let cfg = crate::validator::ValidatorConfig::load(&config)?;
            let (_, config_bytes, genesis_state) = crate::lane_toml::genesis(&app, &cfg.lane)?;
            let config_hash = caravel_runtime::checkpoint::sha256(&config_bytes);
            let mut store = caravel_runtime::store::Store::open(
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
            let cfg = crate::node_config::SequencerConfig::load(&config)?;
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(crate::sequencer::run(app, cfg))?;
        }
        Command::Genesis {
            config,
            config_out,
            state_out,
        } => {
            let file = crate::lane_toml::LaneFile::load(&config)?;
            let (report, config_bytes, state) = crate::lane_toml::genesis(&app, &file)?;
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
