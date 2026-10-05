//! Block building and the checkpoint policy (spec §14.1, §14.2), for any app.
//! Pure: the same inputs always give the same block, so it is tested without a
//! node.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use caravel_core::block::{BlockInputV1, Entry, BLOCK_HEADER_LEN};
use caravel_core::inbox::{InboxKind, InboxMsgV1};
use caravel_core::state::StateFrameV1;
use caravel_core::tx::kind;

use crate::app::{FeedUpdate, LaneApp};
use crate::checkpoint::{BatchBudget, BatchContent, CheckpointTiming, EndReason};
use crate::mempool::Mempool;

/// Where a block entry came from, so a fatal entry can be quarantined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Inbox(u64),
    /// A feed update, by slot.
    Feed(u32),
    User([u8; 32]),
}

pub struct BuildInput<'a, A: LaneApp> {
    pub app: &'a A,
    pub state: &'a A::State,
    pub frame: &'a StateFrameV1,
    pub prev_block_hash: [u8; 32],
    pub now_ms: u64,
    /// Inbox messages from `frame.inbox_through` on, contiguous.
    pub inbox: &'a [InboxMsgV1],
    /// Latest update per feed slot.
    pub feeds: &'a BTreeMap<u32, FeedUpdate>,
    pub mempool: &'a Mempool,
    /// `min(max_block_bytes, space left in the batch)`, lowered under back-pressure.
    pub byte_budget: usize,
    /// At most this many USER entries (halved after a budget exhaustion).
    pub user_cap: usize,
    /// False after an inbox mismatch: the lane stops taking inbox messages.
    pub include_inbox: bool,
}

#[derive(Clone, Debug)]
pub struct Built {
    pub block: BlockInputV1,
    /// `sources[i]` is where entry `i` came from.
    pub sources: Vec<Source>,
    /// Transactions in the block.
    pub included: HashSet<[u8; 32]>,
    /// Transactions that can never be included (expired or nonce already used).
    pub dropped: HashSet<[u8; 32]>,
    pub encoded_len: usize,
}

impl Built {
    /// What this block holds that someone waits for on Stellar (K-01).
    pub fn content(&self) -> BatchContent {
        let mut c = BatchContent::default();
        for e in &self.block.entries {
            match e {
                Entry::Inbox(_) => c.inbox = true,
                Entry::User(tx) => {
                    c.users = true;
                    c.withdrawals |= tx.kind == kind::WITHDRAW;
                }
                Entry::Feed(_) => {}
            }
        }
        c
    }

    pub fn inbox_entries(&self) -> usize {
        self.sources
            .iter()
            .filter(|s| matches!(s, Source::Inbox(_)))
            .count()
    }
}

