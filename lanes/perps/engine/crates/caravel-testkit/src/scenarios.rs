//! Engine scenarios (spec §19.3). Each one asserts its expected behaviour with
//! numbers worked out from the spec formulas, and returns the lanes it ran so
//! the final state hashes can be kept in `test-vectors/scenarios.json`. Every
//! scenario is generic over the [`Executor`], so the same assertions run on the
//! native path and, from T-005, on the Wasm path (INV-P5).

use caravel_merkle::{verify, MerkleTree, NativeSha256};
use caravel_types::codes::*;
use caravel_types::config::AccessMode;
use caravel_types::fatal;
use caravel_types::inbox::{InboxKind, InboxMsgV1};
use caravel_types::preimage::{account_leaf_preimage, withdrawal_leaf_preimage};
use caravel_types::receipts::{
    CancelReason, DepositOutcome, Event, PSEUDO_BLOCK_END, PSEUDO_BLOCK_START,
};
use caravel_types::state::BACKSTOP_INDEX;
use caravel_types::tx::{
    PlaceOrder, Side, SigScheme, Tif, TxBody, ALL_MARKETS, PERM_ALL, PERM_CANCEL,
};
use caravel_types::vectors::{hex, pk, sha256};

use crate::lane::seeds::*;
use crate::lane::*;

/// Scenario names, in §19.3 order.
pub const NAMES: [&str; 20] = [
    "deposit_create_account",
    "limit_rest_and_cancel",
    "cross_and_fill_partial",
    "ioc_remainder_canceled",
    "post_only_rejected_when_crossing",
    "self_trade_prevention",
    "margin_reject_worst_case",
    "flip_position",
    "funding_zero_sum",
    "liquidation_to_backstop",
    "backstop_deficit_flag",
    "withdraw_then_checkpoint",
    "forced_withdrawal_cancels_orders",
    "session_key_permissions",
    "nonce_rules",
    "oracle_rules",
    "fatal_cases",
    "withdraw_liquidity_and_cash",
    "account_slot_reuse",
    "oracle_breaker_widens",
];

/// What a scenario leaves behind.
pub struct Outcome {
    pub name: &'static str,
    /// Final state hash of each lane the scenario ran, in order.
    pub state_hashes: Vec<[u8; 32]>,
    /// Blocks executed across all lanes.
    pub blocks: usize,
}

/// Runs one scenario by name.
pub fn run<E: Executor + Clone>(name: &str, exec: &E) -> Outcome {
    let lanes = match name {
        "deposit_create_account" => deposit_create_account(exec),
        "limit_rest_and_cancel" => limit_rest_and_cancel(exec),
        "cross_and_fill_partial" => cross_and_fill_partial(exec),
        "ioc_remainder_canceled" => ioc_remainder_canceled(exec),
        "post_only_rejected_when_crossing" => post_only_rejected_when_crossing(exec),
        "self_trade_prevention" => self_trade_prevention(exec),
        "margin_reject_worst_case" => margin_reject_worst_case(exec),
        "flip_position" => flip_position(exec),
        "funding_zero_sum" => funding_zero_sum(exec),
        "liquidation_to_backstop" => liquidation_to_backstop(exec),
        "backstop_deficit_flag" => backstop_deficit_flag(exec),
        "withdraw_then_checkpoint" => withdraw_then_checkpoint(exec),
        "forced_withdrawal_cancels_orders" => forced_withdrawal_cancels_orders(exec),
        "session_key_permissions" => session_key_permissions(exec),
        "nonce_rules" => nonce_rules(exec),
        "oracle_rules" => oracle_rules(exec),
        "fatal_cases" => fatal_cases(exec),
        "withdraw_liquidity_and_cash" => withdraw_liquidity_and_cash(exec),
        "account_slot_reuse" => account_slot_reuse(exec),
        "oracle_breaker_widens" => oracle_breaker_widens(exec),
        other => panic!("unknown scenario {other}"),
    };
    let name = NAMES
        .iter()
        .copied()
        .find(|n| *n == name)
        .expect("listed name");
    Outcome {
        name,
        state_hashes: lanes.iter().map(Lane::state_hash).collect(),
        blocks: lanes.iter().map(|l| l.records.len()).sum(),
    }
}

/// Runs every scenario in order.
pub fn all<E: Executor + Clone>(exec: &E) -> Vec<Outcome> {
    NAMES.iter().map(|n| run(n, exec)).collect()
}

/// The contents of `test-vectors/scenarios.json`.
pub fn vector_file() -> String {
    let cfg = config();
    let cfg_bytes = cfg.encode().expect("config encodes");
    let genesis = NativeExecutor.genesis(&cfg_bytes).expect("genesis");
    let vectors: Vec<String> = all(&NativeExecutor)
        .iter()
        .map(|o| {
            let hashes: Vec<String> = o.state_hashes.iter().map(|h| format!("          \"{}\"", hex(h))).collect();
            format!(
                "    {{\n      \"name\": \"{}\",\n      \"fields\": {{\n        \"blocks\": \"{}\",\n        \"state_hashes\": [\n{}\n        ]\n      }},\n      \"hex\": \"\",\n      \"hash\": \"{}\"\n    }}",
                o.name,
                o.blocks,
                hashes.join(",\n"),
                hex(o.state_hashes.last().expect("a scenario runs a lane"))
            )
        })
        .collect();
    format!(
        "{{\n  \"format\": \"engine scenarios\",\n  \"spec\": \"§19.3\",\n  \"hash_rule\": \"hash = the final state_hash (H(StateV1 bytes)) of the scenario's last lane. fields.state_hashes lists every lane's final hash; fields.blocks counts executed blocks. Native and Wasm runs MUST match (INV-P5).\",\n  \"context\": {{\n    \"config_hash\": \"{}\",\n    \"genesis_state_hash\": \"{}\",\n    \"genesis_state_bytes\": \"{}\"\n  }},\n  \"vectors\": [\n{}\n  ],\n  \"invalid\": []\n}}\n",
        hex(&sha256(&cfg_bytes)),
        hex(&sha256(&genesis)),
        genesis.len(),
        vectors.join(",\n")
    )
}

// --- Helpers -----------------------------------------------------------------

const P: i64 = BTC_PRICE;

/// `ceil(q × p × bps / 10_000)`, written out independently of the engine.
fn fee(q: i64, p: i64, bps: i128) -> i128 {
    let n = i128::from(q) * i128::from(p) * bps;
    (n + 9_999) / 10_000
}

/// The backstop's share of a fee: `floor(fee × 3_000 / 10_000)`.
fn ins(fee: i128) -> i128 {
    fee * 3_000 / 10_000
}

/// A lane with deposits and one oracle price per market, executed in block 1.
fn setup<E: Executor + Clone>(
    exec: &E,
    cfg: caravel_types::config::GenesisConfigV1,
    deposits: &[(u8, i128)],
) -> Lane<E> {
    let mut l = Lane::new(exec.clone(), cfg);
    for &(seed, amount) in deposits {
        l.deposit(seed, amount);
    }
    l.oracle(BTC, BTC_PRICE);
    l.oracle(ETH, ETH_PRICE);
    l.oracle(XLM, XLM_PRICE);
    l.block();
    l.all_ok();
    l
}

