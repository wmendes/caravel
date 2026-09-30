//! Executes the exact engine Wasm through `soroban-env-host 28.0.2` (spec
//! §14.5, DEC-002). This is the consensus path: the only one whose output is
//! signed (spec §8.2).
//!
//! Every call runs in a fresh host, so nothing carries over between blocks and
//! host memory stays bounded. The contract is registered with an unlimited
//! budget; then the budget is reset to the lane's consensus limits
//! (`exec_cpu_limit`, `exec_mem_limit` from `GenesisConfigV1`, DEC-015) right
//! before `step` runs. Metering is deterministic, so every node exhausts the
//! budget on the same block. The module cache stays off, so Wasm parsing and
//! instantiation count toward the budget on every node alike.

use std::path::Path;

use crate::app::StepOutput;
use caravel_core::step::StepEnvelope;
use sha2::{Digest, Sha256};
use soroban_env_host::budget::AsBudget;
use soroban_env_host::testutils::generate_account_id;
use soroban_env_host::xdr::{ScErrorCode, ScErrorType};
use soroban_env_host::{
    BytesObject, Env, EnvBase, Host, HostError, LedgerInfo, Symbol, U32Val, Val,
};

/// Protocol the host runs (spec §3.1).
pub const PROTOCOL_VERSION: u32 = 28;

/// Network id seen by the engine during lane execution. The engine never reads
/// ledger info, so this value cannot affect output (a test asserts it).
pub fn lane_exec_network_id() -> [u8; 32] {
    Sha256::digest(b"CARAVEL/LANE-EXEC/V1").into()
}

/// The fixed ledger info for lane execution (spec §14.5).
pub fn lane_ledger_info() -> LedgerInfo {
    LedgerInfo {
        protocol_version: PROTOCOL_VERSION,
        sequence_number: 1,
        timestamp: 0,
        network_id: lane_exec_network_id(),
        base_reserve: 5_000_000,
        min_persistent_entry_ttl: 4_096,
        min_temp_entry_ttl: 16,
        max_entry_ttl: 6_312_000,
    }
}

/// Host metering for one call. These are not network fees (spec §14.5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Metering {
    pub cpu_insns: u64,
    pub mem_bytes: u64,
}

/// Why a call did not produce output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecError {
    /// The engine returned a fatal code as a contract error (spec §11).
    Fatal(u16),
    /// `ed25519_verify` trapped: an invalid key or signature (fatal).
    InvalidSignature,
    /// The per-call CPU or memory limit was exceeded (fatal, DEC-015).
    BudgetExceeded,
    /// Any other host error or trap (fatal, spec §8.3).
    Host(String),
}

/// The Wasm could not be loaded.
#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    /// The file's sha256 is not the expected engine hash (spec §14.5).
    HashMismatch {
        expected: [u8; 32],
        actual: [u8; 32],
    },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "cannot read engine wasm: {e}"),
            Self::HashMismatch { expected, actual } => {
                write!(
                    f,
                    "engine wasm sha256 {} does not match the expected {}",
                    hex(actual),
                    hex(expected)
                )
            }
        }
    }
}

impl std::error::Error for LoadError {}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Consensus executor for the engine Wasm.
#[derive(Clone)]
pub struct WasmExecutor {
    wasm: Vec<u8>,
    wasm_hash: [u8; 32],
    cpu_limit: u64,
    mem_limit: u64,
    ledger_info: LedgerInfo,
}

impl WasmExecutor {
    /// Loads Wasm bytes and refuses them unless `sha256 == expected_hash`.
    pub fn new(
        wasm: Vec<u8>,
        expected_hash: [u8; 32],
        cpu_limit: u64,
        mem_limit: u64,
    ) -> Result<Self, LoadError> {
        let actual: [u8; 32] = Sha256::digest(&wasm).into();
        if actual != expected_hash {
            return Err(LoadError::HashMismatch {
                expected: expected_hash,
                actual,
            });
        }
        Ok(Self {
            wasm,
            wasm_hash: actual,
            cpu_limit,
            mem_limit,
            ledger_info: lane_ledger_info(),
        })
    }

    pub fn from_file(
        path: &Path,
        expected_hash: [u8; 32],
        cpu_limit: u64,
        mem_limit: u64,
    ) -> Result<Self, LoadError> {
        Self::new(
            std::fs::read(path).map_err(LoadError::Io)?,
            expected_hash,
            cpu_limit,
            mem_limit,
        )
    }

    pub fn wasm_hash(&self) -> [u8; 32] {
        self.wasm_hash
    }

    pub fn limits(&self) -> (u64, u64) {
        (self.cpu_limit, self.mem_limit)
    }

    /// The same executor with different ledger info. Only for the test that
    /// shows ledger info cannot affect output.
    pub fn with_ledger_info(mut self, info: LedgerInfo) -> Self {
        self.ledger_info = info;
        self
    }

    /// `genesis(config)`, with the consensus limits.
    pub fn genesis(&self, config: &[u8]) -> Result<(Vec<u8>, Metering), ExecError> {
        self.invoke("genesis", &[config])
    }

