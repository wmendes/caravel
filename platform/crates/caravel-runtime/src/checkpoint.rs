//! Batch accounting (spec §14.2), checkpoint assembly (§14.3) and the
//! withdrawal and account leaves a checkpoint commits to. The sequencer,
//! validators and replay all build headers with this code, so they agree byte
//! for byte.

use caravel_core::batch::{BatchV1, BATCH_HEADER_LEN, BATCH_PER_BLOCK_OVERHEAD};
use caravel_core::block::{block_hash_preimage, BlockInputV1, BlockRecordV1, Entry};
use caravel_core::checkpoint::CheckpointHeaderV1;
use caravel_core::inbox::InboxKind;
use caravel_core::merkle::{MerkleTree, NativeSha256};
use caravel_core::preimage::{account_leaf_preimage, withdrawal_leaf_preimage};
use caravel_core::receipts::{DepositOutcome, PlatformEvent, ReceiptsV1};
use caravel_core::state::StateFrameV1;
use caravel_core::tx::StandardBody;
use sha2::{Digest, Sha256};

/// Room kept at the end of a batch (spec §14.2).
const BATCH_SLACK: usize = 36;

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// `H(passphrase)`, as `env.ledger().network_id()` returns it.
pub fn network_id(passphrase: &str) -> [u8; 32] {
    sha256(passphrase.as_bytes())
}

/// `H(ScVal XDR of the contract address)`: the XDR is `SCV_ADDRESS` (18),
/// `SC_ADDRESS_TYPE_CONTRACT` (1) and the 32-byte contract id.
pub fn settlement_addr_hash(contract_id: &[u8; 32]) -> [u8; 32] {
    let mut xdr = [0u8; 40];
    xdr[3] = 0x12;
    xdr[7] = 0x01;
    xdr[8..].copy_from_slice(contract_id);
    sha256(&xdr)
}

/// `block_hash = H(H(input) || state_hash_after)` (spec §9.6).
pub fn block_hash(record: &BlockRecordV1) -> [u8; 32] {
    sha256(&block_hash_preimage(
        &sha256(&record.input),
        &record.state_hash_after,
    ))
}

/// Bytes used by the batch being built (spec §14.2), and (K-01) what it
/// holds and since when it is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchBudget {
    pub max_batch_bytes: usize,
    pub max_block_bytes: usize,
    used: usize,
    blocks: u32,
    content: BatchContent,
    /// The timestamp of the block that sealed the previous checkpoint, or
    /// of the batch's first block after genesis.
    opened_ms: Option<u64>,
}

/// What a block, or the open batch, holds that someone waits for on
/// Stellar (K-01).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BatchContent {
    /// An inbox entry: a deposit or a forced withdrawal.
    pub inbox: bool,
    /// A user transaction.
    pub users: bool,
    /// A WITHDRAW transaction.
    pub withdrawals: bool,
}

impl BatchContent {
    pub fn or(self, o: Self) -> Self {
        Self {
            inbox: self.inbox || o.inbox,
            users: self.users || o.users,
            withdrawals: self.withdrawals || o.withdrawals,
        }
    }
}

/// When a batch ends by time (K-01), all in milliseconds of block time
/// since the batch opened. A node setting, not consensus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckpointTiming {
    /// A deposit, forced withdrawal or withdrawal is waiting.
    pub urgent_ms: u64,
    /// User transactions are waiting.
    pub busy_ms: u64,
    /// Nothing is waiting: the heartbeat.
    pub idle_ms: u64,
}

/// Why a block ended its batch (spec §14.2; K-01).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// (a) `checkpoint_every_blocks`.
    Blocks,
    /// (b) the next block might not fit.
    Full,
    /// (c) the withdrawal queue is half full.
    Withdrawals,
    Urgent,
    Busy,
    Idle,
}

impl EndReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blocks => "blocks",
            Self::Full => "full",
            Self::Withdrawals => "withdrawals",
            Self::Urgent => "urgent",
            Self::Busy => "busy",
            Self::Idle => "idle",
        }
    }
}

impl BatchBudget {
    pub fn new(max_batch_bytes: usize, max_block_bytes: usize) -> Self {
        Self {
            max_batch_bytes,
            max_block_bytes,
            used: BATCH_HEADER_LEN,
            blocks: 0,
            content: BatchContent::default(),
            opened_ms: None,
        }
    }

    pub fn content(&self) -> BatchContent {
        self.content
    }

    pub fn opened_ms(&self) -> Option<u64> {
        self.opened_ms
    }

    /// How long the batch has been open at block time `timestamp_ms`.
    pub fn age_ms(&self, timestamp_ms: u64) -> u64 {
        self.opened_ms.map_or(0, |o| timestamp_ms.saturating_sub(o))
    }

