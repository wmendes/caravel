//! Property tests (spec §19.4): random sequences of up to 200 blocks with
//! deposits, orders, cancels, oracle moves (±3% per block), withdrawals,
//! forced withdrawals, checkpoints and funding crossings. The lane checks
//! INV-P1…P4, P7 and P8 after every block; a fatal on generated input fails
//! the test, because the generator only builds well-formed, signed blocks.

use caravel_testkit::lane::seeds::{A, B, C, D};
use caravel_testkit::lane::{config, BTC_PRICE, ETH_PRICE, TICK, USDC, XLM_PRICE};
use caravel_testkit::Lane;
use caravel_types::receipts::Event;
use caravel_types::tx::{PlaceOrder, Side, SigScheme, Tif, TxBody, ALL_MARKETS};
use proptest::collection::vec;
use proptest::prelude::*;

const USERS: [u8; 4] = [A, B, C, D];
const MARKETS: [u16; 3] = [1, 2, 3];

#[derive(Clone, Debug)]
enum Action {
    Deposit {
        user: usize,
        usdc: u16,
    },
    Order {
        user: usize,
        market: usize,
        sell: bool,
        tif: u8,
        reduce_only: bool,
        offset_bps: i16,
        lots: u16,
    },
    Cancel {
        user: usize,
        market: usize,
        pick: u16,
    },
    CancelAll {
        user: usize,
    },
    Withdraw {
        user: usize,
        usdc: u16,
    },
    Forced {
        user: usize,
        usdc: u16,
    },
}

#[derive(Clone, Debug)]
struct Plan {
    /// Extra time before the block, in seconds.
    jump_s: u32,
    /// Oracle move per market, in bps.
    moves: [i16; 3],
    checkpoint: bool,
    actions: Vec<Action>,
}

fn action() -> impl Strategy<Value = Action> {
    let user = 0usize..4;
    let market = 0usize..3;
    prop_oneof![
        1 => (user.clone(), 1u16..3000).prop_map(|(user, usdc)| Action::Deposit { user, usdc }),
        6 => (user.clone(), market.clone(), any::<bool>(), 0u8..3, prop::bool::weighted(0.15), -550i16..550, 1u16..1200)
            .prop_map(|(user, market, sell, tif, reduce_only, offset_bps, lots)| Action::Order { user, market, sell, tif, reduce_only, offset_bps, lots }),
        2 => (user.clone(), market, any::<u16>()).prop_map(|(user, market, pick)| Action::Cancel { user, market, pick }),
        1 => user.clone().prop_map(|user| Action::CancelAll { user }),
        1 => (user.clone(), 1u16..500).prop_map(|(user, usdc)| Action::Withdraw { user, usdc }),
        1 => (user, 1u16..500).prop_map(|(user, usdc)| Action::Forced { user, usdc }),
    ]
}

fn plan() -> impl Strategy<Value = Plan> {
    (
        prop_oneof![8 => Just(0u32), 2 => 60u32..1200],
        [-300i16..=300, -300i16..=300, -300i16..=300],
        prop::bool::weighted(0.1),
        vec(action(), 0..7),
    )
        .prop_map(|(jump_s, moves, checkpoint, actions)| Plan {
            jump_s,
            moves,
            checkpoint,
            actions,
        })
}

fn snap(price: i128) -> i64 {
    let p = i64::try_from(price).expect("price fits");
    (p / TICK).max(1) * TICK
}

/// What a run exercised, counted from the receipts.
#[derive(Debug, Default)]
struct Stats {
    fills: usize,
    rested: usize,
    liquidations: usize,
    funding_nonzero: usize,
    commitments: usize,
    rejected: usize,
    codes: std::collections::BTreeMap<u16, usize>,
}

fn tally(stats: &mut Stats, lane: &Lane) {
    for r in &lane.receipts.receipts {
        if r.code != 0 {
            stats.rejected += 1;
            *stats.codes.entry(r.code).or_default() += 1;
        }
        for e in &r.events {
            match e {
                Event::Fill { .. } => stats.fills += 1,
                Event::OrderRested { .. } => stats.rested += 1,
                Event::Liquidation { .. } => stats.liquidations += 1,
                Event::Funding { fpl, .. } if *fpl != 0 => stats.funding_nonzero += 1,
                Event::Commitment { .. } => stats.commitments += 1,
                _ => {}
            }
        }
    }
}