fn gtc(side: Side, price: i64, lots: i64) -> TxBody {
    TxBody::PlaceOrder(PlaceOrder {
        market_id: BTC,
        side,
        tif: Tif::Gtc,
        reduce_only: false,
        price,
        lots,
        client_order_id: 0,
    })
}

fn idx<E: Executor>(l: &Lane<E>, seed: u8) -> u32 {
    l.account_index(seed).expect("account exists") as u32
}

fn end_events<E: Executor>(l: &Lane<E>) -> Vec<Event> {
    l.receipts
        .receipts
        .iter()
        .find(|r| r.entry_index == PSEUDO_BLOCK_END)
        .map_or_else(Vec::new, |r| r.events.clone())
}

fn start_events<E: Executor>(l: &Lane<E>) -> Vec<Event> {
    l.receipts
        .receipts
        .iter()
        .find(|r| r.entry_index == PSEUDO_BLOCK_START)
        .map_or_else(Vec::new, |r| r.events.clone())
}

// --- 1 ----------------------------------------------------------------------

fn deposit_create_account<E: Executor + Clone>(exec: &E) -> Vec<Lane<E>> {
    // Open lane with room for two users.
    let mut cfg = config();
    cfg.max_accounts = 4;
    let mut l = Lane::new(exec.clone(), cfg);
    l.deposit(A, 100 * USDC);
    l.deposit(B, 50 * USDC);
    let t1 = l.now;
    l.block();
    assert_eq!(
        l.events(0),
        &[Event::Deposit {
            key: pk(A),
            amount: 100 * USDC,
            outcome: DepositOutcome::Created
        }]
    );
    assert_eq!(
        l.events(1),
        &[Event::Deposit {
            key: pk(B),
            amount: 50 * USDC,
            outcome: DepositOutcome::Created
        }]
    );
    let st = l.state();
    assert_eq!(st.accounts.len(), 4);
    assert_eq!(
        (
            st.accounts[2].key,
            st.accounts[2].collateral,
            st.accounts[2].next_nonce
        ),
        (pk(A), 100 * USDC, t1)
    );
    assert_eq!(st.accounts[3].key, pk(B));

    // A second deposit credits; a third key finds no slot and bounces.
    l.deposit(A, 10 * USDC);
    l.deposit(C, 20 * USDC);
    l.block();
    assert_eq!(
        l.events(0),
        &[Event::Deposit {
            key: pk(A),
            amount: 10 * USDC,
            outcome: DepositOutcome::Credited
        }]
    );
    assert_eq!(
        l.events(1),
        &[Event::Deposit {
            key: pk(C),
            amount: 20 * USDC,
            outcome: DepositOutcome::Bounced
        }]
    );
    let st = l.state();
    assert_eq!(l.collateral(A), 110 * USDC);
    assert_eq!(st.deposits_credited_total, 180 * USDC);
    assert_eq!(
        (st.pending.len(), st.pending[0].key, st.pending[0].amount),
        (1, pk(C), 20 * USDC)
    );

    // The bounce is refunded through the withdrawals root.
    l.checkpoint();
    let st = l.state();
    let c = st.last_commitment;
    assert!(st.pending.is_empty());
    assert_eq!(
        (c.seq, c.withdrawal_count, c.withdrawals_total),
        (1, 1, 20 * USDC)
    );
    assert_eq!(st.withdrawals_committed_total, 20 * USDC);
    let leaf = sha256(&withdrawal_leaf_preimage(
        &st.lane_id,
        1,
        0,
        &pk(C),
        20 * USDC,
    ));
    assert!(verify(&NativeSha256, &leaf, 0, 1, &[], &c.withdrawals_root));

    // Allowlist lane: only A may open an account.
    let mut cfg2 = config();
    cfg2.access_mode = AccessMode::Allowlist;
    cfg2.allowlist = vec![pk(A)];
    let mut l2 = Lane::new(exec.clone(), cfg2);
    l2.deposit(B, 5 * USDC);
    l2.deposit(A, 5 * USDC);
    l2.block();
    assert_eq!(
        l2.events(0),
        &[Event::Deposit {
            key: pk(B),
            amount: 5 * USDC,
            outcome: DepositOutcome::Bounced
        }]
    );
    assert_eq!(
        l2.events(1),
        &[Event::Deposit {
            key: pk(A),
            amount: 5 * USDC,
            outcome: DepositOutcome::Created
        }]
    );
    assert_eq!(l2.state().accounts.len(), 3);
    vec![l, l2]
}

// --- 2 ----------------------------------------------------------------------

fn limit_rest_and_cancel<E: Executor + Clone>(exec: &E) -> Vec<Lane<E>> {
    let mut l = setup(exec, config(), &[(A, 1000 * USDC), (B, 1000 * USDC)]);
    l.order(A, BTC, Side::Buy, Tif::Gtc, P - 10 * TICK, 10);
    l.order(A, BTC, Side::Buy, Tif::Gtc, P - 20 * TICK, 5);
    l.order(A, BTC, Side::Sell, Tif::Gtc, P + 10 * TICK, 3);
    l.order(B, BTC, Side::Buy, Tif::Gtc, P - 30 * TICK, 1);
    l.block();
    l.all_ok();
    let ids: Vec<u64> = l
        .user_entries()
        .into_iter()
        .map(|e| l.rested_id(e))
        .collect();
    assert_eq!(ids, vec![1, 2, 3, 4]);
    let st = l.state();
    let bids: Vec<u64> = st.markets[0].bids.iter().map(|o| o.order_id).collect();
    assert_eq!(bids, vec![1, 2, 4]);
    assert_eq!(st.markets[0].asks.len(), 1);
    assert_eq!(l.account(A).unwrap().open_order_count, 3);

    // Cancel by id; unknown or someone else's id is ORDER_NOT_FOUND and still consumes the nonce.
    let n = l.account(A).unwrap().next_nonce;
    l.tx(
        A,
        TxBody::CancelOrder {
            market_id: BTC,
            order_id: 1,
        },
    );
    l.tx(
        A,
        TxBody::CancelOrder {
            market_id: BTC,
            order_id: 1,
        },
    );
    l.tx(
        A,
        TxBody::CancelOrder {
            market_id: BTC,
            order_id: 4,
        },
    );
    l.block();
    assert_eq!(l.user_codes(), vec![OK, ORDER_NOT_FOUND, ORDER_NOT_FOUND]);
    assert_eq!(
        l.user(0).events,
        vec![Event::OrderCanceled {
            market: BTC,
            order_id: 1,
            reason: CancelReason::User
        }]
    );
    assert_eq!(l.account(A).unwrap().next_nonce, n + 3);
    assert_eq!(l.account(A).unwrap().open_order_count, 2);

    // Cancel-all: bids then asks, in book order. An unconfigured market id is rejected.
    l.tx(
        A,
        TxBody::CancelAll {
            market_id: ALL_MARKETS,
        },
    );
    l.tx(A, TxBody::CancelAll { market_id: 99 });
    l.block();
    assert_eq!(l.user_codes(), vec![OK, UNKNOWN_MARKET]);
    assert_eq!(
        l.user(0).events,
        vec![
            Event::OrderCanceled {
                market: BTC,
                order_id: 2,
                reason: CancelReason::User
            },
            Event::OrderCanceled {
                market: BTC,
                order_id: 3,
                reason: CancelReason::User
            },
        ]
    );
    let st = l.state();
    assert_eq!(
        st.markets[0]
            .bids
            .iter()
            .map(|o| o.order_id)
            .collect::<Vec<_>>(),
        vec![4]
    );
    assert!(st.markets[0].asks.is_empty());
    assert_eq!(l.account(A).unwrap().open_order_count, 0);
    vec![l]
}

