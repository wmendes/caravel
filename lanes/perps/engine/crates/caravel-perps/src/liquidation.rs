//! Liquidations at the end of every block (spec §11.7). Positions move to the
//! backstop at mark, which keeps `Σ s = 0`; the backstop absorbs any deficit.

use alloc::vec::Vec;

use caravel_types::fatal::BLOCK_LEVEL;
use caravel_types::fixed::{mul_div_ceil, notional};
use caravel_types::receipts::{CancelReason, Event};
use caravel_types::state::BACKSTOP_INDEX;

use crate::engine::{Engine, OrFatal, Res};
use crate::margin::{account_margin_with, equity, resting_table};
use crate::Crypto;

impl<C: Crypto> Engine<'_, C> {
    pub(crate) fn liquidations(&mut self) -> Res<()> {
        let e = BLOCK_LEVEL;
        let backstop = BACKSTOP_INDEX as usize;
        let mut events = Vec::new();
        // Resting lots per account, once per block. Liquidating an account only
        // cancels its own orders, so the other rows stay correct.
        let resting = resting_table(&self.st).or_fatal(e)?;
        for (a, rest) in resting.iter().enumerate() {
            let account = &self.st.accounts[a];
            if account.system || account.positions.iter().all(|p| p.lots == 0) {
                continue;
            }
            // Skip while any market it holds a position in has a stale oracle.
            let open: Vec<usize> = (0..self.st.markets.len())
                .filter(|m| account.positions[*m].lots != 0)
                .collect();
            if open.iter().any(|m| !self.fresh(*m)) {
                continue;
            }
            let margin = account_margin_with(&self.st, a, rest, None).or_fatal(e)?;
            if margin.equity >= margin.maintenance {
                continue;
            }
            // 1. Cancel every open order.
            self.cancel_all_orders(e, a, CancelReason::Liquidation, &mut events)?;
            // 2. Fee from the pre-liquidation positions.
            let mut fee_due: i128 = 0;
            for &m in &open {
                let s = self.st.accounts[a].positions[m].lots;
                let price = self.st.markets[m].oracle_price;
                let bps = i128::from(self.st.config.markets[m].liq_fee_bps);
                fee_due = fee_due
                    .checked_add(mul_div_ceil(notional(s, price), bps, 10_000).or_fatal(e)?)
                    .or_fatal(e)?;
            }
            // 3. Transfer each position to the backstop at mark.
            for &m in &open {
                let s = self.st.accounts[a].positions[m].lots;
                let price = self.st.markets[m].oracle_price;
                self.fill(e, a, m, s.checked_neg().or_fatal(e)?, price)?;
                self.fill(e, backstop, m, s, price)?;
            }
            // 4. Collect the fee from what is left.
            let fee = fee_due.min(self.st.accounts[a].collateral.max(0));
            self.transfer(e, a, backstop, fee)?;
            // 5. The backstop absorbs a negative balance.
            let collateral = self.st.accounts[a].collateral;
            let deficit = if collateral < 0 {
                self.transfer(e, backstop, a, collateral.checked_neg().or_fatal(e)?)?;
                collateral.checked_neg().or_fatal(e)?
            } else {
                0
            };
            events.push(Event::Liquidation {
                account_idx: u32::try_from(a).or_fatal(e)?,
                fee,
                deficit,
            });
        }
        // The flag is informational: trading continues (DEC-014).
        let backstop_equity = equity(&self.st, backstop).or_fatal(e)?;
        let deficit_now = backstop_equity < 0;
        if deficit_now != self.st.backstop_deficit {
            self.st.backstop_deficit = deficit_now;
            events.push(Event::BackstopDeficit {
                active: deficit_now,
                backstop_equity,
            });
        }
        self.end_events.extend(events);
        Ok(())
    }
}
