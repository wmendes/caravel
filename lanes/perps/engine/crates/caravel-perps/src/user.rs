//! USER entries: signed lane transactions (spec §11.3).
//!
//! Every check that can reject happens before any state change, so a rejected
//! transaction changes nothing except, after step 9, the nonce and the per-block
//! counter (spec §8.3).

use alloc::vec::Vec;

use caravel_types::codes;
use caravel_types::fixed::{is_multiple_of, mul_div_ceil};
use caravel_types::receipts::{CancelReason, Event, Receipt};
use caravel_types::state::{OrderV1, SessionKeyV1};
use caravel_types::tx::{
    sep53_preimage, sep53_tx_message, LaneTxV1, PlaceOrder, Side, SigScheme, Tif, TxBody,
    ALL_MARKETS, PERM_ALL,
};

use crate::book;
use crate::engine::{Engine, OrFatal, Res};
use crate::margin::{account_margin, resting_lots, ExtraOrder};
use crate::Crypto;

/// Longest session key lifetime (spec §11.3.3).
const MAX_SESSION_MS: u64 = 7 * 24 * 60 * 60 * 1000;

impl<C: Crypto> Engine<'_, C> {
    pub(crate) fn user(&mut self, entry: u32, tx: &LaneTxV1) -> Res<()> {
        let mut events = Vec::new();
        let code = self.user_tx(entry, tx, &mut events)?;
        if code != codes::OK {
            events.clear();
        }
        self.receipts.push(Receipt {
            entry_index: entry,
            code,
            events,
        });
        Ok(())
    }

    fn user_tx(&mut self, entry: u32, tx: &LaneTxV1, events: &mut Vec<Event>) -> Res<u16> {
        // 2. Lane.
        if tx.lane_id != self.st.lane_id {
            return Ok(codes::WRONG_LANE);
        }
        // 3. Account.
        let Some(&a) = self.index.get(&tx.account) else {
            return Ok(codes::UNKNOWN_ACCOUNT);
        };
        let a = a as usize;
        // 4. Signer rules.
        if tx.signer != tx.account && !self.session_key_allows(a, tx) {
            return Ok(codes::UNAUTHORIZED_SIGNER);
        }
        // 5. Signature (fatal on failure; the sequencer pre-verifies).
        let tx_hash = self.c.sha256(&tx.tx_hash_preimage(&self.st.config_hash));
        match tx.sig_scheme {
            SigScheme::RawEd25519 => self.verify(entry, &tx.signer, &tx_hash, &tx.signature)?,
            SigScheme::Sep53 => {
                let msg = self.c.sha256(&sep53_preimage(&sep53_tx_message(&tx_hash)));
                self.verify(entry, &tx.signer, &msg, &tx.signature)?;
            }
        }
        let account = &self.st.accounts[a];
        // 6–8. Not consumed on these rejections.
        if self.now > tx.expiry_ms {
            return Ok(codes::EXPIRED);
        }
        if tx.nonce != account.next_nonce {
            return Ok(codes::BAD_NONCE);
        }
        if account.txs_this_block >= self.st.config.max_txs_per_account_per_block {
            return Ok(codes::RATE_LIMITED);
        }
        // 9. Consume the nonce. From here on a rejection still consumes it.
        let account = &mut self.st.accounts[a];
        account.next_nonce = account.next_nonce.checked_add(1).or_fatal(entry)?;
        account.txs_this_block = account.txs_this_block.checked_add(1).or_fatal(entry)?;
        // 10. Dispatch.
        match tx.body {
            TxBody::PlaceOrder(o) => self.place_order(entry, a, &o, events),
            TxBody::CancelOrder {
                market_id,
                order_id,
            } => self.cancel_order(entry, a, market_id, order_id, events),
            TxBody::CancelAll { market_id } => self.cancel_all(entry, a, market_id, events),
            TxBody::Withdraw { amount } => self.withdraw(entry, a, amount),
            TxBody::AddSessionKey {
                session_key,
                expires_at_ms,
                permissions,
            } => Ok(self.add_session_key(a, &tx.account, session_key, expires_at_ms, permissions)),
            TxBody::RevokeSessionKey { session_key } => {
                Ok(self.revoke_session_key(a, &session_key))
            }
        }
    }

    /// A session key may sign if it is registered, unexpired, has the kind's
    /// permission, and the scheme is raw ed25519 (SEP-53 is owner only).
    fn session_key_allows(&self, a: usize, tx: &LaneTxV1) -> bool {
        let Some(perm) = tx.kind().session_permission() else {
            return false;
        };
        if tx.sig_scheme == SigScheme::Sep53 {
            return false;
        }
        let keys = &self.st.accounts[a].session_keys;
        keys.binary_search_by(|k| k.key.cmp(&tx.signer))
            .ok()
            .map(|i| &keys[i])
            .is_some_and(|k| k.expires_at_ms > self.now && k.permissions & perm != 0)
    }

    // --- PLACE_ORDER (spec §11.3.1) -----------------------------------------

    fn place_order(
        &mut self,
        entry: u32,
        a: usize,
        o: &PlaceOrder,
        events: &mut Vec<Event>,
    ) -> Res<u16> {
        let code = self.check_order(a, o);
        if code != codes::OK {
            return Ok(code);
        }
        self.match_order(entry, a, o, events)?;
        Ok(codes::OK)
    }

    /// Checks 1–9 in order; returns the first failing code or `OK`.
    fn check_order(&self, a: usize, o: &PlaceOrder) -> u16 {
        // 1. Market.
        let Some(m) = self.market_idx(o.market_id) else {
            return codes::UNKNOWN_MARKET;
        };
        let params = &self.st.config.markets[m];
        let mk = &self.st.markets[m];
        // 2. Price and size.
        if o.price <= 0 || !is_multiple_of(o.price, params.tick) {
            return codes::BAD_PRICE;
        }
        if o.lots <= 0 || o.lots > params.max_position_lots {
            return codes::BAD_SIZE;
        }
        let s = i128::from(self.st.accounts[a].positions[m].lots);
        let lots = i128::from(o.lots);
        // 3. Reduce-only: IOC, opposite side, no bigger than the position.
        if o.reduce_only {
            let opposite = match o.side {
                Side::Buy => s < 0,
                Side::Sell => s > 0,
            };
            if o.tif != Tif::Ioc || !opposite || lots > s.abs() {
                return codes::REDUCE_ONLY_VIOLATION;
            }
        }
        // 4. Staleness (skipped for reduce-only).
        if !o.reduce_only && !self.fresh(m) {
            return codes::ORACLE_STALE;
        }
        // 5. Price band around mark = oracle price.
        let mark = i128::from(mk.oracle_price);
        if (i128::from(o.price) - mark).abs() * 10_000 > i128::from(params.band_bps) * mark {
            return codes::OUTSIDE_PRICE_BAND;
        }
        // 6. Post-only must not cross.
        let (same, opposite) = match o.side {
            Side::Buy => (&mk.bids, &mk.asks),
            Side::Sell => (&mk.asks, &mk.bids),
        };
        if o.tif == Tif::PostOnly && book::crosses(opposite, o.side, o.price) {
            return codes::POST_ONLY_WOULD_CROSS;
        }
        // 7. Room to rest.
        if o.tif != Tif::Ioc {
            if self.st.accounts[a].open_order_count >= self.st.config.max_open_orders_per_account {
                return codes::TOO_MANY_OPEN_ORDERS;
            }
            if same.len() >= self.st.config.max_orders_per_side as usize {
                return codes::BOOK_FULL;
            }
        }
        if o.reduce_only {
            return codes::OK;
        }
        // 8. Position and open interest, worst case including resting orders.
        let Ok((bids, asks)) = resting_lots(&self.st, m, a as u32) else {
            return codes::POSITION_LIMIT;
        };
        let (bids, asks) = match o.side {
            Side::Buy => (bids + lots, asks),
            Side::Sell => (bids, asks + lots),
        };
        if (s + bids).abs().max((s - asks).abs()) > i128::from(params.max_position_lots) {
            return codes::POSITION_LIMIT;
        }
        if i128::from(mk.open_interest_lots) + lots > i128::from(params.max_oi_lots) {
            return codes::OPEN_INTEREST_LIMIT;
        }
        // 9. Margin, worst case (§11.3.4). Arithmetic overflow here is a rejection.
        let worst = || -> Option<i128> {
            let margin = account_margin(
                &self.st,
                a,
                Some(ExtraOrder {
                    market: m,
                    side: o.side,
                    lots: o.lots,
                }),
            )
            .ok()?;
            let limit = i128::from(o.price);
            let fee_max = mul_div_ceil(
                lots.checked_mul(limit)?,
                i128::from(params.taker_fee_bps),
                10_000,
            )
            .ok()?;
            let adverse = match o.side {
                Side::Buy => lots.checked_mul(limit - mark)?,
                Side::Sell => lots.checked_mul(mark - limit)?,
            }
            .max(0);
            margin
                .free_collateral()
                .ok()?
                .checked_sub(fee_max)?
                .checked_sub(adverse)
        };
        match worst() {
            Some(free) if free >= 0 => codes::OK,
            _ => codes::INSUFFICIENT_MARGIN,
        }
    }

    /// Price-time matching at the maker's price, then rest or cancel the remainder.
    fn match_order(
        &mut self,
        entry: u32,
        a: usize,
        o: &PlaceOrder,
        events: &mut Vec<Event>,
    ) -> Res<()> {
        let m = self.market_idx(o.market_id).or_fatal(entry)?;
        let params = self.st.config.markets[m];
        let taker_idx = u32::try_from(a).or_fatal(entry)?;
        let mut remaining = o.lots;
        while remaining > 0 {
            let opposite = match o.side {
                Side::Buy => &self.st.markets[m].asks,
                Side::Sell => &self.st.markets[m].bids,
            };
            if !book::crosses(opposite, o.side, o.price) {
                break;
            }
            let maker = opposite[0];
            let maker_a = maker.account_index as usize;
            if maker.account_index == taker_idx {
                // Self-trade prevention: cancel the resting order.
                self.opposite_book(m, o.side).remove(0);
                let count = &mut self.st.accounts[a].open_order_count;
                *count = count.checked_sub(1).or_fatal(entry)?;
                events.push(Event::OrderCanceled {
                    market: o.market_id,
                    order_id: maker.order_id,
                    reason: CancelReason::SelfTrade,
                });
                continue;
            }
            let q = remaining.min(maker.lots_remaining);
            let taker_delta = match o.side {
                Side::Buy => q,
                Side::Sell => q.checked_neg().or_fatal(entry)?,
            };
            self.fill(entry, a, m, taker_delta, maker.price)?;
            self.fill(
                entry,
                maker_a,
                m,
                taker_delta.checked_neg().or_fatal(entry)?,
                maker.price,
            )?;
            self.charge_fee(entry, a, q, maker.price, params.taker_fee_bps)?;
            self.charge_fee(entry, maker_a, q, maker.price, params.maker_fee_bps)?;
            let book = self.opposite_book(m, o.side);
            book[0].lots_remaining = book[0].lots_remaining.checked_sub(q).or_fatal(entry)?;
            if book[0].lots_remaining == 0 {
                book.remove(0);
                let count = &mut self
                    .st
                    .accounts
                    .get_mut(maker_a)
                    .or_fatal(entry)?
                    .open_order_count;
                *count = count.checked_sub(1).or_fatal(entry)?;
            }
            remaining = remaining.checked_sub(q).or_fatal(entry)?;
            events.push(Event::Fill {
                market: o.market_id,
                maker_order_id: maker.order_id,
                maker_idx: maker.account_index,
                taker_idx,
                price: maker.price,
                lots: q,
                taker_side: o.side,
            });
        }
        if remaining > 0 {
            match o.tif {
                Tif::Gtc | Tif::PostOnly => {
                    let order_id = self.st.next_order_id;
                    self.st.next_order_id = order_id.checked_add(1).or_fatal(entry)?;
                    let order = OrderV1 {
                        order_id,
                        account_index: taker_idx,
                        price: o.price,
                        lots_remaining: remaining,
                        client_order_id: o.client_order_id,
                    };
                    let own = match o.side {
                        Side::Buy => &mut self.st.markets[m].bids,
                        Side::Sell => &mut self.st.markets[m].asks,
                    };
                    book::insert(own, o.side, order);
                    let count = &mut self.st.accounts[a].open_order_count;
                    *count = count.checked_add(1).or_fatal(entry)?;
                    events.push(Event::OrderRested {
                        market: o.market_id,
                        order_id,
                        account_idx: taker_idx,
                        side: o.side,
                        price: o.price,
                        lots: remaining,
                    });
                }
                // An IOC order never rests, so it never gets an order id (DEC-023).
                Tif::Ioc => events.push(Event::OrderCanceled {
                    market: o.market_id,
                    order_id: 0,
                    reason: CancelReason::IocRemainder,
                }),
            }
        }
        Ok(())
    }

    fn opposite_book(&mut self, m: usize, side: Side) -> &mut Vec<OrderV1> {
        match side {
            Side::Buy => &mut self.st.markets[m].asks,
            Side::Sell => &mut self.st.markets[m].bids,
        }
    }

    // --- Cancels ---------------------------------------------------------------

    fn cancel_order(
        &mut self,
        entry: u32,
        a: usize,
        market_id: u16,
        order_id: u64,
        events: &mut Vec<Event>,
    ) -> Res<u16> {
        let Some(m) = self.market_idx(market_id) else {
            return Ok(codes::ORDER_NOT_FOUND);
        };
        let account_index = u32::try_from(a).or_fatal(entry)?;
        let market = &mut self.st.markets[m];
        for book in [&mut market.bids, &mut market.asks] {
            if let Some(i) = book
                .iter()
                .position(|o| o.order_id == order_id && o.account_index == account_index)
            {
                book.remove(i);
                let count = &mut self.st.accounts[a].open_order_count;
                *count = count.checked_sub(1).or_fatal(entry)?;
                events.push(Event::OrderCanceled {
                    market: market_id,
                    order_id,
                    reason: CancelReason::User,
                });
                return Ok(codes::OK);
            }
        }
        Ok(codes::ORDER_NOT_FOUND)
    }

    fn cancel_all(
        &mut self,
        entry: u32,
        a: usize,
        market_id: u16,
        events: &mut Vec<Event>,
    ) -> Res<u16> {
        if market_id == ALL_MARKETS {
            self.cancel_all_orders(entry, a, CancelReason::User, events)?;
            return Ok(codes::OK);
        }
        // A market id that is neither ALL_MARKETS nor configured (DEC-023).
        let Some(m) = self.market_idx(market_id) else {
            return Ok(codes::UNKNOWN_MARKET);
        };
        self.cancel_account_orders(entry, m, a, CancelReason::User, events)?;
        Ok(codes::OK)
    }

    // --- WITHDRAW (spec §11.3.2) -------------------------------------------------

    fn withdraw(&mut self, entry: u32, a: usize, amount: i128) -> Res<u16> {
        if amount < self.st.config.min_withdrawal {
            return Ok(codes::BELOW_MIN_WITHDRAWAL);
        }
        if self.st.pending.len() >= self.st.config.max_pending_withdrawals as usize {
            return Ok(codes::WITHDRAWAL_QUEUE_FULL);
        }
        let account = &self.st.accounts[a];
        if (0..self.st.markets.len()).any(|m| account.positions[m].lots != 0 && !self.fresh(m)) {
            return Ok(codes::ORACLE_STALE);
        }
        // Only realized cash can leave, and only what initial margin leaves free.
        if amount > account.collateral {
            return Ok(codes::INSUFFICIENT_FREE_COLLATERAL);
        }
        match account_margin(&self.st, a, None).and_then(|m| m.free_collateral()) {
            Ok(free) if amount <= free => {}
            _ => return Ok(codes::INSUFFICIENT_FREE_COLLATERAL),
        }
        // INV-P7: Σ pending never exceeds what the lane holds on Stellar.
        if amount > self.liquidity_left(entry)? {
            return Ok(codes::INSUFFICIENT_LANE_LIQUIDITY);
        }
        let key = self.st.accounts[a].key;
        let c = &mut self.st.accounts[a].collateral;
        *c = c.checked_sub(amount).or_fatal(entry)?;
        self.push_pending(entry, key, amount)?;
        Ok(codes::OK)
    }

    // --- Session keys (spec §11.3.3) -------------------------------------------

    fn add_session_key(
        &mut self,
        a: usize,
        owner: &[u8; 32],
        key: [u8; 32],
        expires_at_ms: u64,
        permissions: u8,
    ) -> u16 {
        let keys = &self.st.accounts[a].session_keys;
        let Err(at) = keys.binary_search_by(|k| k.key.cmp(&key)) else {
            return codes::BAD_SESSION_KEY; // already registered
        };
        if key == *owner {
            return codes::BAD_SESSION_KEY;
        }
        if expires_at_ms <= self.now || expires_at_ms > self.now.saturating_add(MAX_SESSION_MS) {
            return codes::BAD_SESSION_KEY;
        }
        if permissions & !PERM_ALL != 0 {
            return codes::BAD_SESSION_KEY;
        }
        if keys.len() >= usize::from(self.st.config.max_session_keys) {
            return codes::TOO_MANY_SESSION_KEYS;
        }
        self.st.accounts[a].session_keys.insert(
            at,
            SessionKeyV1 {
                key,
                expires_at_ms,
                permissions,
            },
        );
        codes::OK
    }

    fn revoke_session_key(&mut self, a: usize, key: &[u8; 32]) -> u16 {
        let keys = &mut self.st.accounts[a].session_keys;
        match keys.binary_search_by(|k| k.key.cmp(key)) {
            Ok(i) => {
                keys.remove(i);
                codes::OK
            }
            Err(_) => codes::BAD_SESSION_KEY,
        }
    }
}
