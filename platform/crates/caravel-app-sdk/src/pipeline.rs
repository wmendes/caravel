//! `genesis` and `step` for any SDK app (spec §20.4.3).
//!
//! The standard paths repeat the frozen perps engine's rules and check order
//! (spec §11.2–§11.4, §11.8): the same codes, the same effects, the same
//! fatal cases. `lanes/perps/node/tests/sdk_conformance.rs` compares the two.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use caravel_core::block::{block_hash_preimage, entry_type, BlockDecodeError, BlockInputV1, Entry};
use caravel_core::codes::{fatal, receipt as codes};
use caravel_core::inbox::{inbox_acc_preimage, InboxKind, InboxMsgV1};
use caravel_core::preimage::{account_leaf_preimage, withdrawal_leaf_preimage};
use caravel_core::receipts::{
    DepositOutcome, EventV1, PlatformEvent, ReceiptV1, ReceiptsV1, PSEUDO_BLOCK_END,
    PSEUDO_BLOCK_START,
};
use caravel_core::state::CommitmentV1;
use caravel_core::tx::{
    kind, sep53_preimage, sep53_tx_message, SigScheme, StandardBody, TxEnvelopeV1,
};

use crate::app::{AppCtx, AppEngine};
use crate::crypto::{Crypto, Hasher};
use crate::genesis::{AccessMode, AppGenesisV1};
use crate::state::{AppAccountV1, PendingV1, SdkState, SessionKeyV1, FLAG_SYSTEM};
use crate::{Fatal, OrFatal, Res, StepOutput};

/// Longest session key lifetime (spec §11.3.3).
const MAX_SESSION_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// Builds the genesis state from `AppGenesisV1` bytes (spec §20.4.1).
pub fn genesis<E: AppEngine>(config_bytes: &[u8], c: &impl Crypto) -> Res<Vec<u8>> {
    let bad = Fatal::block(fatal::BAD_CONFIG);
    let config = AppGenesisV1::decode(config_bytes).map_err(|_| bad)?;
    config.validate().map_err(|_| bad)?;
    if config.template != E::TEMPLATE || !E::TEMPLATE_VERSIONS.contains(&config.template_version) {
        return Err(bad);
    }
    let params = E::params(&config.app_params).ok_or(bad)?;
    let accounts = config
        .system_keys
        .iter()
        .map(|key| AppAccountV1 {
            key: *key,
            flags: FLAG_SYSTEM,
            next_nonce: 0,
            balance: 0,
            session_keys: Vec::new(),
            txs_this_block: 0,
            ext: E::new_account_ext(&params),
        })
        .collect();
    let st = SdkState {
        magic: E::STATE_MAGIC,
        lane_id: config.lane_id,
        config_hash: c.sha256(config_bytes),
        height: 0,
        last_block_input_hash: [0; 32],
        last_timestamp_ms: 0,
        checkpoint_seq: 0,
        inbox_through: 0,
        inbox_acc: [0; 32],
        app_word: 0,
        deposits_credited_total: 0,
        withdrawals_committed_total: 0,
        app_flags: 0,
        app_globals: E::genesis_globals(&params),
        accounts,
        pending: Vec::new(),
        last_commitment: CommitmentV1::default(),
        config,
    };
    st.encode().map_err(|_| bad)
}

