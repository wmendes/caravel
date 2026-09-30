//! ORACLE entries (spec §11.5).

use alloc::vec;

use caravel_types::codes;
use caravel_types::fatal::{self, Fatal};
use caravel_types::fixed::div_floor;
use caravel_types::oracle::OracleUpdateV1;
use caravel_types::receipts::{Event, Receipt};

use crate::engine::{Engine, OrFatal, Res};
use crate::Crypto;

impl<C: Crypto> Engine<'_, C> {
    pub(crate) fn oracle(&mut self, entry: u32, u: &OracleUpdateV1) -> Res<()> {
        // Fatal: an unknown key or a bad signature. (A second entry for one
        // market is rejected in the block checks.)
        if self
            .st
            .config
            .oracle_keys
            .binary_search(&u.oracle_key)
            .is_err()
        {
            return Err(Fatal::entry(fatal::UNKNOWN_ORACLE_KEY, entry));
        }
        let msg = self.c.sha256(&u.signing_preimage(&self.st.lane_id));
        self.verify(entry, &u.oracle_key, &msg, &u.signature)?;

        let accepted = self.apply_oracle(entry, u)?;
        self.receipts.push(Receipt {
            entry_index: entry,
            code: codes::OK,
            events: vec![Event::Oracle {
                market: u.market_id,
                price: u.price,
                accepted,
            }],
        });
        Ok(())
    }

    /// The non-fatal checks; returns whether the price was accepted.
    fn apply_oracle(&mut self, entry: u32, u: &OracleUpdateV1) -> Res<bool> {
        let Some(m) = self.market_idx(u.market_id) else {
            return Ok(false);
        };
        let cfg = &self.st.config;
        let mk = &self.st.markets[m];
        if u.price <= 0 || u.publish_time_ms <= mk.oracle_time_ms {
            return Ok(false);
        }
        if u.publish_time_ms > self.now.saturating_add(cfg.oracle_max_future_ms) {
            return Ok(false);
        }
        if mk.oracle_price > 0 {
            // The allowed move widens with the time since the last accepted
            // update, so the price can recover after an outage.
            let old = i128::from(mk.oracle_price);
            let elapsed = i128::from(u.publish_time_ms - mk.oracle_time_ms);
            let allowed_bps = i128::from(cfg.oracle_breaker_bps_per_sec)
                .checked_mul(div_floor(elapsed, 1000).or_fatal(entry)?)
                .and_then(|x| x.checked_add(i128::from(cfg.oracle_circuit_breaker_bps)));
            let moved = (i128::from(u.price) - old).abs() * 10_000;
            // An overflowing bound is larger than any possible move.
            if let Some(bound) = allowed_bps.and_then(|b| b.checked_mul(old)) {
                if moved > bound {
                    return Ok(false);
                }
            }
        }
        let mk = &mut self.st.markets[m];
        mk.oracle_price = u.price;
        mk.oracle_time_ms = u.publish_time_ms;
        Ok(true)
    }
}