/// Builds the next block (without `CHECKPOINT_END`; see [`checkpoint_end`]).
pub fn build<A: LaneApp>(input: &BuildInput<'_, A>) -> Built {
    let app = input.app;
    let st = input.state;
    let frame = input.frame;
    let limits = app.limits(st);
    let timestamp_ms = input.now_ms.max(frame.last_timestamp_ms);
    let max_entries = limits.max_entries_per_block as usize;
    let byte_budget = input.byte_budget.min(limits.max_block_bytes as usize);
    let mut bytes = BLOCK_HEADER_LEN;
    let mut entries: Vec<Entry> = Vec::new();
    let mut sources = Vec::new();
    // Every INBOX entry and every WITHDRAW can push one pending withdrawal (§14.2 c).
    let mut pending = app.pending_withdrawals(st);
    let max_pending = limits.max_pending_withdrawals as usize;
    let fits = |bytes: usize, e: &Entry| bytes + e.framed_len() <= byte_budget;

    // 1. Inbox, in index order.
    let mut new_accounts = BTreeSet::new();
    if input.include_inbox {
        let mut expected = frame.inbox_through;
        for msg in input.inbox {
            if msg.index != expected || entries.len() >= max_entries || pending >= max_pending {
                break;
            }
            let e = Entry::Inbox(*msg);
            if !fits(bytes, &e) {
                break;
            }
            bytes += e.framed_len();
            entries.push(e);
            sources.push(Source::Inbox(msg.index));
            pending += 1;
            expected += 1;
            if msg.kind == InboxKind::Deposit {
                new_accounts.insert(msg.lane_account);
            }
        }
    }

    // 2. Feeds: the latest update per slot, if the app takes it now.
    for (slot, u) in input.feeds {
        if !app.feed_include(st, u, timestamp_ms) {
            continue;
        }
        let e = Entry::Feed(u.bytes.clone());
        if entries.len() >= max_entries || !fits(bytes, &e) {
            break;
        }
        bytes += e.framed_len();
        entries.push(e);
        sources.push(Source::Feed(*slot));
    }

    // 3. User transactions, FIFO.
    let mut next_nonce = app.next_nonces(st);
    // An account a deposit in this block creates starts at the app's new
    // account nonce (M0: `block.timestamp_ms`, spec §11.4).
    for key in new_accounts {
        next_nonce
            .entry(key)
            .or_insert(app.new_account_nonce(timestamp_ms));
    }
    let mut per_account: BTreeMap<[u8; 32], u16> = BTreeMap::new();
    let mut included = HashSet::new();
    let mut dropped = HashSet::new();
    let mut users = 0usize;
    for q in input.mempool.iter() {
        let tx = &q.tx;
        if timestamp_ms > tx.expiry_ms {
            dropped.insert(q.hash);
            continue;
        }
        let Some(next) = next_nonce.get(&tx.account).copied() else {
            continue; // no account yet; a deposit may still create it
        };
        if tx.nonce < next {
            dropped.insert(q.hash);
            continue;
        }
        if tx.nonce > next || users >= input.user_cap || entries.len() >= max_entries {
            continue;
        }
        let count = per_account.get(&tx.account).copied().unwrap_or(0);
        if count >= limits.max_txs_per_account_per_block {
            continue;
        }
        let is_withdraw = tx.kind == kind::WITHDRAW;
        if is_withdraw && pending >= max_pending {
            continue;
        }
        let e = Entry::User(tx.clone());
        if !fits(bytes, &e) {
            continue;
        }
        bytes += e.framed_len();
        entries.push(e);
        sources.push(Source::User(q.hash));
        included.insert(q.hash);
        next_nonce.insert(tx.account, next + 1);
        per_account.insert(tx.account, count + 1);
        users += 1;
        if is_withdraw {
            pending += 1;
        }
    }

    let block = BlockInputV1 {
        lane_id: frame.lane_id,
        height: frame.height + 1,
        timestamp_ms,
        prev_block_hash: input.prev_block_hash,
        checkpoint_end: false,
        entries,
    };
    debug_assert_eq!(block.encoded_len(), bytes);
    Built {
        block,
        sources,
        included,
        dropped,
        encoded_len: bytes,
    }
}

/// The block being built, as `checkpoint_end` sees it.
pub struct EndInput<'a> {
    pub budget: &'a BatchBudget,
    pub block_len: usize,
    pub block: BatchContent,
    pub timestamp_ms: u64,
    pub checkpoint_every_blocks: u32,
    pub timing: Option<&'a CheckpointTiming>,
    pub pending_at_start: usize,
    pub max_pending: usize,
}

/// Whether the block being built ends the batch, and why (spec §14.2 a, b,
/// c; K-01's time rules when the node sets them). Sequencer policy only:
/// validators and the engine accept a batch of any length.
pub fn checkpoint_end(i: &EndInput) -> Option<EndReason> {
    if i.budget.must_end_after(i.block_len) {
        return Some(EndReason::Full);
    }
    if i.pending_at_start >= i.max_pending / 2 {
        return Some(EndReason::Withdrawals);
    }
    if i.budget.blocks() + 1 >= i.checkpoint_every_blocks {
        return Some(EndReason::Blocks);
    }
    let t = i.timing?;
    let held = i.budget.content().or(i.block);
    // A batch opens with its first block after genesis: this one, if none.
    let age = match i.budget.opened_ms() {
        Some(_) => i.budget.age_ms(i.timestamp_ms),
        None => 0,
    };
    let waiting_on_stellar = held.inbox || held.withdrawals || i.pending_at_start > 0;
    if waiting_on_stellar && age >= t.urgent_ms {
        Some(EndReason::Urgent)
    } else if held.users && age >= t.busy_ms {
        Some(EndReason::Busy)
    } else if age >= t.idle_ms {
        Some(EndReason::Idle)
    } else {
        None
    }
}
