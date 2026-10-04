//! The payments parity gate (INV-P5, spec §20.4.4): the engine Wasm run
//! through `soroban-env-host` gives the same state and receipt bytes as the
//! native SDK on random workloads, and a full block stays inside the lane's
//! execution budget. `scenarios.rs` replays P1–P12 the same way.
//!
//! `cargo test --locked -p caravel-payments-node --test parity -- --ignored`
//! runs the 10,000-block gate.

mod common;

use std::collections::BTreeSet;

use caravel_core::receipts::PSEUDO_BLOCK_START;
use caravel_core::tx::{SigScheme, StandardBody};
use caravel_harness::{pk, Lane, USDC};
use caravel_payments::{Payments, Transfer, PERM_TRANSFER, TRANSFER};
use caravel_runtime::executor::LoadError;
use caravel_runtime::{ExecError, Metering, WasmExecutor};
use common::*;

#[test]
fn the_wasm_file_is_the_one_of_record() {
    assert_eq!(wasm(1, 1).wasm_hash(), engine_hash());
    let err = WasmExecutor::from_file(
        &root().join("target/contracts/payments_engine.wasm"),
        [0; 32],
        1,
        1,
    )
    .err()
    .expect("refused");
    assert!(matches!(err, LoadError::HashMismatch { .. }), "{err}");
}

/// xorshift64*: a fixed, seedable workload.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) % n
    }

    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.below(xs.len() as u64) as usize]
    }
}

/// Twelve users on a lane of ten accounts, so deposits bounce and slots are reused.
const USERS: [u8; 12] = [
    0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x4B, 0x4C,
];

fn session(u: u8) -> u8 {
    u + 0x40
}

fn amount(r: &mut Rng) -> i128 {
    match r.below(10) {
        0 => 0,
        1 => -1,
        2 => i128::MAX,
        3 => FEE,
        _ => (r.below(4_000) as i128 + 1) * USDC / 100,
    }
}

/// Queues one block of random entries.
fn random_block(l: &mut Lane<Payments>, r: &mut Rng) {
    for _ in 0..r.below(12) {
        let u = r.pick(&USERS);
        match r.below(20) {
            0..=2 => {
                l.deposit(u, (r.below(50) as i128 + 1) * USDC);
            }
            3..=12 => {
                let to = if r.below(12) == 0 { T } else { r.pick(&USERS) };
                let signer = if r.below(4) == 0 { session(u) } else { u };
                let a = amount(r);
                transfer(l, signer, u, to, a);
            }
            13 | 14 => {
                let a = amount(r);
                l.withdraw(u, a);
            }
            15 => {
                let a = amount(r).max(0);
                l.forced_withdrawal(u, a);
            }
            16 => {
                let expires_at_ms = l.now + r.below(120_000) + 1;
                let permissions = r.pick(&[PERM_TRANSFER, 0, 0x02]);
                l.standard(
                    u,
                    StandardBody::AddSessionKey {
                        session_key: pk(session(u)),
                        expires_at_ms,
                        permissions,
                    },
                );
            }
            17 => {
                l.standard(
                    u,
                    StandardBody::RevokeSessionKey {
                        session_key: pk(session(u)),
                    },
                );
            }
            18 => {
                // Owner-signed with SEP-53, as a wallet without raw signing would.
                let body = Transfer {
                    to: pk(r.pick(&USERS)),
                    amount: amount(r),
                    memo: r.below(1 << 40),
                }
                .encode();
                l.tx_as(u, u, SigScheme::Sep53, TRANSFER, body);
            }
            _ => l.advance(r.below(30_000)),
        }
    }
}

/// Runs `blocks` random blocks natively and through the Wasm, comparing every
/// byte; returns the receipt codes seen.
fn random_parity(seed: u64, blocks: u64, fee: i128) -> BTreeSet<u16> {
    let mut c = config(fee);
    c.max_accounts = 10;
    let w = wasm(c.exec_cpu_limit, c.exec_mem_limit);
    let mut l = Lane::<Payments>::new(c);
    let (genesis, _) = w.genesis(&l.config.encode().unwrap()).unwrap();
    assert!(genesis == l.state_bytes);
    let mut r = Rng(seed);
    let mut seen = BTreeSet::new();
    for h in 1..=blocks {
        random_block(&mut l, &mut r);
        let prev = l.state_bytes.clone();
        if h % 10 == 0 {
            l.checkpoint();
        } else {
            l.block();
        }
        let input = &l.records.last().unwrap().input;
        let (out, _) = w
            .step(&prev, input)
            .unwrap_or_else(|e| panic!("seed {seed} block {h}: {e:?}"));
        assert!(
            out.state == l.state_bytes,
            "seed {seed} block {h}: state differs"
        );
        assert!(
            out.receipts == l.receipts.encode().unwrap(),
            "seed {seed} block {h}: receipts differ"
        );
        seen.extend(
            l.receipts
                .receipts
                .iter()
                .filter(|x| x.entry_index < PSEUDO_BLOCK_START)
                .map(|x| x.code),
        );
    }
    seen
}

#[test]
fn random_workloads_match_native() {
    let mut seen = BTreeSet::new();
    for (seed, fee) in [(1, FEE), (2, 0), (3, FEE)] {
        seen.extend(random_parity(seed, 100, fee));
    }
    // The workload reaches every payments code and the common platform ones.
    use caravel_core::codes::receipt as p;
    use caravel_payments::codes as c;
    for code in [
        p::OK,
        p::UNKNOWN_ACCOUNT,
        p::UNAUTHORIZED_SIGNER,
        p::BELOW_MIN_WITHDRAWAL,
        p::INSUFFICIENT_FREE_BALANCE,
        p::BAD_SESSION_KEY,
        c::INSUFFICIENT_BALANCE,
        c::UNKNOWN_RECIPIENT,
        c::BELOW_MIN_TRANSFER,
        c::SELF_TRANSFER,
    ] {
        assert!(seen.contains(&code), "code {code} never seen: {seen:?}");
    }
}