// --- 3 ----------------------------------------------------------------------

fn cross_and_fill_partial<E: Executor + Clone>(exec: &E) -> Vec<Lane<E>> {
    let mut l = setup(
        exec,
        config(),
        &[
            (A, 1000 * USDC),
            (B, 1000 * USDC),
            (C, 1000 * USDC),
            (D, 1000 * USDC),
        ],
    );
    let p1 = P + TICK;
    l.order(B, BTC, Side::Sell, Tif::Gtc, P, 5);
    l.order(C, BTC, Side::Sell, Tif::Gtc, P, 5);
    l.order(D, BTC, Side::Sell, Tif::Gtc, p1, 10);
    l.block();
    l.all_ok();
    let before = |l: &Lane<E>| {
        [
            l.collateral(A),
            l.collateral(B),
            l.collateral(C),
            l.collateral(D),
        ]
    };
    let c0 = before(&l);
    let (b0, t0) = (
        l.state().accounts[0].collateral,
        l.state().accounts[1].collateral,
    );

    l.order(A, BTC, Side::Buy, Tif::Gtc, p1, 25);
    l.block();
    l.all_ok();
    let (a, b, c, d) = (idx(&l, A), idx(&l, B), idx(&l, C), idx(&l, D));
    let fill = |maker_order_id, maker_idx, price, lots| Event::Fill {
        market: BTC,
        maker_order_id,
        maker_idx,
        taker_idx: a,
        price,
        lots,
        taker_side: Side::Buy,
    };
    assert_eq!(
        l.user(0).events,
        vec![
            fill(1, b, P, 5),
            fill(2, c, P, 5),
            fill(3, d, p1, 10),
            Event::OrderRested {
                market: BTC,
                order_id: 4,
                account_idx: a,
                side: Side::Buy,
                price: p1,
                lots: 5
            },
        ]
    );
    let p128 = i128::from(P);
    let p1_128 = i128::from(p1);
    assert_eq!(l.position(A, BTC), (20, 10 * p128 + 10 * p1_128));
    assert_eq!(l.position(B, BTC), (-5, -5 * p128));
    assert_eq!(l.position(C, BTC), (-5, -5 * p128));
    assert_eq!(l.position(D, BTC), (-10, -10 * p1_128));
    let fees = [fee(5, P, 5), fee(5, P, 5), fee(10, p1, 5)];
    assert_eq!(
        before(&l),
        [c0[0] - fees.iter().sum::<i128>(), c0[1], c0[2], c0[3]],
        "taker pays 5 bps, makers pay 0"
    );
    let st = l.state();
    assert_eq!(
        st.accounts[0].collateral,
        b0 + fees.iter().map(|f| ins(*f)).sum::<i128>()
    );
    assert_eq!(
        st.accounts[1].collateral,
        t0 + fees.iter().map(|f| f - ins(*f)).sum::<i128>()
    );
    assert_eq!(st.markets[0].open_interest_lots, 20);
    assert!(st.markets[0].asks.is_empty());
    assert_eq!(st.markets[0].bids.len(), 1);
    vec![l]
}

// --- 4 ----------------------------------------------------------------------

fn ioc_remainder_canceled<E: Executor + Clone>(exec: &E) -> Vec<Lane<E>> {
    let mut l = setup(exec, config(), &[(A, 1000 * USDC), (B, 1000 * USDC)]);
    l.order(B, BTC, Side::Sell, Tif::Gtc, P, 5);
    l.block();
    let next_id = l.state().next_order_id;
    l.order(A, BTC, Side::Buy, Tif::Ioc, P, 8);
    l.block();
    l.all_ok();
    let (a, b) = (idx(&l, A), idx(&l, B));
    assert_eq!(
        l.user(0).events,
        vec![
            Event::Fill {
                market: BTC,
                maker_order_id: 1,
                maker_idx: b,
                taker_idx: a,
                price: P,
                lots: 5,
                taker_side: Side::Buy
            },
            Event::OrderCanceled {
                market: BTC,
                order_id: 0,
                reason: CancelReason::IocRemainder
            },
        ]
    );
    let st = l.state();
    assert_eq!(
        st.next_order_id, next_id,
        "an IOC order never gets an order id"
    );
    assert!(st.markets[0].bids.is_empty());
    assert_eq!(l.position(A, BTC).0, 5);
    vec![l]
}

fn post_only_rejected_when_crossing<E: Executor + Clone>(exec: &E) -> Vec<Lane<E>> {
    let mut l = setup(exec, config(), &[(A, 1000 * USDC), (B, 1000 * USDC)]);
    l.order(B, BTC, Side::Sell, Tif::Gtc, P, 5);
    l.block();
    let n = l.account(A).unwrap().next_nonce;
    l.order(A, BTC, Side::Buy, Tif::PostOnly, P, 1);
    l.order(A, BTC, Side::Buy, Tif::PostOnly, P - TICK, 1);
    l.block();
    assert_eq!(l.user_codes(), vec![POST_ONLY_WOULD_CROSS, OK]);
    assert_eq!(
        l.account(A).unwrap().next_nonce,
        n + 2,
        "the rejection consumed its nonce"
    );
    let st = l.state();
    assert_eq!(
        (st.markets[0].bids.len(), st.markets[0].bids[0].price),
        (1, P - TICK)
    );
    assert_eq!(st.markets[0].asks[0].lots_remaining, 5);
    vec![l]
}

fn self_trade_prevention<E: Executor + Clone>(exec: &E) -> Vec<Lane<E>> {
    let mut l = setup(exec, config(), &[(A, 1000 * USDC), (B, 1000 * USDC)]);
    l.order(A, BTC, Side::Sell, Tif::Gtc, P, 5);
    l.order(B, BTC, Side::Sell, Tif::Gtc, P + TICK, 5);
    l.block();
    l.order(A, BTC, Side::Buy, Tif::Ioc, P + TICK, 5);
    l.block();
    l.all_ok();
    let (a, b) = (idx(&l, A), idx(&l, B));
    assert_eq!(
        l.user(0).events,
        vec![
            Event::OrderCanceled {
                market: BTC,
                order_id: 1,
                reason: CancelReason::SelfTrade
            },
            Event::Fill {
                market: BTC,
                maker_order_id: 2,
                maker_idx: b,
                taker_idx: a,
                price: P + TICK,
                lots: 5,
                taker_side: Side::Buy
            },
        ]
    );
    assert!(l.state().markets[0].asks.is_empty());
    assert_eq!(l.account(A).unwrap().open_order_count, 0);
    assert_eq!((l.position(A, BTC).0, l.position(B, BTC).0), (5, -5));
    vec![l]
}

// --- 5 ----------------------------------------------------------------------

