//! A deterministic random workload for long runs and the native/Wasm parity
//! gate (spec §8.2, T-005). Unlike the proptest in caravel-perps it is seeded,
//! so a failing block can be replayed exactly.

use caravel_types::tx::{PlaceOrder, Side, SigScheme, Tif, TxBody, ALL_MARKETS};

use crate::lane::{seeds, Executor, Lane, BTC_PRICE, ETH_PRICE, TICK, USDC, XLM_PRICE};

/// SplitMix64.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.below((hi - lo + 1) as u64) as i64)
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

const USERS: [u8; 6] = [seeds::A, seeds::B, seeds::C, seeds::D, seeds::E, seeds::U];
const MARKETS: [u16; 3] = [1, 2, 3];

fn snap(price: i128) -> i64 {
    let p = i64::try_from(price).expect("price fits");
    (p / TICK).max(1) * TICK
}

/// Drives a lane: deposits, orders of every kind, cancels, withdrawals, forced
/// withdrawals, stale and bad-nonce transactions, ±3% oracle moves with an
/// occasional −9% shock, time jumps across funding boundaries, checkpoints.
/// Every block goes through [`Lane::block`], which checks the invariants.
pub fn drive<E: Executor>(lane: &mut Lane<E>, rng: &mut Rng, blocks: usize) {
    let mut prices = [BTC_PRICE, ETH_PRICE, XLM_PRICE];
    if lane.height() == 0 {
        for u in USERS {
            lane.deposit(u, 2000 * USDC);
        }
    } else {
        let st = lane.state();
        for (i, m) in st.markets.iter().enumerate() {
            if m.oracle_price > 0 {
                prices[i] = m.oracle_price;
            }
        }
    }
    for _ in 0..blocks {
        if rng.chance(8) {
            lane.advance(rng.below(1_500) * 1000);
        }
        let shock = rng.chance(2);
        for i in 0..3 {
            let bps = if shock { -900 } else { rng.range(-300, 300) };
            prices[i] = snap(i128::from(prices[i]) * i128::from(10_000 + bps) / 10_000);
            if rng.chance(95) {
                lane.oracle(MARKETS[i], prices[i]);
            }
        }
        for _ in 0..rng.below(8) {
            let user = USERS[rng.below(USERS.len() as u64) as usize];
            let m = rng.below(3) as usize;
            match rng.below(100) {
                0..=6 => {
                    lane.deposit(user, i128::from(rng.range(1, 3000) as i32) * USDC);
                }
                7..=9 => {
                    lane.forced_withdrawal(user, i128::from(rng.range(1, 500) as i32) * USDC);
                }
                10..=64 => {
                    let offset = rng.range(-560, 560);
                    let price = snap(i128::from(prices[m]) * i128::from(10_000 + offset) / 10_000);
                    let reduce_only = rng.chance(10);
                    let tif = if reduce_only {
                        Tif::Ioc
                    } else {
                        [Tif::Gtc, Tif::Ioc, Tif::PostOnly][rng.below(3) as usize]
                    };
                    let side = if rng.chance(50) {
                        Side::Sell
                    } else {
                        Side::Buy
                    };
                    let lots = rng.range(1, 1500);
                    lane.tx(
                        user,
                        TxBody::PlaceOrder(PlaceOrder {
                            market_id: MARKETS[m],
                            side,
                            tif,
                            reduce_only,
                            price,
                            lots,
                            client_order_id: rng.next_u64(),
                        }),
                    );
                }
                65..=74 => {
                    let st = lane.state();
                    let book = &st.markets[m];
                    let ids: Vec<u64> = book
                        .bids
                        .iter()
                        .chain(&book.asks)
                        .map(|o| o.order_id)
                        .collect();
                    let order_id = if ids.is_empty() {
                        1
                    } else {
                        ids[rng.below(ids.len() as u64) as usize]
                    };
                    lane.tx(
                        user,
                        TxBody::CancelOrder {
                            market_id: MARKETS[m],
                            order_id,
                        },
                    );
                }
                75..=78 => {
                    lane.tx(
                        user,
                        TxBody::CancelAll {
                            market_id: if rng.chance(70) {
                                ALL_MARKETS
                            } else {
                                MARKETS[m]
                            },
                        },
                    );
                }
                79..=90 => {
                    lane.tx_as(
                        user,
                        user,
                        SigScheme::Sep53,
                        TxBody::Withdraw {
                            amount: i128::from(rng.range(1, 400) as i32) * USDC,
                        },
                    );
                }
                91..=94 => {
                    // A stale nonce: rejected, not consumed.
                    let t = lane.signed(
                        user,
                        user,
                        SigScheme::RawEd25519,
                        1,
                        lane.now + 60_000,
                        TxBody::CancelAll {
                            market_id: ALL_MARKETS,
                        },
                    );
                    lane.push_tx(t);
                }
                _ => {
                    // Already expired.
                    let n = lane.next_nonce(user);
                    let t = lane.signed(
                        user,
                        user,
                        SigScheme::RawEd25519,
                        n,
                        lane.now.saturating_sub(1),
                        TxBody::CancelAll {
                            market_id: ALL_MARKETS,
                        },
                    );
                    lane.push_tx(t);
                }
            }
        }
        if rng.chance(12) {
            lane.checkpoint();
        } else {
            lane.block();
        }
    }
}
