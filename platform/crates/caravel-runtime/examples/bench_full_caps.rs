//! Host metering of `step` at full caps (spec §12.2, T-005): 1,024 accounts,
//! 3 markets × 2 sides × 256 resting orders, and the heaviest block shapes the
//! caps allow (256 entries, 24,000 bytes). Every block also runs natively and
//! must match byte for byte.
//!
//! `cargo run --release -p caravel-runtime --example bench_full_caps [wasm]`

use caravel_perps::native::DiagnosticCrypto;
use caravel_runtime::WasmExecutor;
use caravel_types::block::{BlockInputV1, Entry};
use caravel_types::inbox::{InboxKind, InboxMsgV1};
use caravel_types::oracle::OracleUpdateV1;
use caravel_types::state::{
    AccountV1, MarketStateV1, OrderV1, PendingWithdrawalV1, PositionV1, SessionKeyV1, StateV1,
};
use caravel_types::tx::{LaneTxV1, PlaceOrder, Side, SigScheme, Tif, TxBody};
use caravel_types::vectors::{config, key, sha256};
use ed25519_dalek::{Signer, SigningKey};

const T: u64 = 1_790_000_000_000;
const USDC: i128 = 10_000_000;
const PRICES: [i64; 3] = [65_000_000, 35_000_000, 40_000_000];
const TICK: i64 = 1000;

fn user_key(i: u32) -> SigningKey {
    let mut seed = b"caravel-bench-user".to_vec();
    seed.extend_from_slice(&i.to_le_bytes());
    SigningKey::from_bytes(&sha256(&seed))
}

/// A full-caps state. Users alternate long and short so positions sum to zero;
/// `levered` users are long with thin collateral, so a shock liquidates them.
fn caps() -> (u32, u32, u32) {
    let get = |k: &str, d: u32| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    (
        get("BENCH_ACCOUNTS", 1024),
        get("BENCH_ORDERS_PER_SIDE", 256),
        get("BENCH_BLOCK_BYTES", 24_000),
    )
}

#[allow(clippy::needless_range_loop)] // parallel arrays indexed by market
fn full_state(levered: bool) -> (StateV1, Vec<SigningKey>) {
    let (n_accounts, levels, block_bytes) = caps();
    let users = n_accounts - 2;
    let mut cfg = config();
    cfg.max_accounts = n_accounts;
    cfg.max_orders_per_side = levels;
    cfg.max_block_bytes = block_bytes;
    let n_markets = cfg.markets.len();
    let config_hash = sha256(&cfg.encode().unwrap());
    let keys: Vec<SigningKey> = (0..users).map(user_key).collect();
    let mut accounts = vec![
        AccountV1 {
            key: cfg.backstop_key,
            system: true,
            next_nonce: 0,
            collateral: 1_000_000 * USDC,
            open_order_count: 0,
            session_keys: vec![],
            positions: vec![PositionV1::default(); n_markets],
            txs_this_block: 0,
        },
        AccountV1 {
            key: cfg.treasury_key,
            system: true,
            next_nonce: 0,
            collateral: 0,
            open_order_count: 0,
            session_keys: vec![],
            positions: vec![PositionV1::default(); n_markets],
            txs_this_block: 0,
        },
    ];
    for (i, k) in keys.iter().enumerate() {
        let long = i % 2 == 0;
        let lots: i64 = if levered { 1000 } else { 5 };
        let s = if long { lots } else { -lots };
        let positions = (0..n_markets)
            .map(|m| PositionV1 {
                lots: s,
                cost_basis: i128::from(s) * i128::from(PRICES[m]),
            })
            .collect();
        let collateral = if levered && long {
            1_900 * USDC
        } else {
            20_000 * USDC
        };
        accounts.push(AccountV1 {
            key: k.verifying_key().to_bytes(),
            system: false,
            next_nonce: 1,
            collateral,
            open_order_count: 0,
            session_keys: vec![SessionKeyV1 {
                key: sha256(&k.verifying_key().to_bytes()),
                expires_at_ms: T + 86_400_000,
                permissions: 3,
            }],
            positions,
            txs_this_block: 0,
        });
    }
    let mut markets = Vec::new();
    for m in 0..n_markets {
        let mut bids = Vec::new();
        let mut asks = Vec::new();
        for level in 0..levels {
            let bid_owner = 2 + (m as u32 * 2 * levels + 2 * level) % users;
            let ask_owner = 2 + (m as u32 * 2 * levels + 2 * level + 1) % users;
            bids.push(OrderV1 {
                order_id: u64::from(m as u32 * 1000 + 2 * level + 1),
                account_index: bid_owner,
                price: PRICES[m] - TICK * i64::from(level + 1),
                lots_remaining: 10,
                client_order_id: 0,
            });
            asks.push(OrderV1 {
                order_id: u64::from(m as u32 * 1000 + 2 * level + 2),
                account_index: ask_owner,
                price: PRICES[m] + TICK * i64::from(level + 1),
                lots_remaining: 10,
                client_order_id: 0,
            });
        }
        for o in bids.iter().chain(&asks) {
            accounts[o.account_index as usize].open_order_count += 1;
        }
        let oi = accounts
            .iter()
            .map(|a| i64::from(a.positions[m].lots.max(0) as i32))
            .sum();
        markets.push(MarketStateV1 {
            oracle_price: PRICES[m],
            oracle_time_ms: T - 1000,
            last_funding_time_ms: T - 3_600_000,
            cumulative_funding_per_lot: 0,
            open_interest_lots: oi,
            bids,
            asks,
        });
    }
    // 511 pending withdrawals, so a checkpoint commits a near-full queue.
    let pending: Vec<PendingWithdrawalV1> = (0..511.min(users as usize))
        .map(|i| PendingWithdrawalV1 {
            key: accounts[2 + i].key,
            amount: USDC,
        })
        .collect();
    let deposits = accounts
        .iter()
        .map(|a| a.collateral - a.positions.iter().map(|p| p.cost_basis).sum::<i128>())
        .sum::<i128>()
        + pending.iter().map(|p| p.amount).sum::<i128>();
    let st = StateV1 {
        lane_id: cfg.lane_id,
        config_hash,
        height: 100,
        last_block_input_hash: [3; 32],
        last_timestamp_ms: T - 1000,
        checkpoint_seq: 9,
        inbox_through: 0,
        inbox_acc: [0; 32],
        next_order_id: 10_000,
        deposits_credited_total: deposits,
        withdrawals_committed_total: 0,
        backstop_deficit: false,
        accounts,
        markets,
        pending,
        last_commitment: Default::default(),
        config: cfg,
    };
    caravel_perps::invariants::check(&st).expect("benchmark state satisfies the invariants");
    (st, keys)
}