fn margin_reject_worst_case<E: Executor + Clone>(exec: &E) -> Vec<Lane<E>> {
    let deposit = 800 * USDC;
    let mut l = setup(exec, config(), &[(A, deposit)]);
    let hi = P + P / 20; // +5%, the edge of the price band
    let lots = 1000;
    let im = i128::from(lots) * i128::from(P) * 1000 / 10_000;
    let at_mark = im + fee(lots, P, 5);
    let at_hi = im + fee(lots, hi, 5) + i128::from(lots) * i128::from(hi - P);
    assert!(
        deposit >= at_mark && deposit < at_hi,
        "the deposit sits between the two requirements"
    );
    l.order(A, BTC, Side::Buy, Tif::Gtc, hi, lots);
    l.order(A, BTC, Side::Buy, Tif::Gtc, P, lots);
    l.block();
    assert_eq!(l.user_codes(), vec![INSUFFICIENT_MARGIN, OK]);
    assert_eq!(l.state().markets[0].bids[0].price, P);
    vec![l]
}

// --- 6 ----------------------------------------------------------------------

fn flip_position<E: Executor + Clone>(exec: &E) -> Vec<Lane<E>> {
    let mut l = setup(exec, config(), &[(A, 1000 * USDC), (B, 1000 * USDC)]);
    let p1 = P;
    let p2 = 66_000_000;
    l.order(B, BTC, Side::Sell, Tif::Gtc, p1, 10);
    l.block();
    l.order(A, BTC, Side::Buy, Tif::Ioc, p1, 10);
    l.block();
    l.all_ok();
    assert_eq!(l.position(A, BTC), (10, 10 * i128::from(p1)));
    let (a1, b1) = (l.collateral(A), l.collateral(B));

    l.oracle(BTC, p2);
    l.tx(B, gtc(Side::Buy, p2, 25));
    l.block();
    l.all_ok();
    l.order(A, BTC, Side::Sell, Tif::Ioc, p2, 25);
    l.block();
    l.all_ok();
    // A closes 10 (realized 10 × (p2 − p1) = 1 USDC) and opens 15 short at p2.
    let realized = 10 * i128::from(p2 - p1);
    assert_eq!(realized, USDC);
    assert_eq!(l.position(A, BTC), (-15, -15 * i128::from(p2)));
    assert_eq!(l.collateral(A), a1 + realized - fee(25, p2, 5));
    // B mirrors it as maker (0 fee).
    assert_eq!(l.position(B, BTC), (15, 15 * i128::from(p2)));
    assert_eq!(l.collateral(B), b1 - realized);
    vec![l]
}

// --- 7 ----------------------------------------------------------------------

fn funding_zero_sum<E: Executor + Clone>(exec: &E) -> Vec<Lane<E>> {
    let mut l = setup(
        exec,
        config(),
        &[
            (A, 1000 * USDC),
            (B, 1000 * USDC),
            (C, 1000 * USDC),
            (D, 2000 * USDC),
        ],
    );
    // Block 2 runs the first funding: fresh price, empty book, so the rate is 0.
    l.order(B, BTC, Side::Sell, Tif::Gtc, P, 15);
    l.order(A, BTC, Side::Buy, Tif::Ioc, P, 10);
    l.order(C, BTC, Side::Buy, Tif::Ioc, P, 5);
    l.block();
    l.all_ok();
    assert!(start_events(&l).contains(&Event::Funding {
        market: BTC,
        rate_ppm: 0,
        fpl: 0
    }));
    // Depth of exactly impact_lots (1000) on each side, both above mark.
    l.tx(D, gtc(Side::Buy, P + 20_000, 1000));
    l.tx(D, gtc(Side::Sell, P + 40_000, 1000));
    l.block();
    l.all_ok();

    // mid = P + 30_000; premium = floor(30_000e6 / P) = 461 ppm; rate = trunc(461 / 8) = 57;
    // fpl = floor(57 × P / 1e6) = 3_705 stroops per lot, paid by longs.
    let premium = 30_000i128 * 1_000_000 / i128::from(P);
    let rate = premium / 8;
    let fpl = rate * i128::from(P) / 1_000_000;
    assert_eq!((premium, rate, fpl), (461, 57, 3_705));
    let interval = 3_600_000;
    let mut next = (l.state().markets[0].last_funding_time_ms) + interval;
    for _ in 0..2 {
        // Refresh the price one second before the boundary, then cross it.
        l.now = next - 1000;
        l.oracle(BTC, P);
        l.block();
        assert!(start_events(&l).is_empty());
        let all = |l: &Lane<E>| {
            l.state()
                .accounts
                .iter()
                .map(|a| a.collateral)
                .collect::<Vec<_>>()
        };
        let before = all(&l);
        let (a0, b0, c0, d0) = (
            l.collateral(A),
            l.collateral(B),
            l.collateral(C),
            l.collateral(D),
        );
        l.block();
        assert_eq!(
            start_events(&l),
            vec![Event::Funding {
                market: BTC,
                rate_ppm: rate as i32,
                fpl: fpl as i64
            }]
        );
        assert_eq!(l.collateral(A) - a0, -10 * fpl);
        assert_eq!(l.collateral(C) - c0, -5 * fpl);
        assert_eq!(l.collateral(B) - b0, 15 * fpl);
        assert_eq!(l.collateral(D), d0);
        let after = all(&l);
        assert_eq!(
            before.iter().sum::<i128>(),
            after.iter().sum::<i128>(),
            "funding sums to zero"
        );
        assert_eq!(l.state().markets[0].last_funding_time_ms, next);
        next += interval;
    }
    assert_eq!(l.state().markets[0].cumulative_funding_per_lot, 2 * fpl);
    vec![l]
}

// --- 8 ----------------------------------------------------------------------

/// A crash from P to M2 = −11%, 20 s after the last price, inside the widened breaker.
const M2: i64 = 57_850_000;

fn liquidation_to_backstop<E: Executor + Clone>(exec: &E) -> Vec<Lane<E>> {
    let mut l = setup(
        exec,
        config(),
        &[(A, 660 * USDC), (C, 1000 * USDC), (B, 2000 * USDC)],
    );
    l.order(B, BTC, Side::Sell, Tif::Gtc, P, 2000);
    l.block();
    l.order(A, BTC, Side::Buy, Tif::Ioc, P, 1000);
    l.order(C, BTC, Side::Buy, Tif::Ioc, P, 1000);
    l.block();
    l.all_ok();
    let taker_fee = fee(1000, P, 5);
    assert_eq!(l.collateral(A), 660 * USDC - taker_fee);
    let backstop0 = l.state().accounts[0].collateral;
    assert_eq!(backstop0, 2 * ins(taker_fee));

    l.advance(20_000);
    l.oracle(BTC, M2);
    l.block();
    let loss = 1000 * i128::from(P - M2);
    let fee_due = fee(1000, M2, 100);
    // A: bankrupt at mark, so no fee is collected and the backstop absorbs the rest.
    let a_after_close = 660 * USDC - taker_fee - loss;
    assert!(a_after_close < 0);
    // C: still solvent at mark but under maintenance, so it pays the full liquidation fee.
    let c_after_close = 1000 * USDC - taker_fee - loss;
    assert!(c_after_close > fee_due);
    assert_eq!(
        end_events(&l),
        vec![
            Event::Liquidation {
                account_idx: idx(&l, A),
                fee: 0,
                deficit: -a_after_close
            },
            Event::Liquidation {
                account_idx: idx(&l, C),
                fee: fee_due,
                deficit: 0
            },
        ]
    );
    assert_eq!(l.collateral(A), 0);
    assert_eq!(l.collateral(C), c_after_close - fee_due);
    let st = l.state();
    let backstop = &st.accounts[BACKSTOP_INDEX as usize];
    assert_eq!(
        backstop.positions[0].lots, 2000,
        "the backstop takes both positions at mark"
    );
    assert_eq!(backstop.positions[0].cost_basis, 2000 * i128::from(M2));
    assert_eq!(backstop.collateral, backstop0 + a_after_close + fee_due);
    assert!(!st.backstop_deficit);
    assert_eq!(l.position(A, BTC), (0, 0));
    assert_eq!(l.position(B, BTC).0, -2000);
    vec![l]
}

