//! The parity gate (spec §8.2, INV-P5, T-005): the engine Wasm run through
//! `soroban-env-host` produces the same state and receipt bytes as the native
//! engine on every scenario, on random workloads and on the ed25519 edge cases.

mod common;

use caravel_runtime::executor::{lane_ledger_info, LoadError};
use caravel_runtime::{ExecError, WasmExecutor};
use caravel_testkit::lane::seeds::A;
use caravel_testkit::lane::{config, BTC, BTC_PRICE, TICK, USDC};
use caravel_testkit::random::{drive, Rng};
use caravel_testkit::scenarios::{run, NAMES};
use caravel_testkit::{Lane, NativeExecutor};
use caravel_types::fatal;
use caravel_types::tx::{LaneTxV1, Side, SigScheme, Tif, TxBody, ALL_MARKETS};
use common::*;
use soroban_env_host::LedgerInfo;

#[test]
fn the_wasm_file_is_the_one_of_record() {
    assert_eq!(wasm().wasm_hash(), engine_hash());
}

#[test]
fn a_wasm_with_another_hash_is_refused() {
    let err = WasmExecutor::from_file(&wasm_path(), [0; 32], 1, 1)
        .err()
        .expect("refused");
    assert!(matches!(err, LoadError::HashMismatch { .. }), "{err}");
}

#[test]
fn genesis_matches_native() {
    let cfg = config().encode().unwrap();
    let (state, _) = wasm().genesis(&cfg).unwrap();
    assert_eq!(state, NativeExecutor_genesis(&cfg));
}

#[allow(non_snake_case)]
fn NativeExecutor_genesis(cfg: &[u8]) -> Vec<u8> {
    use caravel_testkit::Executor;
    NativeExecutor.genesis(cfg).unwrap()
}

/// Every scenario, block for block through both paths, and again through the
/// Wasm path alone (which cannot report entry indexes).
#[test]
fn every_scenario_matches_native_block_for_block() {
    let dual = Dual::new();
    let wasm_only = Wasm(wasm());
    for name in NAMES {
        let native = run(name, &NativeExecutor).state_hashes;
        assert_eq!(run(name, &dual).state_hashes, native, "{name} (dual)");
        assert_eq!(
            run(name, &wasm_only).state_hashes,
            native,
            "{name} (wasm only)"
        );
    }
    assert!(
        dual.calls.get() > 80,
        "every scenario block ran through both paths"
    );
}

/// 1,000 random blocks (5 seeded lanes × 200 blocks), compared at every block.
#[test]
fn a_thousand_random_blocks_match_native() {
    random_parity(1..=5, 200);
}

/// The full T-005 gate: 10,000 random blocks (50 lanes × 200). Run with
/// `cargo test --release -p caravel-runtime --test parity -- --ignored`.
#[test]
#[ignore = "long: the 10,000-block parity gate, run in CI"]
fn ten_thousand_random_blocks_match_native() {
    random_parity(1..=50, 200);
}

fn random_parity(seeds: std::ops::RangeInclusive<u64>, blocks: usize) {
    let dual = Dual::new();
    for seed in seeds {
        let mut lane = Lane::new(dual.clone(), config());
        drive(&mut lane, &mut Rng::new(seed), blocks);
        lane.checkpoint();
    }
    eprintln!(
        "parity: {} blocks, max host cpu per block {} insns",
        dual.calls.get(),
        dual.max_cpu.get()
    );
}

/// The engine never reads ledger info, so two very different ledger infos give
/// identical output and identical metering.
#[test]
fn ledger_info_does_not_affect_output() {
    let mut lane = Lane::native(config());
    drive(&mut lane, &mut Rng::new(7), 30);
    let rec = lane.records.last().unwrap().clone();
    let prev = {
        let mut l = Lane::native(config());
        for r in &lane.records[..lane.records.len() - 1] {
            l.execute_bytes(&r.input).unwrap();
        }
        l.state_bytes
    };
    let other = LedgerInfo {
        protocol_version: 28,
        sequence_number: 987_654,
        timestamp: 1_790_000_000,
        network_id: [7; 32],
        base_reserve: 1,
        min_persistent_entry_ttl: 10,
        min_temp_entry_ttl: 10,
        max_entry_ttl: 100_000,
    };
    assert_ne!(other, lane_ledger_info());
    let a = wasm().step(&prev, &rec.input).unwrap();
    let b = wasm()
        .with_ledger_info(other)
        .step(&prev, &rec.input)
        .unwrap();
    assert_eq!(a.0, b.0, "output");
    assert_eq!(a.1, b.1, "metering");
    assert_eq!(
        caravel_types::vectors::sha256(&a.0.state),
        rec.state_hash_after
    );
}