fn oracle(m: usize, price: i64) -> Entry {
    let cfg = config();
    let mut u = OracleUpdateV1 {
        market_id: cfg.markets[m].market_id,
        price,
        publish_time_ms: T,
        oracle_key: cfg.oracle_keys[0],
        signature: [0; 64],
    };
    u.signature = key(0x31)
        .sign(&sha256(&u.signing_preimage(&cfg.lane_id)))
        .to_bytes();
    Entry::Oracle(u)
}

fn user_tx(st: &StateV1, k: &SigningKey, nonce: u64, body: TxBody) -> Entry {
    let mut tx = LaneTxV1 {
        lane_id: st.lane_id,
        account: k.verifying_key().to_bytes(),
        signer: k.verifying_key().to_bytes(),
        nonce,
        expiry_ms: u64::MAX,
        sig_scheme: SigScheme::RawEd25519,
        body,
        signature: [0; 64],
    };
    tx.signature = k
        .sign(&sha256(&tx.tx_hash_preimage(&st.config_hash)))
        .to_bytes();
    Entry::User(tx)
}

fn block(st: &StateV1, state_bytes: &[u8], checkpoint_end: bool, entries: Vec<Entry>) -> Vec<u8> {
    let prev = sha256(&caravel_types::block::block_hash_preimage(
        &st.last_block_input_hash,
        &sha256(state_bytes),
    ));
    BlockInputV1 {
        lane_id: st.lane_id,
        height: st.height + 1,
        timestamp_ms: T,
        prev_block_hash: prev,
        checkpoint_end,
        entries,
    }
    .encode()
    .unwrap()
}

/// Adds user entries while the block stays within `max_block_bytes`.
fn fill_block(
    st: &StateV1,
    state_bytes: &[u8],
    mut entries: Vec<Entry>,
    mut next: impl FnMut(usize) -> Entry,
) -> Vec<Entry> {
    for i in 0.. {
        let e = next(i);
        entries.push(e);
        if block(st, state_bytes, false, entries.clone()).len() > st.config.max_block_bytes as usize
            || entries.len() > 256
        {
            entries.pop();
            break;
        }
    }
    entries
}