// --- 9 ----------------------------------------------------------------------

fn backstop_deficit_flag<E: Executor + Clone>(exec: &E) -> Vec<Lane<E>> {
    let mut l = setup(
        exec,
        config(),
        &[(A, 660 * USDC), (B, 2000 * USDC), (D, 2000 * USDC)],
    );
    l.order(B, BTC, Side::Sell, Tif::Gtc, P, 1000);
    l.block();
    l.order(A, BTC, Side::Buy, Tif::Ioc, P, 1000);
    l.block();
    l.advance(20_000);
    l.oracle(BTC, M2);
    l.block();
    let deficit = 1000 * i128::from(P - M2) - (660 * USDC - fee(1000, P, 5));
    let backstop_collateral = ins(fee(1000, P, 5)) - deficit;
    assert!(backstop_collateral < 0);
    assert_eq!(
        end_events(&l),
        vec![
            Event::Liquidation {
                account_idx: idx(&l, A),
                fee: 0,
                deficit
            },
            Event::BackstopDeficit {
                active: true,
                backstop_equity: backstop_collateral
            },
        ]
    );
    assert!(l.state().backstop_deficit);

    // Trading continues.
    let hi = 60_742_000; // M2 + 5%, floored to the tick
    l.tx(D, gtc(Side::Buy, hi, 1000));
    l.block();
    l.all_ok();

    // The operator unwinds the backstop with its own key (reduce-only IOC) above mark.
    l.reduce_only(BACKSTOP, BTC, Side::Sell, hi, 1000);
    l.block();
    l.all_ok();
    let unwind_fee = fee(1000, hi, 5);
    let equity = backstop_collateral + 1000 * i128::from(hi - M2) - unwind_fee + ins(unwind_fee);
    assert!(equity > 0);
    assert_eq!(
        end_events(&l),
        vec![Event::BackstopDeficit {
            active: false,
            backstop_equity: equity
        }]
    );
    let st = l.state();
    assert!(!st.backstop_deficit);
    assert_eq!(st.accounts[BACKSTOP_INDEX as usize].positions[0].lots, 0);
    assert_eq!(l.position(D, BTC).0, 1000);
    vec![l]
}

// --- 10 ---------------------------------------------------------------------

fn withdraw_then_checkpoint<E: Executor + Clone>(exec: &E) -> Vec<Lane<E>> {
    let mut l = setup(exec, config(), &[(A, 100 * USDC)]);
    l.withdraw(A, 30 * USDC);
    l.block();
    l.all_ok();
    let st = l.state();
    assert_eq!((st.pending.len(), st.pending[0].amount), (1, 30 * USDC));
    assert_eq!(l.collateral(A), 70 * USDC);

    l.checkpoint();
    let st = l.state();
    let c = st.last_commitment;
    assert_eq!(
        end_events(&l),
        vec![Event::Commitment {
            seq: 1,
            withdrawals_total: 30 * USDC,
            escape_total: 70 * USDC
        }]
    );
    assert_eq!(
        (c.seq, c.last_block_height, c.account_count, c.escape_total),
        (1, l.height(), 3, 70 * USDC)
    );
    assert_eq!((c.withdrawal_count, c.withdrawals_total), (1, 30 * USDC));
    assert_eq!(
        (c.inbox_through, c.inbox_acc),
        (st.inbox_through, st.inbox_acc)
    );
    assert_eq!(
        (st.checkpoint_seq, st.withdrawals_committed_total),
        (1, 30 * USDC)
    );
    assert!(st.pending.is_empty());

    // Both roots verify with caravel-merkle.
    let wleaf = sha256(&withdrawal_leaf_preimage(
        &st.lane_id,
        1,
        0,
        &pk(A),
        30 * USDC,
    ));
    assert!(verify(
        &NativeSha256,
        &wleaf,
        0,
        1,
        &[],
        &c.withdrawals_root
    ));
    let leaves: Vec<[u8; 32]> = st
        .accounts
        .iter()
        .enumerate()
        .map(|(j, a)| {
            sha256(&account_leaf_preimage(
                &st.lane_id,
                1,
                j as u32,
                &a.key,
                a.collateral.max(0),
            ))
        })
        .collect();
    let tree = MerkleTree::build(&NativeSha256, &leaves).unwrap();
    assert_eq!(tree.root(), c.accounts_root);
    assert!(verify(
        &NativeSha256,
        &leaves[2],
        2,
        3,
        &tree.proof(2).unwrap(),
        &c.accounts_root
    ));

    // A second checkpoint uses seq 2 in its leaves.
    l.withdraw(A, 10 * USDC);
    l.checkpoint();
    let st = l.state();
    let leaf2 = sha256(&withdrawal_leaf_preimage(
        &st.lane_id,
        2,
        0,
        &pk(A),
        10 * USDC,
    ));
    assert!(verify(
        &NativeSha256,
        &leaf2,
        0,
        1,
        &[],
        &st.last_commitment.withdrawals_root
    ));
    assert_eq!(st.withdrawals_committed_total, 40 * USDC);
    vec![l]
}

// --- 11 ---------------------------------------------------------------------

fn forced_withdrawal_cancels_orders<E: Executor + Clone>(exec: &E) -> Vec<Lane<E>> {
    let mut l = setup(exec, config(), &[(A, 100 * USDC), (B, 100 * USDC)]);
    l.order(A, BTC, Side::Buy, Tif::Gtc, P - TICK, 2);
    l.order(A, ETH, Side::Sell, Tif::Gtc, ETH_PRICE + TICK, 3);
    l.block();
    l.all_ok();
    assert_eq!(l.account(A).unwrap().open_order_count, 2);

    l.forced_withdrawal(A, 50 * USDC);
    l.forced_withdrawal(E, 10 * USDC); // no account: a no-op
    l.forced_withdrawal(B, 1000 * USDC); // capped at what B holds
    l.block();
    assert_eq!(
        l.events(0),
        &[
            Event::OrderCanceled {
                market: BTC,
                order_id: 1,
                reason: CancelReason::ForcedWithdrawal
            },
            Event::OrderCanceled {
                market: ETH,
                order_id: 2,
                reason: CancelReason::ForcedWithdrawal
            },
            Event::ForcedWithdrawalProcessed {
                key: pk(A),
                amount: 50 * USDC
            },
        ]
    );
    assert!(l.events(1).is_empty());
    assert_eq!(
        l.events(2),
        &[Event::ForcedWithdrawalProcessed {
            key: pk(B),
            amount: 100 * USDC
        }]
    );
    let st = l.state();
    assert!(st
        .markets
        .iter()
        .all(|m| m.bids.is_empty() && m.asks.is_empty()));
    assert_eq!(l.account(A).unwrap().open_order_count, 0);
    assert_eq!((l.collateral(A), l.collateral(B)), (50 * USDC, 0));
    let pending: Vec<_> = st.pending.iter().map(|p| (p.key, p.amount)).collect();
    assert_eq!(pending, vec![(pk(A), 50 * USDC), (pk(B), 100 * USDC)]);
    vec![l]
}