    /// `step(state, block)` → state and receipts, with the consensus limits.
    pub fn step(&self, state: &[u8], block: &[u8]) -> Result<(StepOutput, Metering), ExecError> {
        let (bytes, metering) = self.invoke("step", &[state, block])?;
        let env = StepEnvelope::decode(&bytes)
            .map_err(|e| ExecError::Host(format!("bad CVSTEP01 envelope: {e:?}")))?;
        Ok((
            StepOutput {
                state: env.state,
                receipts: env.receipts,
            },
            metering,
        ))
    }

    /// `step` with the host's per-cost-type budget table, for profiling.
    pub fn step_budget_report(&self, state: &[u8], block: &[u8]) -> String {
        let mut report = String::new();
        let _ = self.invoke_inner("step", &[state, block], Some(&mut report));
        report
    }

    /// Calls any function that takes `Bytes` arguments and returns metering only.
    /// For profiling contracts; consensus code uses `genesis` and `step`.
    pub fn meter(&self, func: &str, args: &[&[u8]]) -> Result<Metering, ExecError> {
        let host = Host::test_host_with_recording_footprint();
        let err = |e: HostError| ExecError::Host(format!("{e:?}"));
        host.set_ledger_info(self.ledger_info.clone())
            .map_err(err)?;
        host.as_budget().reset_unlimited().map_err(err)?;
        let contract = host
            .register_test_contract_wasm_from_source_account(
                &self.wasm,
                generate_account_id(&host),
                [9; 32],
            )
            .map_err(err)?;
        let vals = args
            .iter()
            .map(|a| host.bytes_new_from_slice(a).map(Val::from))
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        let argv = host.vec_new_from_slice(&vals).map_err(err)?;
        let symbol =
            Symbol::try_from_small_str(func).map_err(|_| ExecError::Host(func.to_string()))?;
        host.as_budget()
            .reset_limits(self.cpu_limit, self.mem_limit)
            .map_err(err)?;
        host.call(contract, symbol, argv).map_err(classify)?;
        Ok(Metering {
            cpu_insns: host.as_budget().get_cpu_insns_consumed().map_err(err)?,
            mem_bytes: host.as_budget().get_mem_bytes_consumed().map_err(err)?,
        })
    }

    fn invoke(&self, func: &str, args: &[&[u8]]) -> Result<(Vec<u8>, Metering), ExecError> {
        self.invoke_inner(func, args, None)
    }

    fn invoke_inner(
        &self,
        func: &str,
        args: &[&[u8]],
        report: Option<&mut String>,
    ) -> Result<(Vec<u8>, Metering), ExecError> {
        let host = Host::test_host_with_recording_footprint();
        let host_err = |what: &str, e: HostError| ExecError::Host(format!("{what}: {e:?}"));
        host.set_ledger_info(self.ledger_info.clone())
            .map_err(|e| host_err("ledger info", e))?;
        host.as_budget()
            .reset_unlimited()
            .map_err(|e| host_err("budget", e))?;
        let contract = host
            .register_test_contract_wasm_from_source_account(
                &self.wasm,
                generate_account_id(&host),
                [9; 32],
            )
            .map_err(|e| host_err("register", e))?;
        let vals = args
            .iter()
            .map(|a| host.bytes_new_from_slice(a).map(Val::from))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| host_err("args", e))?;
        let argv = host
            .vec_new_from_slice(&vals)
            .map_err(|e| host_err("args", e))?;
        let symbol = Symbol::try_from_small_str(func)
            .map_err(|_| ExecError::Host(format!("bad function name {func}")))?;

        // The consensus budget covers exactly the call.
        host.as_budget()
            .reset_limits(self.cpu_limit, self.mem_limit)
            .map_err(|e| ExecError::Host(format!("{e:?}")))?;
        let result = host.call(contract, symbol, argv);
        let metering = Metering {
            cpu_insns: host
                .as_budget()
                .get_cpu_insns_consumed()
                .unwrap_or(u64::MAX),
            mem_bytes: host
                .as_budget()
                .get_mem_bytes_consumed()
                .unwrap_or(u64::MAX),
        };
        if let Some(r) = report {
            *r = format!("{}", host.as_budget());
        }
        host.as_budget()
            .reset_unlimited()
            .map_err(|e| ExecError::Host(format!("{e:?}")))?;

        let val = result.map_err(classify)?;
        let obj = BytesObject::try_from(val)
            .map_err(|_| ExecError::Host(format!("{func} did not return bytes")))?;
        let len: u32 = host
            .bytes_len(obj)
            .map_err(|e| host_err("output", e))?
            .into();
        let mut out = vec![0u8; len as usize];
        host.bytes_copy_to_slice(obj, U32Val::from(0), &mut out)
            .map_err(|e| host_err("output", e))?;
        Ok((out, metering))
    }
}

fn classify(e: HostError) -> ExecError {
    let err = e.error;
    if err.is_type(ScErrorType::Contract) {
        return match u16::try_from(err.get_code()) {
            Ok(code) => ExecError::Fatal(code),
            Err(_) => ExecError::Host(format!("{err:?}")),
        };
    }
    if err.is_type(ScErrorType::Budget) && err.is_code(ScErrorCode::ExceededLimit) {
        return ExecError::BudgetExceeded;
    }
    if err.is_type(ScErrorType::Crypto) && err.is_code(ScErrorCode::InvalidInput) {
        return ExecError::InvalidSignature;
    }
    ExecError::Host(format!("{err:?}"))
}
