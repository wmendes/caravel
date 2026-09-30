//! Shared helpers: a payments config on the harness lane, transfers, and the
//! engine Wasm of record.

#![allow(dead_code)]

use std::path::PathBuf;

use caravel_app_sdk::crypto::native::DiagnosticCrypto;
use caravel_app_sdk::genesis::{MAX_EXEC_CPU, MAX_EXEC_MEM};
use caravel_app_sdk::{genesis, step, AppEngine, AppGenesisV1};
use caravel_core::tx::SigScheme;
use caravel_harness::{pk, Lane, USDC};
use caravel_payments::{Params, Payments, Transfer, TRANSFER};
use caravel_runtime::{ExecError, WasmExecutor};

pub const A: u8 = 0x41;
pub const B: u8 = 0x42;
pub const C: u8 = 0x43;
pub const K: u8 = 0x5E;
/// The treasury (system account 0).
pub const T: u8 = 0x22;
pub const FEE: i128 = USDC / 100;

pub fn config(fee: i128) -> AppGenesisV1 {
    let mut c = caravel_harness::config();
    c.template = Payments::TEMPLATE;
    c.system_keys = vec![pk(T)];
    c.app_params = Params {
        transfer_fee: fee,
        min_transfer: 1,
    }
    .encode();
    c
}

pub fn lane(fee: i128) -> Lane<Payments> {
    Lane::<Payments>::new(config(fee))
}

/// Queues a transfer from `from` (signed by `signer`) to the key of seed `to`.
pub fn transfer(l: &mut Lane<Payments>, signer: u8, from: u8, to: u8, amount: i128) {
    let body = Transfer {
        to: pk(to),
        amount,
        memo: 7,
    }
    .encode();
    l.tx_as(signer, from, SigScheme::RawEd25519, TRANSFER, body);
}

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

pub fn engine_hash() -> [u8; 32] {
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root().join("versions.json")).unwrap())
            .unwrap();
    let h = v["lanes"]["payments"]["engine_wasm_sha256"]
        .as_str()
        .unwrap();
    let raw: Vec<u8> = (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
        .collect();
    raw.try_into().unwrap()
}

/// The payments engine of record with the lane's execution limits.
pub fn wasm(cpu: u64, mem: u64) -> WasmExecutor {
    WasmExecutor::from_file(
        &root().join("target/contracts/payments_engine.wasm"),
        engine_hash(),
        cpu,
        mem,
    )
    .unwrap_or_else(|e| panic!("{e} (run ./scripts/build-contracts.sh first)"))
}

/// Replays `l`'s blocks through the engine Wasm from genesis and checks every
/// state and receipt byte against the native SDK (INV-P5).
pub fn check_wasm(l: &Lane<Payments>) {
    let w = wasm(l.config.exec_cpu_limit, l.config.exec_mem_limit);
    let cfg = l.config.encode().unwrap();
    let (mut state, _) = w.genesis(&cfg).unwrap();
    assert_eq!(
        state,
        genesis::<Payments>(&cfg, &DiagnosticCrypto).unwrap(),
        "genesis"
    );
    for (i, r) in l.records.iter().enumerate() {
        let native = step::<Payments, _>(&state, &r.input, &DiagnosticCrypto).unwrap();
        let (out, _) = w
            .step(&state, &r.input)
            .unwrap_or_else(|e| panic!("block {}: {e:?}", i + 1));
        assert!(out.state == native.state, "block {}: state differs", i + 1);
        assert!(
            out.receipts == native.receipts,
            "block {}: receipts differ",
            i + 1
        );
        assert_eq!(caravel_harness::sha256(&out.state), r.state_hash_after);
        state = out.state;
    }
    assert!(state == l.state_bytes);
}

/// Executes a block both ways from the lane's state: the same fatal code, and
/// the lane doesn't move.
pub fn fatal_both(l: &mut Lane<Payments>, bytes: &[u8]) -> u16 {
    let before = l.state_hash();
    let native = l.execute_bytes(bytes).map(|_| ()).unwrap_err();
    let w = wasm(l.config.exec_cpu_limit, l.config.exec_mem_limit);
    assert_eq!(
        w.step(&l.state_bytes, bytes).err(),
        Some(ExecError::Fatal(native.code))
    );
    assert_eq!(l.state_hash(), before);
    native.code
}

/// Genesis both ways: the fatal code if it fails (the same on both paths).
pub fn genesis_both(c: &AppGenesisV1) -> Option<u16> {
    let bytes = c.encode().unwrap();
    let native = genesis::<Payments>(&bytes, &DiagnosticCrypto);
    let w = wasm(MAX_EXEC_CPU, MAX_EXEC_MEM);
    match (native, w.genesis(&bytes)) {
        (Ok(n), Ok((s, _))) => {
            assert!(n == s);
            None
        }
        (Err(f), Err(e)) => {
            assert_eq!(e, ExecError::Fatal(f.code));
            Some(f.code)
        }
        (n, w) => panic!("genesis differs: native {n:?}, wasm {w:?}"),
    }
}
