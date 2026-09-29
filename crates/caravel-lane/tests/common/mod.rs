//! Shared test harness: the engine Wasm of record, a Wasm `Executor` for the
//! testkit, and a `Dual` executor that runs every call natively and in Wasm and
//! asserts identical bytes (INV-P5).

#![allow(dead_code)]

use std::path::PathBuf;

use caravel_lane::{ExecError, WasmExecutor};
use caravel_perps::{Fatal, StepOutput};
use caravel_testkit::{Executor, NativeExecutor};
use caravel_types::fatal;

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn versions() -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(root().join("versions.json")).unwrap()).unwrap()
}

pub fn unhex<const N: usize>(s: &str) -> [u8; N] {
    let raw: Vec<u8> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect();
    raw.try_into().expect("length")
}

/// The engine hash recorded in versions.json.
pub fn engine_hash() -> [u8; 32] {
    unhex(
        versions()["artifacts"]["engine_wasm_sha256"]
            .as_str()
            .unwrap(),
    )
}

pub fn wasm_path() -> PathBuf {
    std::env::var_os("CARAVEL_ENGINE_WASM")
        .map(PathBuf::from)
        .unwrap_or_else(|| root().join("target/contracts/perps_engine.wasm"))
}

/// The Wasm of record with the §10.3 consensus limits.
pub fn wasm() -> WasmExecutor {
    let cfg = caravel_testkit::lane::config();
    WasmExecutor::from_file(
        &wasm_path(),
        engine_hash(),
        cfg.exec_cpu_limit,
        cfg.exec_mem_limit,
    )
    .unwrap_or_else(|e| panic!("{e} (run ./scripts/build-contracts.sh first)"))
}

/// Maps an execution error to the fatal the native engine would report,
/// without the entry index (the Wasm path loses it).
pub fn as_fatal(e: ExecError) -> Fatal {
    Fatal::block(match e {
        ExecError::Fatal(code) => code,
        ExecError::InvalidSignature => fatal::BAD_SIGNATURE,
        ExecError::BudgetExceeded => panic!("budget exceeded"),
        ExecError::Host(msg) => panic!("host error: {msg}"),
    })
}

/// The consensus path as a testkit executor.
#[derive(Clone)]
pub struct Wasm(pub WasmExecutor);

impl Executor for Wasm {
    fn genesis(&self, config: &[u8]) -> Result<Vec<u8>, Fatal> {
        self.0.genesis(config).map(|(s, _)| s).map_err(as_fatal)
    }

    fn step(&self, state: &[u8], block: &[u8]) -> Result<StepOutput, Fatal> {
        self.0.step(state, block).map(|(o, _)| o).map_err(as_fatal)
    }

    fn reports_entry_index(&self) -> bool {
        false
    }
}

/// Runs both paths on every call and asserts byte-identical results.
#[derive(Clone)]
pub struct Dual {
    pub wasm: WasmExecutor,
    pub calls: std::rc::Rc<std::cell::Cell<u64>>,
    pub max_cpu: std::rc::Rc<std::cell::Cell<u64>>,
}

impl Dual {
    pub fn new() -> Self {
        Self {
            wasm: wasm(),
            calls: Default::default(),
            max_cpu: Default::default(),
        }
    }
}

impl Executor for Dual {
    fn genesis(&self, config: &[u8]) -> Result<Vec<u8>, Fatal> {
        let native = NativeExecutor.genesis(config);
        let wasm = self.wasm.genesis(config).map(|(s, _)| s).map_err(as_fatal);
        assert_eq!(
            native.clone().map_err(|f| f.code),
            wasm.map_err(|f| f.code),
            "genesis parity"
        );
        native
    }

    fn step(&self, state: &[u8], block: &[u8]) -> Result<StepOutput, Fatal> {
        let native = NativeExecutor.step(state, block);
        let wasm = self.wasm.step(state, block);
        self.calls.set(self.calls.get() + 1);
        match (&native, wasm) {
            (Ok(n), Ok((w, m))) => {
                assert!(
                    n.state == w.state,
                    "INV-P5: state bytes differ at call {}",
                    self.calls.get()
                );
                assert!(
                    n.receipts == w.receipts,
                    "INV-P5: receipts differ at call {}",
                    self.calls.get()
                );
                self.max_cpu.set(self.max_cpu.get().max(m.cpu_insns));
            }
            (Err(f), Err(e)) => assert_eq!(f.code, as_fatal(e).code, "INV-P5: fatal codes differ"),
            (n, w) => panic!(
                "INV-P5: native {:?} vs wasm {:?}",
                n.as_ref().map(|_| "ok"),
                w.map(|_| "ok")
            ),
        }
        native
    }

    fn reports_entry_index(&self) -> bool {
        true
    }
}