/// The parity gate: 10,000 blocks (INV-P5), as 16 seeded lanes of 625 blocks
/// run on every core (F-04), like the perps gate's 50 × 200.
#[test]
#[ignore = "10,000 blocks; run with --ignored"]
fn ten_thousand_blocks_match_native() {
    const LANES: u64 = 16;
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(LANES as usize) as u64;
    std::thread::scope(|s| {
        for t in 0..threads {
            s.spawn(move || {
                for seed in (t..LANES).step_by(threads as usize) {
                    random_parity(0x5EED_CAFE + seed, 10_000 / LANES, FEE);
                }
            });
        }
    });
    eprintln!("parity: 10,000 blocks on {threads} threads");
}

// --- Budget --------------------------------------------------------------------

/// The costliest block the local lane's limits allow: a checkpoint over all
/// 256 accounts and a full withdrawal queue (512), filled to `max_block_bytes`
/// with SEP-53 transfers. Returns the lane (at the state before it) and the block.
fn worst_block() -> (Lane<Payments>, Vec<u8>) {
    let c = config(FEE);
    assert_eq!(
        (c.max_accounts, c.max_pending_withdrawals, c.max_block_bytes),
        (256, 512, 12_000)
    );
    let mut l = Lane::<Payments>::new(c);
    let users: Vec<u8> = (0..=255).filter(|&u| u != T).collect();
    for chunk in users.chunks(100) {
        for &u in chunk {
            l.deposit(u, 1_000 * USDC);
        }
        l.block();
        l.all_ok();
    }
    assert_eq!(l.state().accounts.len(), 256);
    // 512 pending withdrawals, a block at a time.
    let mut n = 0;
    while n < 512 {
        let before = l.records.len();
        for &u in users.iter().cycle().skip(n).take(40) {
            if n == 512 {
                break;
            }
            l.withdraw(u, USDC);
            n += 1;
        }
        l.block();
        l.all_ok();
        assert_eq!(l.records.len(), before + 1);
    }
    assert_eq!(l.state().pending.len(), 512);
    // As many transfers as fit, each from another user, so dropping the last
    // ones leaves every nonce in order.
    for k in 0..80 {
        let (u, to) = (users[k], users[k + 1]);
        let body = Transfer {
            to: pk(to),
            amount: USDC,
            memo: u64::MAX,
        }
        .encode();
        l.tx_as(u, u, SigScheme::Sep53, TRANSFER, body);
    }
    let mut block = l.build(true);
    let max = l.config.max_block_bytes as usize;
    assert!(
        block.encode().unwrap().len() > max,
        "80 transfers should overflow a block"
    );
    while block.encode().unwrap().len() > max {
        block.entries.pop();
    }
    (l, block.encode().unwrap())
}

fn measure(l: &Lane<Payments>, bytes: &[u8]) -> Metering {
    let w = wasm(l.config.exec_cpu_limit, l.config.exec_mem_limit);
    let (out, m) = w
        .step(&l.state_bytes, bytes)
        .expect("the worst block runs within the lane's limits");
    let native = caravel_app_sdk::step::<Payments, _>(
        &l.state_bytes,
        bytes,
        &caravel_app_sdk::crypto::native::DiagnosticCrypto,
    )
    .unwrap();
    assert!(out.state == native.state && out.receipts == native.receipts);
    m
}

#[test]
fn a_full_block_fits_the_budget() {
    let (l, bytes) = worst_block();
    let m = measure(&l, &bytes);
    let (cpu, mem) = (l.config.exec_cpu_limit, l.config.exec_mem_limit);
    let transfers = caravel_core::block::BlockInputV1::decode(&bytes)
        .unwrap()
        .entries
        .len();
    eprintln!(
        "worst payments block: {transfers} SEP-53 transfers, {} bytes, cpu {} / {cpu} ({}%), mem {} / {mem} ({}%)",
        bytes.len(),
        m.cpu_insns,
        m.cpu_insns * 100 / cpu,
        m.mem_bytes,
        m.mem_bytes * 100 / mem,
    );
    // ≤ 100M instructions at full caps (spec §12.2), and 2× headroom under the limit (DEC-028).
    assert!(m.cpu_insns <= 100_000_000, "cpu {} > 100M", m.cpu_insns);
    assert!(m.cpu_insns <= cpu / 2, "cpu {} > {}", m.cpu_insns, cpu / 2);
    assert!(m.mem_bytes <= mem / 2, "mem {} > {}", m.mem_bytes, mem / 2);
}

/// Budget exhaustion is fatal and deterministic (DEC-015): the measured amount
/// is enough, one instruction less never is.
#[test]
fn budget_exhaustion_is_deterministic() {
    let (l, bytes) = worst_block();
    let m = measure(&l, &bytes);
    let mem = l.config.exec_mem_limit;
    let exact = wasm(m.cpu_insns, mem);
    assert!(exact.step(&l.state_bytes, &bytes).is_ok());
    let short = wasm(m.cpu_insns - 1, mem);
    for _ in 0..3 {
        assert_eq!(
            short.step(&l.state_bytes, &bytes).err(),
            Some(ExecError::BudgetExceeded)
        );
    }
}