// --- 12 ---------------------------------------------------------------------

fn session_key_permissions<E: Executor + Clone>(exec: &E) -> Vec<Lane<E>> {
    let mut l = setup(exec, config(), &[(A, 100 * USDC)]);
    let day = 86_400_000;
    let order = || gtc(Side::Buy, P - TICK, 1);
    l.tx_as(
        A,
        A,
        SigScheme::Sep53,
        TxBody::AddSessionKey {
            session_key: pk(S),
            expires_at_ms: l.now + day,
            permissions: PERM_ALL,
        },
    );
    l.block();
    l.all_ok();

    // S trades; it can never withdraw, manage keys or use SEP-53.
    l.tx_as(S, A, SigScheme::RawEd25519, order());
    l.tx_as(
        S,
        A,
        SigScheme::RawEd25519,
        TxBody::Withdraw { amount: 10 * USDC },
    );
    l.tx_as(
        S,
        A,
        SigScheme::RawEd25519,
        TxBody::AddSessionKey {
            session_key: pk(T),
            expires_at_ms: l.now + day,
            permissions: PERM_ALL,
        },
    );
    l.tx_as(S, A, SigScheme::Sep53, order());
    l.block();
    assert_eq!(
        l.user_codes(),
        vec![
            OK,
            UNAUTHORIZED_SIGNER,
            UNAUTHORIZED_SIGNER,
            UNAUTHORIZED_SIGNER
        ]
    );
    let order_id = l.rested_id(l.user_entries()[0]);

    // T may only cancel; U expires in 1.5 s.
    l.tx(
        A,
        TxBody::AddSessionKey {
            session_key: pk(T),
            expires_at_ms: l.now + day,
            permissions: PERM_CANCEL,
        },
    );
    l.tx(
        A,
        TxBody::AddSessionKey {
            session_key: pk(U),
            expires_at_ms: l.now + 1500,
            permissions: PERM_ALL,
        },
    );
    l.block();
    l.all_ok();
    l.tx_as(
        T,
        A,
        SigScheme::RawEd25519,
        TxBody::CancelOrder {
            market_id: BTC,
            order_id,
        },
    );
    l.tx_as(T, A, SigScheme::RawEd25519, order());
    l.block();
    assert_eq!(l.user_codes(), vec![OK, UNAUTHORIZED_SIGNER]);
    l.advance(2000);
    l.tx_as(U, A, SigScheme::RawEd25519, order());
    l.block();
    assert_eq!(
        l.user_codes(),
        vec![UNAUTHORIZED_SIGNER],
        "an expired key is unauthorized"
    );

    // Revoke works once.
    l.tx(A, TxBody::RevokeSessionKey { session_key: pk(S) });
    l.tx(A, TxBody::RevokeSessionKey { session_key: pk(S) });
    l.tx_as(S, A, SigScheme::RawEd25519, order());
    l.block();
    assert_eq!(
        l.user_codes(),
        vec![OK, BAD_SESSION_KEY, UNAUTHORIZED_SIGNER]
    );

    // ADD rules: own key, bad bits, too long, already expired, duplicate; then the cap of 4.
    let add = |key, expires_at_ms, permissions| TxBody::AddSessionKey {
        session_key: key,
        expires_at_ms,
        permissions,
    };
    let now = l.now;
    l.tx(A, add(pk(A), now + day, PERM_ALL));
    l.tx(A, add(pk(V), now + day, 0x04));
    l.tx(A, add(pk(V), now + 7 * day + 1, PERM_ALL));
    l.tx(A, add(pk(V), now, PERM_ALL));
    l.tx(A, add(pk(T), now + day, PERM_ALL));
    l.tx(A, add(pk(V), now + 7 * day, PERM_ALL));
    l.tx(A, add(pk(W), now + day, PERM_ALL));
    l.tx(A, add(pk(S), now + day, PERM_ALL));
    l.block();
    assert_eq!(
        l.user_codes(),
        vec![
            BAD_SESSION_KEY,
            BAD_SESSION_KEY,
            BAD_SESSION_KEY,
            BAD_SESSION_KEY,
            BAD_SESSION_KEY,
            OK,
            OK,
            TOO_MANY_SESSION_KEYS
        ]
    );
    let keys: Vec<[u8; 32]> = l
        .account(A)
        .unwrap()
        .session_keys
        .iter()
        .map(|k| k.key)
        .collect();
    let mut want = vec![pk(T), pk(U), pk(V), pk(W)];
    want.sort();
    assert_eq!(keys, want, "sorted by key; expired keys still count");
    vec![l]
}

// --- 13 ---------------------------------------------------------------------

fn nonce_rules<E: Executor + Clone>(exec: &E) -> Vec<Lane<E>> {
    let mut l = setup(exec, config(), &[(A, 1000 * USDC)]);
    let n = l.account(A).unwrap().next_nonce;
    let small = gtc(Side::Buy, P - TICK, 1);
    let huge = gtc(Side::Buy, P, 20_000);
    let expired = l.signed(A, A, SigScheme::RawEd25519, n, l.now - 1, small);
    let ahead = l.signed(A, A, SigScheme::RawEd25519, n + 5, l.now + 60_000, small);
    let margin = l.signed(A, A, SigScheme::RawEd25519, n, l.now + 60_000, huge);
    let ok = l.signed(A, A, SigScheme::RawEd25519, n + 1, l.now + 60_000, small);
    for t in [expired, ahead, margin, ok] {
        l.push_tx(t);
    }
    l.block();
    assert_eq!(
        l.user_codes(),
        vec![EXPIRED, BAD_NONCE, INSUFFICIENT_MARGIN, OK]
    );
    assert_eq!(
        l.account(A).unwrap().next_nonce,
        n + 2,
        "expired and bad nonces are not consumed; a margin reject is"
    );

    // 50 transactions per account per block; the 51st is rate limited and not consumed.
    let n = n + 2;
    for i in 0..51 {
        let t = l.signed(
            A,
            A,
            SigScheme::RawEd25519,
            n + i,
            l.now + 60_000,
            TxBody::CancelOrder {
                market_id: BTC,
                order_id: 999,
            },
        );
        l.push_tx(t);
    }
    l.block();
    let codes = l.user_codes();
    assert!(codes[..50].iter().all(|c| *c == ORDER_NOT_FOUND));
    assert_eq!(codes[50], RATE_LIMITED);
    assert_eq!(l.account(A).unwrap().next_nonce, n + 50);
    // The counter resets each block.
    let t = l.signed(A, A, SigScheme::RawEd25519, n + 50, l.now + 60_000, small);
    l.push_tx(t);
    l.block();
    assert_eq!(l.user_codes(), vec![OK]);
    vec![l]
}

// --- 14 ---------------------------------------------------------------------

