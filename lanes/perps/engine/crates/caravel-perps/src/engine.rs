//! `genesis`, `step` and the state transitions shared by the entry handlers.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec;
use alloc::vec::Vec;

use caravel_types::block::{block_hash_preimage, BlockDecodeError, BlockInputV1, Entry, EntryType};
use caravel_types::codes;
use caravel_types::config::GenesisConfigV1;
use caravel_types::fatal::{self, Fatal};
use caravel_types::fixed::{mul_div_ceil, mul_div_floor, ArithError};
use caravel_types::receipts::{CancelReason, Event, Receipt, Receipts, PSEUDO_BLOCK_END};
use caravel_types::state::{
    AccountV1, CommitmentV1, MarketStateV1, OrderV1, PendingWithdrawalV1, PositionV1, StateV1,
    BACKSTOP_INDEX, TREASURY_INDEX,
};

use crate::{Crypto, StepOutput};

pub(crate) type Res<T> = Result<T, Fatal>;

/// Maps an arithmetic failure in a state transition to `ARITHMETIC_OVERFLOW`
/// for `entry` (an invariant-level overflow, not a user rejection).
pub(crate) trait OrFatal<T> {
    fn or_fatal(self, entry: u32) -> Res<T>;
}

impl<T> OrFatal<T> for Result<T, ArithError> {
    fn or_fatal(self, entry: u32) -> Res<T> {
        self.map_err(|_| Fatal::entry(fatal::ARITHMETIC_OVERFLOW, entry))
    }
}

impl<T> OrFatal<T> for Result<T, core::num::TryFromIntError> {
    fn or_fatal(self, entry: u32) -> Res<T> {
        self.map_err(|_| Fatal::entry(fatal::ARITHMETIC_OVERFLOW, entry))
    }
}

impl<T> OrFatal<T> for Option<T> {
    fn or_fatal(self, entry: u32) -> Res<T> {
        self.ok_or(Fatal::entry(fatal::ARITHMETIC_OVERFLOW, entry))
    }
}

/// Adapts [`Crypto`] to the Merkle hasher trait.
pub(crate) struct Hasher<'a, C: Crypto>(pub &'a C);

impl<C: Crypto> caravel_merkle::Sha256 for Hasher<'_, C> {
    fn hash(&self, data: &[u8]) -> [u8; 32] {
        self.0.sha256(data)
    }
}

/// One block's execution context.
pub(crate) struct Engine<'a, C: Crypto> {
    pub c: &'a C,
    pub st: StateV1,
    /// Account key → index, rebuilt on decode (spec §9.10).
    pub index: BTreeMap<[u8; 32], u32>,
    /// `block.timestamp_ms`.
    pub now: u64,
    pub receipts: Vec<Receipt>,
    /// Events for the block-end pseudo entry: liquidations, backstop flag, commitment.
    pub end_events: Vec<Event>,
}

pub(crate) fn genesis(config_bytes: &[u8], c: &impl Crypto) -> Res<Vec<u8>> {
    let bad = Fatal::block(fatal::BAD_CONFIG);
    let config = GenesisConfigV1::decode(config_bytes).map_err(|_| bad)?;
    config.validate().map_err(|_| bad)?;
    let n = config.markets.len();
    let system = |key| AccountV1 {
        key,
        system: true,
        next_nonce: 0,
        collateral: 0,
        open_order_count: 0,
        session_keys: Vec::new(),
        positions: vec![PositionV1::default(); n],
        txs_this_block: 0,
    };
    let st = StateV1 {
        lane_id: config.lane_id,
        config_hash: c.sha256(config_bytes),
        height: 0,
        last_block_input_hash: [0; 32],
        last_timestamp_ms: 0,
        checkpoint_seq: 0,
        inbox_through: 0,
        inbox_acc: [0; 32],
        next_order_id: 1,
        deposits_credited_total: 0,
        withdrawals_committed_total: 0,
        backstop_deficit: false,
        accounts: vec![system(config.backstop_key), system(config.treasury_key)],
        markets: vec![MarketStateV1::default(); n],
        pending: Vec::new(),
        last_commitment: CommitmentV1::default(),
        config,
    };
    st.encode().map_err(|_| bad)
}