/// Executes one `BlockInputV1` against an SDK state (spec §20.4.3).
pub fn step<E: AppEngine, C: Crypto>(
    state_bytes: &[u8],
    block_bytes: &[u8],
    c: &C,
) -> Res<StepOutput> {
    // 1. Decode the state and the block, strictly.
    let bad_state = Fatal::block(fatal::BAD_STATE_ENCODING);
    let st = SdkState::decode(state_bytes, &E::STATE_MAGIC).map_err(|_| bad_state)?;
    let params = E::params(&st.config.app_params).ok_or(bad_state)?;
    let block = BlockInputV1::decode(block_bytes).map_err(|e| match e {
        BlockDecodeError::Header(_) => Fatal::block(fatal::BAD_BLOCK_ENCODING),
        BlockDecodeError::Entry { index, .. } => Fatal::entry(fatal::BAD_ENTRY_ENCODING, index),
    })?;
    check_bodies::<E>(&block)?;
    let mut index = BTreeMap::new();
    for (i, a) in st.accounts.iter().enumerate() {
        let i = u32::try_from(i).map_err(|_| bad_state)?;
        if index.insert(a.key, i).is_some() {
            return Err(bad_state);
        }
    }

    // 2. Block-level checks.
    check_block(&st, &block, state_bytes, block_bytes, c)?;

    let mut run = Run::<C, E> {
        c,
        st,
        params,
        index,
        now: block.timestamp_ms,
        receipts: Vec::new(),
        end_events: Vec::new(),
    };
    // Reset the per-block counters.
    for a in &mut run.st.accounts {
        a.txs_this_block = 0;
    }
    // The app's block start.
    let mut start = Vec::new();
    E::begin_block(&mut run.ctx(fatal::BLOCK_LEVEL, &mut start))?;
    if !start.is_empty() {
        run.receipts.push(ReceiptV1 {
            entry_index: PSEUDO_BLOCK_START,
            code: codes::OK,
            events: start,
        });
    }
    // INBOX, then USER entries (the order and the absence of feeds are checked above).
    for (i, entry) in block.entries.iter().enumerate() {
        let idx = u32::try_from(i).map_err(|_| Fatal::block(fatal::TOO_MANY_ENTRIES))?;
        match entry {
            Entry::Inbox(m) => run.inbox(idx, m)?,
            Entry::Feed(_) => return Err(Fatal::entry(fatal::UNKNOWN_FEED_KEY, idx)),
            Entry::User(tx) => run.user(idx, tx)?,
        }
    }
    // The app's block end, then the commitment.
    let mut end = Vec::new();
    E::end_block(&mut run.ctx(fatal::BLOCK_LEVEL, &mut end))?;
    run.end_events.extend(end);
    if block.checkpoint_end {
        run.commitment(block.height)?;
    }
    if !run.end_events.is_empty() {
        let events = core::mem::take(&mut run.end_events);
        run.receipts.push(ReceiptV1 {
            entry_index: PSEUDO_BLOCK_END,
            code: codes::OK,
            events,
        });
    }
    // Advance and encode.
    run.st.height = block.height;
    run.st.last_timestamp_ms = block.timestamp_ms;
    run.st.last_block_input_hash = c.sha256(block_bytes);
    let overflow = Fatal::block(fatal::ARITHMETIC_OVERFLOW);
    let state = run.st.encode().map_err(|_| overflow)?;
    let receipts = ReceiptsV1 {
        receipts: run.receipts,
    }
    .encode()
    .map_err(|_| overflow)?;
    Ok(StepOutput { state, receipts })
}

/// Every user entry's kind exists and its body has exactly the right length,
/// as M0's strict decode required. Kinds 7 to 15 are reserved.
fn check_bodies<E: AppEngine>(block: &BlockInputV1) -> Res<()> {
    for (i, entry) in block.entries.iter().enumerate() {
        let Entry::User(tx) = entry else { continue };
        let ok = if kind::is_platform(tx.kind) {
            matches!(tx.standard_body(), Some(Ok(_)))
        } else {
            E::body_len(tx.kind) == Some(tx.body.len())
        };
        if !ok {
            return Err(Fatal::entry(fatal::BAD_ENTRY_ENCODING, i as u32));
        }
    }
    Ok(())
}

fn check_block(
    st: &SdkState,
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
    let mut last = entry_type::INBOX;
    for (i, entry) in block.entries.iter().enumerate() {
        let t = entry.entry_type();
        if t < last {
            return Err(Fatal::entry(fatal::ENTRY_ORDER, i as u32));
        }
        last = t;
    }
    Ok(())
}

/// One block's execution.
struct Run<'a, C: Crypto, E: AppEngine> {
    c: &'a C,
    st: SdkState,
    params: E::Params,
    index: BTreeMap<[u8; 32], u32>,
    now: u64,
    receipts: Vec<ReceiptV1>,
    end_events: Vec<EventV1>,
}