fn oracle_rules<E: Executor + Clone>(exec: &E) -> Vec<Lane<E>> {
    let mut l = setup(exec, config(), &[(A, 1000 * USDC)]);
    // Stale: 40 s after the last price (max 30 s).
    l.advance(40_000);
    l.order(A, BTC, Side::Buy, Tif::Gtc, P - TICK, 1);
    l.block();
    assert_eq!(l.user_codes(), vec![ORACLE_STALE]);

    // Non-fatal rejections: too far in the future, not newer, non-positive.
    let now = l.now;
    let future = l.oracle_update(ORACLE, BTC, P, now + 5_001);
    let old = l.oracle_update(ORACLE, ETH, ETH_PRICE, T0);
    let zero = l.oracle_update(ORACLE, XLM, 0, now);
    for u in [future, old, zero] {
        l.push_oracle(u);
    }
    l.block();
    assert_eq!(
        l.events(0),
        &[Event::Oracle {
            market: BTC,
            price: P,
            accepted: false
        }]
    );
    assert_eq!(
        l.events(1),
        &[Event::Oracle {
            market: ETH,
            price: ETH_PRICE,
            accepted: false
        }]
    );
    assert_eq!(
        l.events(2),
        &[Event::Oracle {
            market: XLM,
            price: 0,
            accepted: false
        }]
    );
    // The edge of the future window is accepted.
    let edge = l.oracle_update(ORACLE, BTC, P, l.now + 5_000);
    l.push_oracle(edge);
    l.block();
    assert_eq!(
        l.events(0),
        &[Event::Oracle {
            market: BTC,
            price: P,
            accepted: true
        }]
    );

    // Circuit breaker: +15% one second later is outside 10% + 0.1%/s.
    l.advance(5_000);
    l.oracle(BTC, P);
    l.block();
    let jump = P + P * 15 / 100;
    l.oracle(BTC, jump);
    l.block();
    assert_eq!(
        l.events(0),
        &[Event::Oracle {
            market: BTC,
            price: jump,
            accepted: false
        }]
    );
    assert_eq!(l.state().markets[0].oracle_price, P);

    // Unknown market: rejected, not fatal.
    let unknown = l.oracle_update(ORACLE, 9, P, l.now);
    l.push_oracle(unknown);
    l.block();
    assert_eq!(
        l.events(0),
        &[Event::Oracle {
            market: 9,
            price: P,
            accepted: false
        }]
    );

    // Unknown key: fatal, and the lane does not move.
    let before = l.state_hash();
    let rogue = l.oracle_update(0x3F, BTC, P, l.now);
    l.push_oracle(rogue);
    let r = l.try_block(false).err();
    l.expect_fatal(r, fatal::UNKNOWN_ORACLE_KEY, 0);
    assert_eq!(l.state_hash(), before);
    vec![l]
}

// --- 15 ---------------------------------------------------------------------

fn fatal_cases<E: Executor + Clone>(exec: &E) -> Vec<Lane<E>> {
    let mut l = setup(exec, config(), &[(A, 1000 * USDC)]);
    let good = l.state_hash();
    let blk = BLOCK_LEVEL_ENTRY;

    // Bad encodings.
    let mut bytes = l.build(false).encode().unwrap();
    bytes[0] ^= 0xFF;
    let r = l.execute_bytes(&bytes).err();
    l.expect_fatal(r, fatal::BAD_BLOCK_ENCODING, blk);
    let mut b = l.build(false);
    b.entries.push(caravel_types::block::Entry::Oracle(
        l.oracle_update(ORACLE, BTC, P, l.now),
    ));
    let mut bytes = b.encode().unwrap();
    bytes[93 + 1..93 + 5].copy_from_slice(&113u32.to_le_bytes());
    bytes.truncate(bytes.len() - 1);
    let r = l.execute_bytes(&bytes).err();
    l.expect_fatal(r, fatal::BAD_ENTRY_ENCODING, 0);
    let empty = l.build(false).encode().unwrap();
    let r = l
        .exec
        .step(&l.state_bytes[..l.state_bytes.len() - 1], &empty)
        .err();
    l.expect_fatal(r, fatal::BAD_STATE_ENCODING, blk);

    // Block-level checks.
    let cases: [(BlockEdit, u16); 4] = [
        (|b| b.lane_id = [9; 32], fatal::WRONG_LANE_BLOCK),
        (|b| b.height += 1, fatal::BAD_HEIGHT),
        (|b| b.prev_block_hash = [7; 32], fatal::BAD_PREV_HASH),
        (|b| b.timestamp_ms = T0 - 1, fatal::TIME_REGRESSION),
    ];
    for (edit, code) in cases {
        let mut b = l.build(false);
        edit(&mut b);
        let r = l.execute(&b).err();
        l.expect_fatal(r, code, blk);
    }

    // Entry order, duplicate oracle market, inbox gap, bad signature.
    let st = l.state();
    let dep = InboxMsgV1 {
        kind: InboxKind::Deposit,
        index: st.inbox_through,
        lane_account: pk(B),
        amount: USDC,
        enqueued_at: 0,
    };
    let tx = l.signed(
        A,
        A,
        SigScheme::RawEd25519,
        l.account(A).unwrap().next_nonce,
        l.now + 60_000,
        gtc(Side::Buy, P - TICK, 1),
    );
    let mut b = l.build(false);
    b.entries = vec![
        caravel_types::block::Entry::User(tx),
        caravel_types::block::Entry::Inbox(dep),
    ];
    let r = l.execute(&b).err();
    l.expect_fatal(r, fatal::ENTRY_ORDER, 1);

    let mut b = l.build(false);
    b.entries = vec![
        caravel_types::block::Entry::Oracle(l.oracle_update(ORACLE, BTC, P, l.now)),
        caravel_types::block::Entry::Oracle(l.oracle_update(ORACLE, BTC, P, l.now)),
    ];
    let r = l.execute(&b).err();
    l.expect_fatal(r, fatal::DUPLICATE_ORACLE_MARKET, 1);

    let mut b = l.build(false);
    b.entries = vec![caravel_types::block::Entry::Inbox(InboxMsgV1 {
        index: st.inbox_through + 1,
        ..dep
    })];
    let r = l.execute(&b).err();
    l.expect_fatal(r, fatal::INBOX_GAP, 0);

    let mut bad = tx;
    bad.signature[0] ^= 1;
    let mut b = l.build(false);
    b.entries = vec![
        caravel_types::block::Entry::Inbox(dep),
        caravel_types::block::Entry::User(bad),
    ];
    let r = l.execute(&b).err();
    l.expect_fatal(r, fatal::BAD_SIGNATURE, 1);

    // Over max_block_bytes (24,000): 123 cancel transactions of 196 framed bytes.
    let n = l.account(A).unwrap().next_nonce;
    let mut b = l.build(false);
    for i in 0..123 {
        let t = l.signed(
            A,
            A,
            SigScheme::RawEd25519,
            n + i,
            l.now + 60_000,
            TxBody::CancelOrder {
                market_id: BTC,
                order_id: 1,
            },
        );
        b.entries.push(caravel_types::block::Entry::User(t));
    }
    assert!(b.encoded_len() > 24_000);
    let r = l.execute(&b).err();
    l.expect_fatal(r, fatal::BLOCK_TOO_LARGE, blk);

    // None of this moved the lane, and a good block still works.
    assert_eq!(l.state_hash(), good);
    l.order(A, BTC, Side::Buy, Tif::Gtc, P - TICK, 1);
    l.block();
    l.all_ok();

    // Too many entries: a lane that allows 2 per block.
    let mut cfg = config();
    cfg.max_entries_per_block = 2;
    let mut l2 = Lane::new(exec.clone(), cfg);
    l2.oracle(BTC, P);
    l2.oracle(ETH, ETH_PRICE);
    l2.oracle(XLM, XLM_PRICE);
    let r = l2.try_block(false).err();
    l2.expect_fatal(r, fatal::TOO_MANY_ENTRIES, blk);
    l2.oracle(BTC, P);
    l2.block();
    vec![l, l2]
}