pub(crate) fn step(state_bytes: &[u8], block_bytes: &[u8], c: &impl Crypto) -> Res<StepOutput> {
    // 1. Decode.
    let st = StateV1::decode(state_bytes).map_err(|_| Fatal::block(fatal::BAD_STATE_ENCODING))?;
    let block = BlockInputV1::decode(block_bytes).map_err(|e| match e {
        BlockDecodeError::Header(_) => Fatal::block(fatal::BAD_BLOCK_ENCODING),
        BlockDecodeError::Entry { index, .. } => Fatal::entry(fatal::BAD_ENTRY_ENCODING, index),
    })?;
    let mut index = BTreeMap::new();
    for (i, a) in st.accounts.iter().enumerate() {
        let i = u32::try_from(i).map_err(|_| Fatal::block(fatal::BAD_STATE_ENCODING))?;
        if index.insert(a.key, i).is_some() {
            return Err(Fatal::block(fatal::BAD_STATE_ENCODING));
        }
    }

    // 2. Block-level checks.
    check_block(&st, &block, state_bytes, block_bytes, c)?;

    let mut e = Engine {
        c,
        st,
        index,
        now: block.timestamp_ms,
        receipts: Vec::new(),
        end_events: Vec::new(),
    };
    // 3. Reset per-block counters.
    for a in &mut e.st.accounts {
        a.txs_this_block = 0;
    }
    // 4. Funding.
    e.funding()?;
    // 5–7. INBOX, ORACLE, USER entries (order checked above).
    for (i, entry) in block.entries.iter().enumerate() {
        let idx = u32::try_from(i).map_err(|_| Fatal::block(fatal::TOO_MANY_ENTRIES))?;
        match entry {
            Entry::Inbox(m) => e.inbox(idx, m)?,
            Entry::Oracle(u) => e.oracle(idx, u)?,
            Entry::User(tx) => e.user(idx, tx)?,
        }
    }
    // 8. Liquidations.
    e.liquidations()?;
    // 9. Commitment.
    if block.checkpoint_end {
        e.commitment(block.height)?;
    }
    if !e.end_events.is_empty() {
        let events = core::mem::take(&mut e.end_events);
        e.receipts.push(Receipt {
            entry_index: PSEUDO_BLOCK_END,
            code: codes::OK,
            events,
        });
    }
    // 10. Advance.
    e.st.height = block.height;
    e.st.last_timestamp_ms = block.timestamp_ms;
    e.st.last_block_input_hash = c.sha256(block_bytes);
    // 11. Encode.
    let overflow = Fatal::block(fatal::ARITHMETIC_OVERFLOW);
    let state = e.st.encode().map_err(|_| overflow)?;
    let receipts = Receipts {
        receipts: e.receipts,
    }
    .encode()
    .map_err(|_| overflow)?;
    Ok(StepOutput { state, receipts })
}

fn check_block(
    st: &StateV1,
    block: &BlockInputV1,
    state_bytes: &[u8],
    block_bytes: &[u8],
    c: &impl Crypto,
) -> Res<()> {
    let fatal_block = |code| Err(Fatal::block(code));
    if block.lane_id != st.lane_id {
        return fatal_block(fatal::WRONG_LANE_BLOCK);
    }
    if st.height.checked_add(1) != Some(block.height) {
        return fatal_block(fatal::BAD_HEIGHT);
    }
    // The previous block's block_hash = H(its input hash || H(state it produced)),
    // and the state it produced is this block's input state.
    let expected_prev = if st.height == 0 {
        [0; 32]
    } else {
        c.sha256(&block_hash_preimage(
            &st.last_block_input_hash,
            &c.sha256(state_bytes),
        ))
    };
    if block.prev_block_hash != expected_prev {
        return fatal_block(fatal::BAD_PREV_HASH);
    }
    if block.timestamp_ms < st.last_timestamp_ms {
        return fatal_block(fatal::TIME_REGRESSION);
    }
    if block_bytes.len() > st.config.max_block_bytes as usize {
        return fatal_block(fatal::BLOCK_TOO_LARGE);
    }
    if block.entries.len() > st.config.max_entries_per_block as usize {
        return fatal_block(fatal::TOO_MANY_ENTRIES);
    }
    let mut last = EntryType::Inbox;
    let mut oracle_markets = BTreeSet::new();
    for (i, entry) in block.entries.iter().enumerate() {
        let idx = i as u32;
        let t = entry.entry_type();
        if t < last {
            return Err(Fatal::entry(fatal::ENTRY_ORDER, idx));
        }
        last = t;
        if let Entry::Oracle(u) = entry {
            if !oracle_markets.insert(u.market_id) {
                return Err(Fatal::entry(fatal::DUPLICATE_ORACLE_MARKET, idx));
            }
        }
    }
    Ok(())
}