impl<C: Crypto, E: AppEngine> Run<'_, C, E> {
    fn ctx<'b>(&'b mut self, entry: u32, events: &'b mut Vec<EventV1>) -> AppCtx<'b, E::Params> {
        AppCtx {
            st: &mut self.st,
            params: &self.params,
            index: &self.index,
            now: self.now,
            entry,
            events,
        }
    }

    fn verify(&self, entry: u32, key: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> Res<()> {
        self.c
            .check_ed25519(key, msg, sig)
            .map_err(|_| Fatal::entry(fatal::BAD_SIGNATURE, entry))
    }

    /// A full queue is fatal (spec §11.4); the sequencer prevents it.
    fn push_pending(&mut self, entry: u32, key: [u8; 32], amount: i128) -> Res<()> {
        if self.st.pending.len() >= self.st.config.max_pending_withdrawals as usize {
            return Err(Fatal::entry(fatal::PENDING_QUEUE_OVERFLOW, entry));
        }
        self.st.pending.push(PendingV1 { key, amount });
        Ok(())
    }

    /// `deposits_credited_total − withdrawals_committed_total − Σ pending`.
    fn liquidity_left(&self, entry: u32) -> Res<i128> {
        let pending = self
            .st
            .pending
            .iter()
            .try_fold(0i128, |acc, p| acc.checked_add(p.amount))
            .or_fatal(entry)?;
        self.st
            .deposits_credited_total
            .checked_sub(self.st.withdrawals_committed_total)
            .and_then(|x| x.checked_sub(pending))
            .or_fatal(entry)
    }

    // --- INBOX (spec §11.4) --------------------------------------------------

    fn inbox(&mut self, entry: u32, msg: &InboxMsgV1) -> Res<()> {
        if msg.index != self.st.inbox_through {
            return Err(Fatal::entry(fatal::INBOX_GAP, entry));
        }
        self.st.inbox_acc = self
            .c
            .sha256(&inbox_acc_preimage(&self.st.inbox_acc, &msg.encode()));
        self.st.inbox_through = self.st.inbox_through.checked_add(1).or_fatal(entry)?;
        let mut events = Vec::new();
        match msg.kind {
            InboxKind::Deposit => self.deposit(entry, msg, &mut events)?,
            InboxKind::ForcedWithdrawal => self.forced_withdrawal(entry, msg, &mut events)?,
        }
        self.receipts.push(ReceiptV1 {
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

    /// `Some(None)` for a new index, `Some(Some(a))` to reuse empty account `a`.
    fn free_slot(&self) -> Option<Option<usize>> {
        if self.st.accounts.len() < self.st.config.max_accounts as usize {
            return Some(None);
        }
        (0..self.st.accounts.len())
            .find(|&a| {
                let acct = &self.st.accounts[a];
                !acct.is_system()
                    && E::is_empty(&self.st, &self.params, a)
                    && !self.st.pending.iter().any(|p| p.key == acct.key)
            })
            .map(Some)
    }

    fn deposit(&mut self, entry: u32, msg: &InboxMsgV1, events: &mut Vec<EventV1>) -> Res<()> {
        let key = msg.lane_account;
        self.st.deposits_credited_total = self
            .st
            .deposits_credited_total
            .checked_add(msg.amount)
            .or_fatal(entry)?;
        let outcome = if let Some(&a) = self.index.get(&key) {
            let b = &mut self.st.accounts[a as usize].balance;
            *b = b.checked_add(msg.amount).or_fatal(entry)?;
            DepositOutcome::Credited
        } else if let Some(slot) = self.free_slot().filter(|_| self.access_allows(&key)) {
            // A new account's nonce starts at the block time, so transactions
            // signed for an evicted holder of the same key never replay.
            let account = AppAccountV1 {
                key,
                flags: 0,
                next_nonce: self.now,
                balance: msg.amount,
                session_keys: Vec::new(),
                txs_this_block: 0,
                ext: E::new_account_ext(&self.params),
            };
            let a = match slot {
                None => {
                    self.st.accounts.push(account);
                    self.st.accounts.len() - 1
                }
                Some(a) => {
                    self.index.remove(&self.st.accounts[a].key);
                    self.st.accounts[a] = account;
                    a
                }
            };
            self.index.insert(key, u32::try_from(a).or_fatal(entry)?);
            DepositOutcome::Created
        } else {
            self.push_pending(entry, key, msg.amount)?;
            DepositOutcome::Bounced
        };
        events.push(
            PlatformEvent::Deposit {
                key,
                amount: msg.amount,
                outcome,
            }
            .to_event(),
        );
        Ok(())
    }

    fn forced_withdrawal(
        &mut self,
        entry: u32,
        msg: &InboxMsgV1,
        events: &mut Vec<EventV1>,
    ) -> Res<()> {
        let key = msg.lane_account;
        let Some(&a) = self.index.get(&key) else {
            return Ok(()); // no account: processed as a no-op
        };
        let a = a as usize;
        E::before_forced_withdrawal(&mut self.ctx(entry, events), a)?;
        let free = E::free_balance(&self.st, &self.params, a)?;
        let amt = msg
            .amount
            .min(self.st.accounts[a].balance)
            .min(free.max(0))
            .min(self.liquidity_left(entry)?);
        let queued = if amt >= 1 {
            self.push_pending(entry, key, amt)?;
            let b = &mut self.st.accounts[a].balance;
            *b = b.checked_sub(amt).or_fatal(entry)?;
            amt
        } else {
            0
        };
        events.push(
            PlatformEvent::ForcedWithdrawalProcessed {
                key,
                amount: queued,
            }
            .to_event(),
        );
        Ok(())
    }

    // --- USER (spec §11.3) ---------------------------------------------------

    fn user(&mut self, entry: u32, tx: &TxEnvelopeV1) -> Res<()> {
        let mut events = Vec::new();
        let code = self.user_tx(entry, tx, &mut events)?;
        if code != codes::OK {
            events.clear();
        }
        self.receipts.push(ReceiptV1 {
            entry_index: entry,
            code,
            events,
        });
        Ok(())
    }

    fn user_tx(&mut self, entry: u32, tx: &TxEnvelopeV1, events: &mut Vec<EventV1>) -> Res<u16> {
        if tx.lane_id != self.st.lane_id {
            return Ok(codes::WRONG_LANE);
        }
        let Some(&a) = self.index.get(&tx.account) else {
            return Ok(codes::UNKNOWN_ACCOUNT);
        };
        let a = a as usize;
        if tx.signer != tx.account && !self.session_key_allows(a, tx) {
            return Ok(codes::UNAUTHORIZED_SIGNER);
        }
        // The signature: failure is fatal (the sequencer pre-verifies).
        let tx_hash = self.c.sha256(&tx.tx_hash_preimage(&self.st.config_hash));
        match tx.sig_scheme {
            SigScheme::RawEd25519 => self.verify(entry, &tx.signer, &tx_hash, &tx.signature)?,
            SigScheme::Sep53 => {
                let msg = self.c.sha256(&sep53_preimage(&sep53_tx_message(&tx_hash)));
                self.verify(entry, &tx.signer, &msg, &tx.signature)?;
            }
        }
        let account = &self.st.accounts[a];
        if self.now > tx.expiry_ms {
            return Ok(codes::EXPIRED);
        }
        if tx.nonce != account.next_nonce {
            return Ok(codes::BAD_NONCE);
        }
        if account.txs_this_block >= self.st.config.max_txs_per_account_per_block {
            return Ok(codes::RATE_LIMITED);
        }
        // Consume the nonce. From here on a rejection still consumes it.
        let account = &mut self.st.accounts[a];
        account.next_nonce = account.next_nonce.checked_add(1).or_fatal(entry)?;
        account.txs_this_block = account.txs_this_block.checked_add(1).or_fatal(entry)?;
        match tx.standard_body() {
            Some(Ok(StandardBody::Withdraw { amount })) => self.withdraw(entry, a, amount),
            Some(Ok(StandardBody::AddSessionKey {
                session_key,
                expires_at_ms,
                permissions,
            })) => {
                Ok(self.add_session_key(a, &tx.account, session_key, expires_at_ms, permissions))
            }
            Some(Ok(StandardBody::RevokeSessionKey { session_key })) => {
                Ok(self.revoke_session_key(a, &session_key))
            }
            Some(Err(_)) => Err(Fatal::entry(fatal::BAD_ENTRY_ENCODING, entry)),
            None => E::apply(&mut self.ctx(entry, events), a, tx.kind, &tx.body),
        }
    }

    /// A session key may sign if the kind allows session keys, it is registered
    /// and unexpired, has the kind's permission, and the scheme is raw ed25519.
    fn session_key_allows(&self, a: usize, tx: &TxEnvelopeV1) -> bool {
        if kind::is_platform(tx.kind) || tx.sig_scheme == SigScheme::Sep53 {
            return false;
        }
        let Some(perm) = E::session_permission(tx.kind) else {
            return false;
        };
        let keys = &self.st.accounts[a].session_keys;
        keys.binary_search_by(|k| k.key.cmp(&tx.signer))
            .ok()
            .map(|i| &keys[i])
            .is_some_and(|k| k.expires_at_ms > self.now && k.permissions & perm != 0)
    }

    /// WITHDRAW (spec §11.3.2), with the balance and the app's free balance.
    fn withdraw(&mut self, entry: u32, a: usize, amount: i128) -> Res<u16> {
        if amount < self.st.config.min_withdrawal {
            return Ok(codes::BELOW_MIN_WITHDRAWAL);
        }
        if self.st.pending.len() >= self.st.config.max_pending_withdrawals as usize {
            return Ok(codes::WITHDRAWAL_QUEUE_FULL);
        }
        if amount > self.st.accounts[a].balance {
            return Ok(codes::INSUFFICIENT_FREE_BALANCE);
        }
        if amount > E::free_balance(&self.st, &self.params, a)? {
            return Ok(codes::INSUFFICIENT_FREE_BALANCE);
        }
        if amount > self.liquidity_left(entry)? {
            return Ok(codes::INSUFFICIENT_LANE_LIQUIDITY);
        }
        let key = self.st.accounts[a].key;
        let b = &mut self.st.accounts[a].balance;
        *b = b.checked_sub(amount).or_fatal(entry)?;
        self.push_pending(entry, key, amount)?;
        Ok(codes::OK)
    }

    /// ADD_SESSION_KEY (spec §11.3.3), with the app's permission bits.
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
        if permissions & !E::PERMISSIONS != 0 {
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

    // --- Commitment (spec §11.8) --------------------------------------------

    fn commitment(&mut self, block_height: u64) -> Res<()> {
        let e = fatal::BLOCK_LEVEL;
        let seq = self.st.checkpoint_seq.checked_add(1).or_fatal(e)?;
        let lane_id = self.st.lane_id;
        let mut account_leaves = Vec::with_capacity(self.st.accounts.len());
        let mut escape_total: i128 = 0;
        for j in 0..self.st.accounts.len() {
            let escape_equity = E::escape_equity(&self.st, &self.params, j)?.max(0);
            escape_total = escape_total.checked_add(escape_equity).or_fatal(e)?;
            let key = self.st.accounts[j].key;
            let j = u32::try_from(j).or_fatal(e)?;
            account_leaves.push(self.c.sha256(&account_leaf_preimage(
                &lane_id,
                seq,
                j,
                &key,
                escape_equity,
            )));
        }
        let accounts_root = caravel_core::merkle::root(&Hasher(self.c), &account_leaves)
            .ok()
            .or_fatal(e)?;
        let mut withdrawal_leaves = Vec::with_capacity(self.st.pending.len());
        let mut withdrawals_total: i128 = 0;
        for (i, p) in self.st.pending.iter().enumerate() {
            withdrawals_total = withdrawals_total.checked_add(p.amount).or_fatal(e)?;
            let i = u32::try_from(i).or_fatal(e)?;
            withdrawal_leaves.push(self.c.sha256(&withdrawal_leaf_preimage(
                &lane_id, seq, i, &p.key, p.amount,
            )));
        }
        let withdrawals_root = caravel_core::merkle::root(&Hasher(self.c), &withdrawal_leaves)
            .ok()
            .or_fatal(e)?;
        self.st.last_commitment = CommitmentV1 {
            seq,
            last_block_height: block_height,
            accounts_root,
            account_count: u32::try_from(self.st.accounts.len()).or_fatal(e)?,
            escape_total,
            withdrawals_root,
            withdrawal_count: u32::try_from(self.st.pending.len()).or_fatal(e)?,
            withdrawals_total,
            inbox_through: self.st.inbox_through,
            inbox_acc: self.st.inbox_acc,
        };
        self.st.withdrawals_committed_total = self
            .st
            .withdrawals_committed_total
            .checked_add(withdrawals_total)
            .or_fatal(e)?;
        self.st.pending.clear();
        self.st.checkpoint_seq = seq;
        self.end_events.push(
            PlatformEvent::Commitment {
                seq,
                withdrawals_total,
                escape_total,
            }
            .to_event(),
        );
        Ok(())
    }
}