const BLOCK_LEVEL_ENTRY: u32 = fatal::BLOCK_LEVEL;

/// An edit that makes an otherwise valid block fatal.
type BlockEdit = fn(&mut caravel_types::block::BlockInputV1);

// --- 16 ---------------------------------------------------------------------

fn withdraw_liquidity_and_cash<E: Executor + Clone>(exec: &E) -> Vec<Lane<E>> {
    // Unrealized profit cannot leave: amount ≤ collateral and ≤ free collateral.
    let mut l = setup(exec, config(), &[(D, 100 * USDC), (E, 1000 * USDC)]);
    l.order(E, BTC, Side::Sell, Tif::Gtc, P, 10);
    l.block();
    l.order(D, BTC, Side::Buy, Tif::Ioc, P, 10);
    l.block();
    let up = P + P / 20;
    l.oracle(BTC, up);
    l.block();
    let collateral = 100 * USDC - fee(10, P, 5);
    let equity = collateral + 10 * i128::from(up - P);
    let free = equity - 10 * i128::from(up) * 1000 / 10_000;
    assert_eq!(l.collateral(D), collateral);
    assert!(equity > 100 * USDC && free < 98 * USDC);
    l.withdraw(D, 100 * USDC); // more than collateral, less than equity
    l.withdraw(D, 98 * USDC); // within collateral, above free collateral
    l.withdraw(D, 90 * USDC);
    l.block();
    assert_eq!(
        l.user_codes(),
        vec![
            INSUFFICIENT_FREE_COLLATERAL,
            INSUFFICIENT_FREE_COLLATERAL,
            OK
        ]
    );

    // After a backstop deficit the winners hold more than the lane holds on Stellar.
    let mut l2 = setup(exec, config(), &[(A, 660 * USDC), (B, 2000 * USDC)]);
    l2.order(B, BTC, Side::Sell, Tif::Gtc, P, 1000);
    l2.block();
    l2.order(A, BTC, Side::Buy, Tif::Ioc, P, 1000);
    l2.block();
    l2.advance(20_000);
    l2.oracle(BTC, M2);
    l2.block();
    assert!(l2.state().backstop_deficit);
    // B takes profit against the backstop's reduce-only unwind at mark.
    l2.tx(B, gtc(Side::Buy, M2, 1000));
    l2.block();
    l2.reduce_only(BACKSTOP, BTC, Side::Sell, M2, 1000);
    l2.block();
    l2.all_ok();
    let b_cash = 2000 * USDC + 1000 * i128::from(P - M2);
    assert_eq!(l2.collateral(B), b_cash);
    let st = l2.state();
    let liquidity = st.deposits_credited_total - st.withdrawals_committed_total;
    assert!(b_cash > liquidity);
    l2.withdraw(B, b_cash);
    l2.withdraw(B, liquidity);
    l2.block();
    assert_eq!(l2.user_codes(), vec![INSUFFICIENT_LANE_LIQUIDITY, OK]);
    let st = l2.state();
    assert_eq!(
        st.pending.iter().map(|p| p.amount).sum::<i128>(),
        liquidity,
        "INV-P7 holds with equality"
    );
    vec![l, l2]
}

// --- 17 ---------------------------------------------------------------------

fn account_slot_reuse<E: Executor + Clone>(exec: &E) -> Vec<Lane<E>> {
    let mut cfg = config();
    cfg.max_accounts = 4;
    let mut l = Lane::new(exec.clone(), cfg);
    l.deposit(A, 10 * USDC);
    l.deposit(B, 10 * USDC);
    let t_a = l.now;
    l.block();
    assert_eq!((l.account_index(A), l.account_index(B)), (Some(2), Some(3)));
    // A second transaction A signs now but never submits.
    let replay = l.signed(
        A,
        A,
        SigScheme::RawEd25519,
        t_a + 1,
        u64::MAX,
        TxBody::CancelAll {
            market_id: ALL_MARKETS,
        },
    );

    // A leaves. While its withdrawal is pending the slot is not free.
    l.withdraw(A, 10 * USDC);
    l.block();
    l.all_ok();
    l.deposit(C, 10 * USDC);
    l.block();
    assert_eq!(
        l.events(0),
        &[Event::Deposit {
            key: pk(C),
            amount: 10 * USDC,
            outcome: DepositOutcome::Bounced
        }]
    );
    l.checkpoint();

    // Now slot 2 is empty: C takes it in place, with next_nonce = the block time.
    l.deposit(C, 10 * USDC);
    let t_c = l.now;
    l.block();
    assert_eq!(
        l.events(0),
        &[Event::Deposit {
            key: pk(C),
            amount: 10 * USDC,
            outcome: DepositOutcome::Created
        }]
    );
    let st = l.state();
    assert_eq!(st.accounts.len(), 4);
    assert_eq!(
        (st.accounts[2].key, st.accounts[2].next_nonce),
        (pk(C), t_c)
    );
    assert!(l.account(A).is_none());

    // C leaves; A comes back into the same slot with a fresh nonce.
    l.withdraw(C, 10 * USDC);
    l.checkpoint();
    l.deposit(A, 10 * USDC);
    let t_a2 = l.now;
    l.block();
    assert_eq!(l.account_index(A), Some(2));
    assert_eq!(l.account(A).unwrap().next_nonce, t_a2);
    // The transaction signed for A's earlier account cannot replay.
    l.push_tx(replay);
    l.block();
    assert_eq!(l.user_codes(), vec![BAD_NONCE]);
    vec![l]
}

// --- 18 ---------------------------------------------------------------------

fn oracle_breaker_widens<E: Executor + Clone>(exec: &E) -> Vec<Lane<E>> {
    let mut l = setup(exec, config(), &[(A, 10 * USDC)]);
    let up30 = P + P * 30 / 100;
    // One second after the last price: 30% is outside 10% + 0.1%/s.
    l.oracle(BTC, up30);
    l.block();
    assert_eq!(
        l.events(0),
        &[Event::Oracle {
            market: BTC,
            price: up30,
            accepted: false
        }]
    );
    // After an hour-long gap the bound is 10% + 360%: 30% is accepted.
    l.advance(3_600_000);
    l.oracle(BTC, up30);
    l.block();
    assert_eq!(
        l.events(0),
        &[Event::Oracle {
            market: BTC,
            price: up30,
            accepted: true
        }]
    );
    assert_eq!(l.state().markets[0].oracle_price, up30);
    // And one second later another 30% is rejected again.
    let up30_again = up30 + up30 * 30 / 100;
    l.oracle(BTC, up30_again);
    l.block();
    assert_eq!(
        l.events(0),
        &[Event::Oracle {
            market: BTC,
            price: up30_again,
            accepted: false
        }]
    );
    vec![l]
}