impl<C: Crypto> Engine<'_, C> {
    /// Position of `market_id` in `config.markets`.
    pub fn market_idx(&self, market_id: u16) -> Option<usize> {
        self.st
            .config
            .markets
            .iter()
            .position(|m| m.market_id == market_id)
    }

    /// `oracle_price > 0` and `oracle_age ≤ oracle_max_staleness_ms` (spec §11.3.1 check 4).
    pub fn fresh(&self, m: usize) -> bool {
        let mk = &self.st.markets[m];
        mk.oracle_price > 0
            && self.now.saturating_sub(mk.oracle_time_ms) <= self.st.config.oracle_max_staleness_ms
    }

    /// Verifies a signature; failure is fatal for `entry` (spec §8.3).
    pub fn verify(&self, entry: u32, key: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> Res<()> {
        self.c
            .check_ed25519(key, msg, sig)
            .map_err(|_| Fatal::entry(fatal::BAD_SIGNATURE, entry))
    }

    /// Pushes a pending withdrawal; a full queue is fatal (spec §11.4).
    pub fn push_pending(&mut self, entry: u32, key: [u8; 32], amount: i128) -> Res<()> {
        if self.st.pending.len() >= self.st.config.max_pending_withdrawals as usize {
            return Err(Fatal::entry(fatal::PENDING_QUEUE_OVERFLOW, entry));
        }
        self.st.pending.push(PendingWithdrawalV1 { key, amount });
        Ok(())
    }

    /// `Σ pending.amount`.
    pub fn pending_total(&self, entry: u32) -> Res<i128> {
        self.st
            .pending
            .iter()
            .try_fold(0i128, |acc, p| acc.checked_add(p.amount))
            .or_fatal(entry)
    }

    /// `deposits_credited_total − withdrawals_committed_total − Σ pending`.
    pub fn liquidity_left(&self, entry: u32) -> Res<i128> {
        self.st
            .deposits_credited_total
            .checked_sub(self.st.withdrawals_committed_total)
            .and_then(|x| x.checked_sub(self.pending_total(entry).ok()?))
            .or_fatal(entry)
    }

    /// `fill(account, Δ, p)` (spec §11.3.5). `Δ` is signed: + buy, − sell.
    pub fn fill(&mut self, entry: u32, a: usize, m: usize, delta: i64, price: i64) -> Res<()> {
        let account = self.st.accounts.get_mut(a).or_fatal(entry)?;
        let pos = &mut account.positions[m];
        let s_old = pos.lots;
        let d = i128::from(delta);
        let p = i128::from(price);
        if s_old == 0 || (delta > 0) == (s_old > 0) {
            pos.cost_basis = pos
                .cost_basis
                .checked_add(d.checked_mul(p).or_fatal(entry)?)
                .or_fatal(entry)?;
            pos.lots = s_old.checked_add(delta).or_fatal(entry)?;
        } else {
            let s = i128::from(s_old);
            let q = d.signum() * d.abs().min(s.abs());
            let c_part = caravel_types::fixed::mul_div_trunc(pos.cost_basis, q.abs(), s.abs())
                .or_fatal(entry)?;
            let realized = q
                .checked_mul(p)
                .and_then(|x| x.checked_neg())
                .and_then(|x| x.checked_sub(c_part))
                .or_fatal(entry)?;
            account.collateral = account.collateral.checked_add(realized).or_fatal(entry)?;
            let pos = &mut account.positions[m];
            pos.cost_basis = pos.cost_basis.checked_sub(c_part).or_fatal(entry)?;
            let q64 = i64::try_from(q).or_fatal(entry)?;
            pos.lots = s_old.checked_add(q64).or_fatal(entry)?;
            let rest = delta.checked_sub(q64).or_fatal(entry)?;
            if rest != 0 {
                pos.cost_basis = pos
                    .cost_basis
                    .checked_add(i128::from(rest).checked_mul(p).or_fatal(entry)?)
                    .or_fatal(entry)?;
                pos.lots = pos.lots.checked_add(rest).or_fatal(entry)?;
            }
        }
        let s_new = self.st.accounts[a].positions[m].lots;
        let oi = &mut self.st.markets[m].open_interest_lots;
        *oi = oi
            .checked_add(s_new.max(0))
            .and_then(|x| x.checked_sub(s_old.max(0)))
            .or_fatal(entry)?;
        Ok(())
    }

    /// Moves `amount` of collateral from account `from` to account `to`.
    pub fn transfer(&mut self, entry: u32, from: usize, to: usize, amount: i128) -> Res<()> {
        let f = &mut self.st.accounts.get_mut(from).or_fatal(entry)?.collateral;
        *f = f.checked_sub(amount).or_fatal(entry)?;
        let t = &mut self.st.accounts.get_mut(to).or_fatal(entry)?.collateral;
        *t = t.checked_add(amount).or_fatal(entry)?;
        Ok(())
    }

    /// `charge_fee(account, q, p, bps)` (spec §11.3.5): the fee is split
    /// between the backstop (`insurance_fee_share_bps`) and the treasury.
    pub fn charge_fee(&mut self, entry: u32, a: usize, q: i64, price: i64, bps: u16) -> Res<()> {
        let notional = i128::from(q)
            .checked_mul(i128::from(price))
            .or_fatal(entry)?;
        let fee = mul_div_ceil(notional, i128::from(bps), 10_000).or_fatal(entry)?;
        if fee == 0 {
            return Ok(());
        }
        let ins = mul_div_floor(
            fee,
            i128::from(self.st.config.insurance_fee_share_bps),
            10_000,
        )
        .or_fatal(entry)?;
        self.transfer(entry, a, BACKSTOP_INDEX as usize, ins)?;
        self.transfer(
            entry,
            a,
            TREASURY_INDEX as usize,
            fee.checked_sub(ins).or_fatal(entry)?,
        )
    }

    /// Cancels every order of account `a` on market `m`, bids then asks, in book order.
    pub fn cancel_account_orders(
        &mut self,
        entry: u32,
        m: usize,
        a: usize,
        reason: CancelReason,
        events: &mut Vec<Event>,
    ) -> Res<()> {
        let account_index = u32::try_from(a).or_fatal(entry)?;
        let market_id = self.st.config.markets[m].market_id;
        let mut removed = 0u16;
        let market = &mut self.st.markets[m];
        for book in [&mut market.bids, &mut market.asks] {
            book.retain(|o: &OrderV1| {
                if o.account_index == account_index {
                    events.push(Event::OrderCanceled {
                        market: market_id,
                        order_id: o.order_id,
                        reason,
                    });
                    removed += 1;
                    false
                } else {
                    true
                }
            });
        }
        let count = &mut self.st.accounts[a].open_order_count;
        *count = count.checked_sub(removed).or_fatal(entry)?;
        Ok(())
    }

    /// Cancels every order of account `a` on every market.
    pub fn cancel_all_orders(
        &mut self,
        entry: u32,
        a: usize,
        reason: CancelReason,
        events: &mut Vec<Event>,
    ) -> Res<()> {
        for m in 0..self.st.markets.len() {
            self.cancel_account_orders(entry, m, a, reason, events)?;
        }
        Ok(())
    }
}