/// Empty-block cost for state shapes that isolate each term.
fn breakdown(exec: &WasmExecutor) {
    println!("| State shape (empty block) | Accounts | Orders | State bytes | Host CPU insns |");
    println!("|---|---:|---:|---:|---:|");
    for (name, keep_accounts, keep_orders, keep_positions) in [
        ("genesis-like: 2 system accounts", 2usize, false, false),
        (
            "1,024 accounts, no positions, no orders",
            1024,
            false,
            false,
        ),
        (
            "1,024 accounts with positions, no orders",
            1024,
            false,
            true,
        ),
        (
            "1,024 accounts, 1,536 orders, no positions",
            1024,
            true,
            false,
        ),
        ("full: positions and orders", 1024, true, true),
        ("256 accounts with positions and orders", 256, true, true),
    ] {
        let (mut st, _) = full_state(false);
        st.pending.clear();
        st.accounts.truncate(keep_accounts);
        for a in &mut st.accounts {
            if !keep_positions {
                a.positions
                    .iter_mut()
                    .for_each(|p| *p = PositionV1::default());
            }
            a.open_order_count = 0;
        }
        for mk in &mut st.markets {
            if !keep_orders {
                mk.bids.clear();
                mk.asks.clear();
            }
            mk.bids
                .retain(|o| (o.account_index as usize) < keep_accounts);
            mk.asks
                .retain(|o| (o.account_index as usize) < keep_accounts);
            for o in mk.bids.iter().chain(&mk.asks) {
                st.accounts[o.account_index as usize].open_order_count += 1;
            }
        }
        // Rebalance so positions sum to zero after truncation.
        if st.accounts.len() % 2 == 1 {
            st.accounts.pop();
        }
        for m in 0..st.markets.len() {
            st.markets[m].open_interest_lots =
                st.accounts.iter().map(|a| a.positions[m].lots.max(0)).sum();
        }
        st.deposits_credited_total = st
            .accounts
            .iter()
            .map(|a| a.collateral - a.positions.iter().map(|p| p.cost_basis).sum::<i128>())
            .sum();
        let bytes = st.encode().unwrap();
        let blk = block(&st, &bytes, false, vec![]);
        let orders: usize = st.markets.iter().map(|m| m.bids.len() + m.asks.len()).sum();
        match exec.step(&bytes, &blk) {
            Ok((_, m)) => println!(
                "| {name} | {} | {orders} | {} | {} |",
                st.accounts.len(),
                bytes.len(),
                m.cpu_insns
            ),
            Err(e) => println!(
                "| {name} | {} | {orders} | {} | {e:?} |",
                st.accounts.len(),
                bytes.len()
            ),
        }
    }
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../target/contracts/perps_engine.wasm"
        )
        .to_string()
    });
    let wasm = std::fs::read(&path).expect("engine wasm (run ./scripts/build-contracts.sh)");
    let mut cfg = config();
    let (a, o, b) = caps();
    (
        cfg.max_accounts,
        cfg.max_orders_per_side,
        cfg.max_block_bytes,
    ) = (a, o, b);
    // Measure without limits, then compare with the target and the lane limit.
    let exec = WasmExecutor::new(wasm.clone(), sha256(&wasm), u64::MAX, u64::MAX).unwrap();

    if let Some(profile_wasm) = std::env::var_os("PHASES") {
        let w = std::fs::read(profile_wasm).unwrap();
        let p = WasmExecutor::new(w.clone(), sha256(&w), u64::MAX, u64::MAX).unwrap();
        let (st, _) = full_state(false);
        let bytes = st.encode().unwrap();
        println!(
            "| Phase at full caps ({} state bytes) | Host CPU insns |\n|---|---:|",
            bytes.len()
        );
        for f in ["copy_in", "decode", "dec_enc", "equity", "margin"] {
            println!("| {f} | {} |", p.meter(f, &[&bytes]).unwrap().cpu_insns);
        }
        return;
    }
    if std::env::var_os("PROFILE").is_some() {
        let (st, _) = full_state(false);
        let bytes = st.encode().unwrap();
        println!(
            "{}",
            exec.step_budget_report(&bytes, &block(&st, &bytes, false, vec![]))
        );
        return;
    }
    if std::env::var_os("BREAKDOWN").is_some() {
        breakdown(&exec);
        return;
    }
    let oracles = |shock: bool| {
        (0..3)
            .map(|m| {
                oracle(
                    m,
                    if shock {
                        PRICES[m] / 100 * 91 / 10 * 10
                    } else {
                        PRICES[m]
                    },
                )
            })
            .collect::<Vec<_>>()
    };
    let mut rows = Vec::new();
    for (name, levered) in [
        ("empty block", false),
        ("3 oracle updates + funding on 3 markets", false),
        ("deposits up to the entry and byte caps", false),
        ("GTC orders that rest-reject (book full), max bytes", false),
        ("IOC takers sweeping 3 levels each, max bytes", false),
        (
            "CHECKPOINT_END: every account leaf + a near-full withdrawal queue",
            false,
        ),
        (
            "−9% shock: half the accounts (levered longs) liquidated",
            true,
        ),
    ] {
        let (st, keys) = full_state(levered);
        let state_bytes = st.encode().unwrap();
        let mut checkpoint_end = false;
        let entries = match name {
            "empty block" => vec![],
            "3 oracle updates + funding on 3 markets" => oracles(false),
            "deposits up to the entry and byte caps" => {
                fill_block(&st, &state_bytes, vec![], |i| {
                    Entry::Inbox(InboxMsgV1 {
                        kind: InboxKind::Deposit,
                        index: i as u64,
                        lane_account: st.accounts[2 + (i % keys.len())].key,
                        amount: USDC,
                        enqueued_at: 0,
                    })
                })
            }
            "GTC orders that rest-reject (book full), max bytes" => {
                fill_block(&st, &state_bytes, oracles(false), |i| {
                    let k = &keys[i % keys.len()];
                    user_tx(
                        &st,
                        k,
                        1 + (i / keys.len()) as u64,
                        TxBody::PlaceOrder(PlaceOrder {
                            market_id: 1 + (i % 3) as u16,
                            side: Side::Buy,
                            tif: Tif::Gtc,
                            reduce_only: false,
                            price: PRICES[i % 3] - 300 * TICK,
                            lots: 1,
                            client_order_id: 0,
                        }),
                    )
                })
            }
            "IOC takers sweeping 3 levels each, max bytes" => {
                fill_block(&st, &state_bytes, oracles(false), |i| {
                    let k = &keys[(i * 7 + 3) % keys.len()];
                    user_tx(
                        &st,
                        k,
                        1,
                        TxBody::PlaceOrder(PlaceOrder {
                            market_id: 1 + (i % 3) as u16,
                            side: Side::Buy,
                            tif: Tif::Ioc,
                            reduce_only: false,
                            price: PRICES[i % 3] + 256 * TICK,
                            lots: 30,
                            client_order_id: 0,
                        }),
                    )
                })
            }
            "CHECKPOINT_END: every account leaf + a near-full withdrawal queue" => {
                checkpoint_end = true;
                oracles(false)
            }
            _ => oracles(true),
        };
        let bytes = block(&st, &state_bytes, checkpoint_end, entries.clone());
        let native = caravel_perps::step(&state_bytes, &bytes, &DiagnosticCrypto)
            .unwrap_or_else(|f| panic!("{name}: native fatal {f:?}"));
        let started = std::time::Instant::now();
        let (out, m) = exec
            .step(&state_bytes, &bytes)
            .unwrap_or_else(|e| panic!("{name}: wasm {e:?}"));
        let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
        assert!(out == native, "{name}: native and wasm differ");
        let rc = caravel_types::receipts::Receipts::decode(&out.receipts).unwrap();
        let fills: usize = rc
            .receipts
            .iter()
            .flat_map(|r| &r.events)
            .filter(|e| matches!(e, caravel_types::receipts::Event::Fill { .. }))
            .count();
        let liqs: usize = rc
            .receipts
            .iter()
            .flat_map(|r| &r.events)
            .filter(|e| matches!(e, caravel_types::receipts::Event::Liquidation { .. }))
            .count();
        rows.push((
            name,
            entries.len(),
            bytes.len(),
            state_bytes.len(),
            fills,
            liqs,
            m.cpu_insns,
            m.mem_bytes,
            wall_ms,
        ));
    }
    println!("| Block at full caps | Entries | Block bytes | State bytes | Fills | Liquidations | Host CPU insns | Host mem bytes | Wall ms |");
    println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|");
    for (name, e, b, s, f, l, cpu, mem, ms) in &rows {
        println!("| {name} | {e} | {b} | {s} | {f} | {l} | {cpu} | {mem} | {ms:.0} |");
    }
    let worst = rows.iter().map(|r| r.6).max().unwrap();
    println!("\ncaps: max_accounts {a}, max_orders_per_side {o}, max_block_bytes {b}");
    println!(
        "\nworst block: {worst} CPU insns ({:.1}% of the 100M target, {:.1}% of exec_cpu_limit {})",
        worst as f64 / 1e6,
        worst as f64 * 100.0 / cfg.exec_cpu_limit as f64,
        cfg.exec_cpu_limit
    );
}