    pub fn used(&self) -> usize {
        self.used
    }

    pub fn blocks(&self) -> u32 {
        self.blocks
    }

    /// `max_batch_bytes − batch_used − 36`.
    pub fn space_left(&self) -> usize {
        self.max_batch_bytes
            .saturating_sub(self.used)
            .saturating_sub(BATCH_SLACK)
    }

    /// The byte budget of the next block.
    pub fn block_budget(&self) -> usize {
        self.max_block_bytes.min(self.space_left())
    }

    /// Condition (b): after a block of `block_len` bytes, the next block might not fit.
    pub fn must_end_after(&self, block_len: usize) -> bool {
        let after = self.used + BATCH_PER_BLOCK_OVERHEAD + block_len;
        self.max_batch_bytes
            .saturating_sub(after)
            .saturating_sub(BATCH_SLACK)
            < self.max_block_bytes
    }

    pub fn add(&mut self, block_len: usize) {
        self.used += BATCH_PER_BLOCK_OVERHEAD + block_len;
        self.blocks += 1;
    }

    /// Records a block's content and time (K-01). The first block after
    /// genesis opens the batch.
    pub fn note(&mut self, content: BatchContent, timestamp_ms: u64) {
        self.content = self.content.or(content);
        self.opened_ms.get_or_insert(timestamp_ms);
    }

    /// Starts the next batch: empty, its age set by `opened_at`.
    pub fn reset(&mut self) {
        self.used = BATCH_HEADER_LEN;
        self.blocks = 0;
        self.content = BatchContent::default();
    }

    /// The previous checkpoint was sealed by the block at `sealed_ms`.
    pub fn opened_at(&mut self, sealed_ms: u64) {
        self.opened_ms = Some(sealed_ms);
    }
}

/// The node-config fields of a header (spec §14.3 step 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeaderIds {
    pub network_id: [u8; 32],
    pub settlement_addr_hash: [u8; 32],
    pub engine_wasm_hash: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssembleError {
    /// The last block is not a `CHECKPOINT_END` block, or the state is not its output.
    NotAtCheckpoint,
    Encode,
}

/// Builds `BatchV1` and `CheckpointHeaderV1` for the blocks since the last
/// checkpoint. `state_bytes` is the state after the last of `records`, which
/// is the `CHECKPOINT_END` block; only its frame is read.
pub fn assemble(
    ids: &HeaderIds,
    prev_header_hash: [u8; 32],
    records: &[BlockRecordV1],
    state_bytes: &[u8],
) -> Result<(Vec<u8>, CheckpointHeaderV1), AssembleError> {
    let state = StateFrameV1::read(state_bytes).map_err(|_| AssembleError::Encode)?;
    let last = records.last().ok_or(AssembleError::NotAtCheckpoint)?;
    let last_input = BlockInputV1::decode(&last.input).map_err(|_| AssembleError::Encode)?;
    let c = state.last_commitment;
    let state_hash = sha256(state_bytes);
    if !last_input.checkpoint_end
        || last_input.height != c.last_block_height
        || state_hash != last.state_hash_after
    {
        return Err(AssembleError::NotAtCheckpoint);
    }
    let first_input = BlockInputV1::decode(&records[0].input).map_err(|_| AssembleError::Encode)?;
    let batch = BatchV1 {
        lane_id: state.lane_id,
        checkpoint_seq: c.seq,
        blocks: records.to_vec(),
    }
    .encode()
    .map_err(|_| AssembleError::Encode)?;
    let header = CheckpointHeaderV1 {
        lane_id: state.lane_id,
        network_id: ids.network_id,
        settlement_addr_hash: ids.settlement_addr_hash,
        engine_wasm_hash: ids.engine_wasm_hash,
        seq: c.seq,
        prev_header_hash,
        first_block_height: first_input.height,
        last_block_height: last_input.height,
        last_block_timestamp_ms: last_input.timestamp_ms,
        last_block_hash: block_hash(last),
        batch_hash: sha256(&batch),
        state_hash,
        accounts_root: c.accounts_root,
        account_count: c.account_count,
        escape_total: c.escape_total,
        withdrawals_root: c.withdrawals_root,
        withdrawal_count: c.withdrawal_count,
        withdrawals_total: c.withdrawals_total,
        inbox_through: c.inbox_through,
        inbox_acc: c.inbox_acc,
    };
    Ok((batch, header))
}

/// A leaf and the data needed to claim it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leaf {
    pub index: u32,
    pub key: [u8; 32],
    /// The withdrawal amount, or the escape equity.
    pub amount: i128,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeafError {
    Decode,
    /// The rebuilt leaves do not give the header's root, count or total.
    Mismatch,
}

