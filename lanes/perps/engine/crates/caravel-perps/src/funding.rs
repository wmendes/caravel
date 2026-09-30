//! Funding at block start (spec §11.6). Payments sum to exactly zero because
//! `Σ s = 0` per market (INV-P2).

use alloc::vec::Vec;

use caravel_types::codes;
use caravel_types::fatal::BLOCK_LEVEL;
use caravel_types::fixed::{div_floor, div_trunc, mul_div_floor};
use caravel_types::receipts::{Event, Receipt, PSEUDO_BLOCK_START};
use caravel_types::state::OrderV1;

use crate::engine::{Engine, OrFatal, Res};
use crate::Crypto;

/// The price of the level at which cumulative depth first reaches `impact_lots`.
fn impact_price(book: &[OrderV1], impact_lots: i64) -> Option<i64> {
    let mut depth: i128 = 0;
    for o in book {
        depth += i128::from(o.lots_remaining);
        if depth >= i128::from(impact_lots) {
            return Some(o.price);
        }
    }
    None
}

impl<C: Crypto> Engine<'_, C> {
    pub(crate) fn funding(&mut self) -> Res<()> {
        let e = BLOCK_LEVEL;
        let mut events = Vec::new();
        let interval = self.st.config.funding_interval_ms;
        for m in 0..self.st.markets.len() {
            let mk = &self.st.markets[m];
            let due = mk
                .last_funding_time_ms
                .checked_add(interval)
                .is_some_and(|t| self.now >= t);
            if !self.fresh(m) || !due {
                continue;
            }
            let params = self.st.config.markets[m];
            let mark = i128::from(mk.oracle_price);
            let premium_ppm = match (
                impact_price(&mk.bids, params.impact_lots),
                impact_price(&mk.asks, params.impact_lots),
            ) {
                (Some(bid), Some(ask)) => {
                    let mid = div_floor(i128::from(bid) + i128::from(ask), 2).or_fatal(e)?;
                    mul_div_floor(mid - mark, 1_000_000, mark).or_fatal(e)?
                }
                _ => 0, // thin books pay no funding
            };
            let max_rate = i128::from(self.st.config.funding_max_rate_ppm);
            let rate_ppm = div_trunc(premium_ppm, i128::from(self.st.config.funding_damping))
                .or_fatal(e)?
                .clamp(-max_rate, max_rate);
            // Stroops per lot; > 0 means longs pay shorts.
            let fpl = mul_div_floor(rate_ppm, mark, 1_000_000).or_fatal(e)?;
            for account in &mut self.st.accounts {
                let s = i128::from(account.positions[m].lots);
                if s != 0 {
                    account.collateral = account
                        .collateral
                        .checked_sub(s.checked_mul(fpl).or_fatal(e)?)
                        .or_fatal(e)?;
                }
            }
            let mk = &mut self.st.markets[m];
            mk.cumulative_funding_per_lot =
                mk.cumulative_funding_per_lot.checked_add(fpl).or_fatal(e)?;
            mk.last_funding_time_ms = (self.now / interval) * interval;
            events.push(Event::Funding {
                market: params.market_id,
                rate_ppm: i32::try_from(rate_ppm).or_fatal(e)?,
                fpl: i64::try_from(fpl).or_fatal(e)?,
            });
        }
        if !events.is_empty() {
            self.receipts.push(Receipt {
                entry_index: PSEUDO_BLOCK_START,
                code: codes::OK,
                events,
            });
        }
        Ok(())
    }
}