/// §8.4: the host and the native engine agree on the non-canonical `S + L`
/// signature and on the small-order key that lax verification accepts.
#[test]
#[allow(clippy::needless_range_loop)] // byte-wise add with carry
fn host_and_native_agree_on_ed25519_edge_cases() {
    let mut identity = [0u8; 32];
    identity[0] = 1;
    let mut lane = Lane::new(Dual::new(), config());
    lane.deposit(A, 100 * USDC);
    lane.deposit_key(identity, 100 * USDC);
    lane.oracle(BTC, BTC_PRICE);
    lane.block();

    // Small-order key: R = identity, S = 0 passes a cofactorless check for any message.
    let mut small_sig = [0u8; 64];
    small_sig[0] = 1;
    let nonce = lane
        .state()
        .accounts
        .iter()
        .find(|a| a.key == identity)
        .unwrap()
        .next_nonce;
    let tx = LaneTxV1 {
        lane_id: lane.config.lane_id,
        account: identity,
        signer: identity,
        nonce,
        expiry_ms: u64::MAX,
        sig_scheme: SigScheme::RawEd25519,
        body: TxBody::CancelAll {
            market_id: ALL_MARKETS,
        },
        signature: small_sig,
    };
    lane.push_tx(tx);
    let r = lane.try_block(false).err();
    lane.expect_fatal(r, fatal::BAD_SIGNATURE, 0);

    // Malleable signature: S' = S + L.
    let mut tx = lane.signed(
        A,
        A,
        SigScheme::RawEd25519,
        lane.account(A).unwrap().next_nonce,
        u64::MAX,
        TxBody::CancelAll {
            market_id: ALL_MARKETS,
        },
    );
    let l: [u8; 32] = unhex("edd3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010");
    let mut carry = 0u16;
    for i in 0..32 {
        let sum = u16::from(tx.signature[32 + i]) + u16::from(l[i]) + carry;
        tx.signature[32 + i] = (sum & 0xFF) as u8;
        carry = sum >> 8;
    }
    lane.push_tx(tx);
    let r = lane.try_block(false).err();
    lane.expect_fatal(r, fatal::BAD_SIGNATURE, 0);

    // And a valid transaction goes through on both paths.
    lane.order(A, BTC, Side::Buy, Tif::Gtc, BTC_PRICE - TICK, 1);
    lane.block();
    lane.all_ok();
}

/// Budget exhaustion is fatal and deterministic: the same block fails the same
/// way every time, on every node (DEC-015).
#[test]
fn budget_exhaustion_is_deterministic() {
    let mut lane = Lane::native(config());
    drive(&mut lane, &mut Rng::new(3), 10);
    let rec = lane.records.last().unwrap();
    let prev = {
        let mut l = Lane::native(config());
        for r in &lane.records[..lane.records.len() - 1] {
            l.execute_bytes(&r.input).unwrap();
        }
        l.state_bytes
    };
    let (_, full) = wasm().step(&prev, &rec.input).unwrap();
    let tight = WasmExecutor::from_file(
        &wasm_path(),
        engine_hash(),
        full.cpu_insns / 2,
        config().exec_mem_limit,
    )
    .unwrap();
    for _ in 0..3 {
        assert_eq!(
            tight.step(&prev, &rec.input).err(),
            Some(ExecError::BudgetExceeded)
        );
    }
    // Exactly the measured amount is enough; one instruction less is not.
    let exact = WasmExecutor::from_file(
        &wasm_path(),
        engine_hash(),
        full.cpu_insns,
        config().exec_mem_limit,
    )
    .unwrap();
    assert!(exact.step(&prev, &rec.input).is_ok());
    let short = WasmExecutor::from_file(
        &wasm_path(),
        engine_hash(),
        full.cpu_insns - 1,
        config().exec_mem_limit,
    )
    .unwrap();
    assert_eq!(
        short.step(&prev, &rec.input).err(),
        Some(ExecError::BudgetExceeded)
    );
}