/// The withdrawal leaves of a checkpoint, rebuilt from its blocks and
/// receipts: every push to the pending queue since the previous commitment,
/// in execution order (spec §11.4, §11.8). The result is checked against the
/// header, so a wrong rebuild is never served.
pub fn withdrawal_leaves(
    header: &CheckpointHeaderV1,
    blocks: &[(BlockRecordV1, ReceiptsV1)],
) -> Result<Vec<Leaf>, LeafError> {
    let mut pending: Vec<([u8; 32], i128)> = Vec::new();
    for (record, receipts) in blocks {
        let input = BlockInputV1::decode(&record.input).map_err(|_| LeafError::Decode)?;
        for rc in &receipts.receipts {
            let Some(entry) = input.entries.get(rc.entry_index as usize) else {
                continue;
            };
            match entry {
                Entry::Inbox(msg) => {
                    for e in &rc.events {
                        let Some(e) = e.platform() else { continue };
                        match (msg.kind, e.map_err(|_| LeafError::Decode)?) {
                            (
                                InboxKind::Deposit,
                                PlatformEvent::Deposit {
                                    key,
                                    amount,
                                    outcome: DepositOutcome::Bounced,
                                },
                            ) => pending.push((key, amount)),
                            (
                                InboxKind::ForcedWithdrawal,
                                PlatformEvent::ForcedWithdrawalProcessed { key, amount },
                            ) if amount > 0 => pending.push((key, amount)),
                            _ => {}
                        }
                    }
                }
                Entry::User(tx) if !rc.rejected() => {
                    if let Some(Ok(StandardBody::Withdraw { amount })) = tx.standard_body() {
                        pending.push((tx.account, amount));
                    }
                }
                _ => {}
            }
        }
    }
    let leaves: Vec<Leaf> = pending
        .into_iter()
        .enumerate()
        .map(|(i, (key, amount))| Leaf {
            index: i as u32,
            key,
            amount,
        })
        .collect();
    let hashes: Vec<[u8; 32]> = leaves
        .iter()
        .map(|l| {
            sha256(&withdrawal_leaf_preimage(
                &header.lane_id,
                header.seq,
                l.index,
                &l.key,
                l.amount,
            ))
        })
        .collect();
    let root =
        caravel_core::merkle::root(&NativeSha256, &hashes).map_err(|_| LeafError::Mismatch)?;
    let total: i128 = leaves.iter().map(|l| l.amount).sum();
    if root != header.withdrawals_root
        || leaves.len() != header.withdrawal_count as usize
        || total != header.withdrawals_total
    {
        return Err(LeafError::Mismatch);
    }
    Ok(leaves)
}

/// The account leaves of a checkpoint, from the app's `(key, escape equity)`
/// list for the state right after it (`LaneApp::escape_leaves`), checked
/// against the header.
pub fn account_leaves(
    header: &CheckpointHeaderV1,
    escape: &[([u8; 32], i128)],
) -> Result<Vec<Leaf>, LeafError> {
    let leaves: Vec<Leaf> = escape
        .iter()
        .enumerate()
        .map(|(j, (key, equity))| Leaf {
            index: j as u32,
            key: *key,
            amount: *equity,
        })
        .collect();
    let hashes: Vec<[u8; 32]> = leaves
        .iter()
        .map(|l| {
            sha256(&account_leaf_preimage(
                &header.lane_id,
                header.seq,
                l.index,
                &l.key,
                l.amount,
            ))
        })
        .collect();
    let root =
        caravel_core::merkle::root(&NativeSha256, &hashes).map_err(|_| LeafError::Mismatch)?;
    if root != header.accounts_root || leaves.len() != header.account_count as usize {
        return Err(LeafError::Mismatch);
    }
    Ok(leaves)
}

/// The Merkle proof of `index` among `leaves`, hashed with `leaf_hash`.
pub fn proof(leaf_hashes: &[[u8; 32]], index: u32) -> Option<Vec<[u8; 32]>> {
    let tree = MerkleTree::build(&NativeSha256, leaf_hashes).ok()?;
    tree.proof(index).ok()
}

/// Withdrawal leaf hashes for [`proof`].
pub fn withdrawal_hashes(header: &CheckpointHeaderV1, leaves: &[Leaf]) -> Vec<[u8; 32]> {
    leaves
        .iter()
        .map(|l| {
            sha256(&withdrawal_leaf_preimage(
                &header.lane_id,
                header.seq,
                l.index,
                &l.key,
                l.amount,
            ))
        })
        .collect()
}

/// Account leaf hashes for [`proof`].
pub fn account_hashes(header: &CheckpointHeaderV1, leaves: &[Leaf]) -> Vec<[u8; 32]> {
    leaves
        .iter()
        .map(|l| {
            sha256(&account_leaf_preimage(
                &header.lane_id,
                header.seq,
                l.index,
                &l.key,
                l.amount,
            ))
        })
        .collect()
}