fn run(plans: &[Plan]) -> Stats {
    let mut stats = Stats::default();
    let mut lane = Lane::native(config());
    let mut prices = [BTC_PRICE, ETH_PRICE, XLM_PRICE];
    for u in USERS {
        lane.deposit(u, 1000 * USDC);
    }
    for (i, m) in MARKETS.iter().enumerate() {
        lane.oracle(*m, prices[i]);
    }
    lane.block();

    for plan in plans {
        lane.advance(u64::from(plan.jump_s) * 1000);
        for i in 0..3 {
            prices[i] = snap(i128::from(prices[i]) * i128::from(10_000 + plan.moves[i]) / 10_000);
            lane.oracle(MARKETS[i], prices[i]);
        }
        for a in &plan.actions {
            match *a {
                Action::Deposit { user, usdc } => {
                    lane.deposit(USERS[user], i128::from(usdc) * USDC);
                }
                Action::Forced { user, usdc } => {
                    lane.forced_withdrawal(USERS[user], i128::from(usdc) * USDC);
                }
                Action::Order {
                    user,
                    market,
                    sell,
                    tif,
                    reduce_only,
                    offset_bps,
                    lots,
                } => {
                    let price =
                        snap(i128::from(prices[market]) * i128::from(10_000 + offset_bps) / 10_000);
                    let tif = [Tif::Gtc, Tif::Ioc, Tif::PostOnly][usize::from(tif)];
                    let o = PlaceOrder {
                        market_id: MARKETS[market],
                        side: if sell { Side::Sell } else { Side::Buy },
                        tif: if reduce_only { Tif::Ioc } else { tif },
                        reduce_only,
                        price,
                        lots: i64::from(lots),
                        client_order_id: u64::from(lots),
                    };
                    lane.tx(USERS[user], TxBody::PlaceOrder(o));
                }
                Action::Cancel { user, market, pick } => {
                    let st = lane.state();
                    let mk = &st.markets[market];
                    let ids: Vec<u64> =
                        mk.bids.iter().chain(&mk.asks).map(|o| o.order_id).collect();
                    let order_id = if ids.is_empty() {
                        1
                    } else {
                        ids[usize::from(pick) % ids.len()]
                    };
                    lane.tx(
                        USERS[user],
                        TxBody::CancelOrder {
                            market_id: MARKETS[market],
                            order_id,
                        },
                    );
                }
                Action::CancelAll { user } => {
                    lane.tx(
                        USERS[user],
                        TxBody::CancelAll {
                            market_id: ALL_MARKETS,
                        },
                    );
                }
                Action::Withdraw { user, usdc } => {
                    lane.tx_as(
                        USERS[user],
                        USERS[user],
                        SigScheme::Sep53,
                        TxBody::Withdraw {
                            amount: i128::from(usdc) * USDC,
                        },
                    );
                }
            }
        }
        // Panics on a fatal; the lane checks every invariant after the block.
        if plan.checkpoint {
            lane.checkpoint();
        } else {
            lane.block();
        }
        tally(&mut stats, &lane);
    }
    // A final checkpoint commits whatever is pending.
    lane.checkpoint();
    tally(&mut stats, &lane);
    stats
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, max_shrink_iters: 200, ..ProptestConfig::default() })]

    #[test]
    fn invariants_hold_for_random_blocks(plans in vec(plan(), 1..=200)) {
        run(&plans);
    }
}

/// A fixed long run that must reach every interesting path: two users make
/// markets with asymmetric quotes (so the funding premium is not zero once the
/// book is deeper than impact_lots), two users take, and a −9% shock every 60
/// blocks liquidates the most levered taker.
#[test]
fn long_run_crosses_funding_and_liquidates() {
    let quote = |user, market, sell, offset_bps, lots| Action::Order {
        user,
        market,
        sell,
        tif: 0,
        reduce_only: false,
        offset_bps,
        lots,
    };
    let take = |user, market, sell, lots| Action::Order {
        user,
        market,
        sell,
        tif: 1,
        reduce_only: false,
        offset_bps: if sell { -100 } else { 100 },
        lots,
    };
    let mut plans = Vec::new();
    for i in 0..400u32 {
        let maker = (i % 2) as usize;
        let taker = 2 + (i % 2) as usize;
        let market = (i % 3) as usize;
        let mut actions = vec![
            quote(maker, market, false, -10, 600),
            quote(maker, market, true, 60, 600),
            take(taker, market, i % 3 != 0, 250),
            // User D only ever buys BTC, so the shock pushes it under maintenance.
            take(3, 0, false, 120),
        ];
        if i % 7 == 6 {
            actions.push(Action::CancelAll { user: maker });
        }
        if i % 10 == 0 {
            for user in 0..3 {
                actions.push(Action::Deposit { user, usdc: 1500 });
            }
            actions.push(Action::Withdraw {
                user: taker,
                usdc: 20,
            });
        }
        let shock = i % 60 == 59;
        let moves = if shock {
            [-900, -900, -900]
        } else {
            [(i % 5) as i16 * 10 - 20, 15, -15]
        };
        plans.push(Plan {
            jump_s: if i % 20 == 0 { 1800 } else { 0 },
            moves,
            checkpoint: i % 10 == 9,
            actions,
        });
    }
    let stats = run(&plans);
    // The run must reach the interesting paths, not just pass vacuously.
    assert!(stats.fills > 200 && stats.rested > 200, "{stats:?}");
    assert!(stats.liquidations > 0, "{stats:?}");
    assert!(stats.funding_nonzero > 0, "{stats:?}");
    assert!(stats.commitments >= 40, "{stats:?}");
    assert!(stats.rejected > 0, "{stats:?}");
    eprintln!("long run: {stats:?}");
}
