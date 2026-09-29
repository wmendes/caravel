//! INBOX entries: deposits and forced withdrawals from Stellar (spec §11.4).

use alloc::vec;
use alloc::vec::Vec;

use caravel_types::codes;
use caravel_types::config::AccessMode;
use caravel_types::fatal::{self, Fatal};
use caravel_types::inbox::{inbox_acc_preimage, InboxKind, InboxMsgV1};
use caravel_types::receipts::{CancelReason, DepositOutcome, Event, Receipt};
use caravel_types::state::{AccountV1, PositionV1};

use crate::engine::{Engine, OrFatal, Res};
use crate::margin::account_margin;
use crate::Crypto;

/// Where a new account goes.
enum Slot {
    /// Append at `account_count`.
    New,
    /// Overwrite this empty non-system account in place.
    Reuse(usize),
}

impl<C: Crypto> Engine<'_, C> {
    pub(crate) fn inbox(&mut self, entry: u32, msg: &InboxMsgV1) -> Res<()> {
        // 1. Sequential index.
        if msg.index != self.st.inbox_through {
            return Err(Fatal::entry(fatal::INBOX_GAP, entry));
        }
        // 2. Accumulator.
        self.st.inbox_acc = self
            .c
            .sha256(&inbox_acc_preimage(&self.st.inbox_acc, &msg.encode()));
        self.st.inbox_through = self.st.inbox_through.checked_add(1).or_fatal(entry)?;

        let mut events = Vec::new();
        match msg.kind {
            InboxKind::Deposit => self.deposit(entry, msg, &mut events)?,
            InboxKind::ForcedWithdrawal => self.forced_withdrawal(entry, msg, &mut events)?,
        }
        self.receipts.push(Receipt {
            entry_index: entry,
            code: codes::OK,
            events,
        });
        Ok(())
    }

    fn access_allows(&self, key: &[u8; 32]) -> bool {
        match self.st.config.access_mode {
            AccessMode::Open => true,
            AccessMode::Allowlist => self.st.config.allowlist.binary_search(key).is_ok(),
        }
    }

    /// A new index while `account_count < max_accounts`, else the lowest-index
    /// empty non-system account: no collateral, no positions, no open orders and
    /// no pending withdrawal for its key.
    fn free_slot(&self) -> Option<Slot> {
        if self.st.accounts.len() < self.st.config.max_accounts as usize {
            return Some(Slot::New);
        }
        self.st
            .accounts
            .iter()
            .position(|a| {
                !a.system
                    && a.collateral == 0
                    && a.open_order_count == 0
                    && a.positions.iter().all(|p| p.lots == 0 && p.cost_basis == 0)
                    && !self.st.pending.iter().any(|p| p.key == a.key)
            })
            .map(Slot::Reuse)
    }

    fn deposit(&mut self, entry: u32, msg: &InboxMsgV1, events: &mut Vec<Event>) -> Res<()> {
        let key = msg.lane_account;
        self.st.deposits_credited_total = self
            .st
            .deposits_credited_total
            .checked_add(msg.amount)
            .or_fatal(entry)?;
        let outcome = if let Some(&a) = self.index.get(&key) {
            let c = &mut self.st.accounts[a as usize].collateral;
            *c = c.checked_add(msg.amount).or_fatal(entry)?;
            DepositOutcome::Credited
        } else if let Some(slot) = self.free_slot().filter(|_| self.access_allows(&key)) {
            // A new account's nonce starts at the block time, so transactions
            // signed for an evicted holder of the same key never replay (DEC-016).
            let account = AccountV1 {
                key,
                system: false,
                next_nonce: self.now,
                collateral: msg.amount,
                open_order_count: 0,
                session_keys: Vec::new(),
                positions: vec![PositionV1::default(); self.st.markets.len()],
                txs_this_block: 0,
            };
            let a = match slot {
                Slot::New => {
                    self.st.accounts.push(account);
                    self.st.accounts.len() - 1
                }
                Slot::Reuse(a) => {
                    self.index.remove(&self.st.accounts[a].key);
                    self.st.accounts[a] = account;
                    a
                }
            };
            self.index.insert(key, u32::try_from(a).or_fatal(entry)?);
            DepositOutcome::Created
        } else {
            // Refunded through the next withdrawals root.
            self.push_pending(entry, key, msg.amount)?;
            DepositOutcome::Bounced
        };
        events.push(Event::Deposit {
            key,
            amount: msg.amount,
            outcome,
        });
        Ok(())
    }

    fn forced_withdrawal(
        &mut self,
        entry: u32,
        msg: &InboxMsgV1,
        events: &mut Vec<Event>,
    ) -> Res<()> {
        let key = msg.lane_account;
        let Some(&a) = self.index.get(&key) else {
            return Ok(()); // no account: processed as a no-op
        };
        let a = a as usize;
        self.cancel_all_orders(entry, a, CancelReason::ForcedWithdrawal, events)?;
        let free = account_margin(&self.st, a, None)
            .and_then(|m| m.free_collateral())
            .or_fatal(entry)?;
        let amt = msg
            .amount
            .min(self.st.accounts[a].collateral)
            .min(free.max(0))
            .min(self.liquidity_left(entry)?);
        let queued = if amt >= 1 {
            self.push_pending(entry, key, amt)?;
            let c = &mut self.st.accounts[a].collateral;
            *c = c.checked_sub(amt).or_fatal(entry)?;
            amt
        } else {
            0
        };
        // Positions are not force-closed in M0 (T-M1-08).
        events.push(Event::ForcedWithdrawalProcessed {
            key,
            amount: queued,
        });
        Ok(())
    }
}
