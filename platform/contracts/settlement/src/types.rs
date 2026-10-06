//! Types, storage keys, errors and events (spec §13.1, §13.2).

use soroban_sdk::{contracterror, contractevent, contracttype, Address, BytesN, Vec};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeightedSigner {
    pub key: BytesN<32>,
    pub weight: u32,
}

/// Keys strictly ascending (spec §13.4).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeightedSigners {
    pub signers: Vec<WeightedSigner>,
    pub threshold: u32,
}

/// Indexes strictly ascending.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sig {
    pub signer_index: u32,
    pub signature: BytesN<64>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Params {
    /// M0 testnet default 3600; demo value 600.
    pub force_inclusion_window_secs: u64,
    /// M0 testnet default 21600; demo value 1200.
    pub escape_timeout_secs: u64,
    /// 3600.
    pub min_rotation_delay_secs: u64,
    /// 2.
    pub signer_retention_epochs: u32,
    /// Same as the genesis config.
    pub min_deposit: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub admin: Address,
    pub usdc: Address,
    pub lane_id: BytesN<32>,
    pub engine_wasm_hash: BytesN<32>,
    pub genesis_state_hash: BytesN<32>,
    pub config_hash: BytesN<32>,
    pub params: Params,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LastCheckpoint {
    pub seq: u64,
    pub header_hash: BytesN<32>,
    pub last_block_height: u64,
    pub last_block_hash: BytesN<32>,
    pub last_block_timestamp_ms: u64,
    pub state_hash: BytesN<32>,
    pub accounts_root: BytesN<32>,
    pub account_count: u32,
    pub escape_total: i128,
    pub inbox_through: u64,
    /// Ledger timestamp (seconds).
    pub accepted_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckpointRecord {
    pub header_hash: BytesN<32>,
    pub withdrawals_root: BytesN<32>,
    pub withdrawal_count: u32,
    pub withdrawals_total: i128,
    pub claimed_total: i128,
    pub stellar_ledger: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InboxMsg {
    /// 0 DEPOSIT, 1 FORCED_WITHDRAWAL.
    pub kind: u32,
    pub from: Address,
    pub lane_account: BytesN<32>,
    pub amount: i128,
    pub enqueued_at: u64,
    pub acc_after: BytesN<32>,
    pub cum_deposits_after: i128,
    pub refunded: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrozenInfo {
    pub at: u64,
    pub payout_num: i128,
    pub payout_den: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Config,
    Epoch,
    MinValidEpoch,
    LastCkpt,
    InboxCount,
    Frozen,
    LastRotationAt,
    TotalWithdrawalsCommitted,
    TotalWithdrawalsClaimed,
    Signers(u64),
    SignersEpoch(BytesN<32>),
    Inbox(u64),
    Ckpt(u64),
    Claimed(u64, u32),
    EscapeClaimed(BytesN<32>),
}

/// Every rejection has its own code, so a failing check is identifiable.
#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    // §13.4 signer sets and the constructor.
    BadSignerSet = 1,
    BadParams = 2,
    // Deposits and forced withdrawals.
    Frozen = 10,
    BelowMinDeposit = 11,
    BadAmount = 12,
    NotOwner = 13,
    // §13.3 submit_checkpoint, one per numbered check.
    BadHeaderEncoding = 20,
    WrongLane = 21,
    WrongNetwork = 22,
    WrongSettlement = 23,
    WrongEngine = 24,
    BadSeq = 25,
    BadPrevHeader = 26,
    BadFirstBlock = 27,
    BadBlockRange = 28,
    TimeRegression = 29,
    TimeInFuture = 30,
    BadBatchHash = 31,
    BatchTooLarge = 32,
    BadInboxThrough = 33,
    BadInboxAcc = 34,
    BadEpoch = 35,
    UnknownSigners = 36,
    BadSignerIndex = 37,
    BelowThreshold = 38,
    Insolvent = 39,
    // §13.4 rotation.
    RotationTooSoon = 40,
    SignersReused = 41,
    // §13.5 claims.
    WrongRecipient = 50,
    UnknownCheckpoint = 51,
    BadIndex = 52,
    AlreadyClaimed = 53,
    BadProof = 54,
    // §13.6 freeze and escape.
    NotFrozen = 60,
    FreezeNotAllowed = 61,
    NotRefundable = 62,
    UnknownInboxMessage = 63,
    /// The escape payout does not compute (issue #145, S-01): the claim is
    /// refused and stays unclaimed, never paid 0.
    PayoutOverflow = 64,
}

// --- Events (spec §13.2) --------------------------------------------------------

#[contractevent(topics = ["inbox"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InboxEvent {
    #[topic]
    pub index: u64,
    pub kind: u32,
    pub lane_account: BytesN<32>,
    pub amount: i128,
    pub enqueued_at: u64,
    pub acc_after: BytesN<32>,
    pub from: Address,
}

#[contractevent(topics = ["ckpt"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckpointEvent {
    #[topic]
    pub seq: u64,
    pub header_hash: BytesN<32>,
    pub last_block_height: u64,
    pub withdrawals_total: i128,
}

#[contractevent(topics = ["claim"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimedEvent {
    #[topic]
    pub seq: u64,
    #[topic]
    pub index: u32,
    pub recipient: Address,
    pub amount: i128,
}

#[contractevent(topics = ["frozen"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrozenEvent {
    pub payout_num: i128,
    pub payout_den: i128,
}

#[contractevent(topics = ["escape"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscapeClaimedEvent {
    #[topic]
    pub lane_account: BytesN<32>,
    pub amount: i128,
}

#[contractevent(topics = ["rotate"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignersRotatedEvent {
    #[topic]
    pub epoch: u64,
    pub signers_hash: BytesN<32>,
}

/// Testnet-only admin power (spec §13.2, §4.3).
#[contractevent(topics = ["admin_rotate"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminRotateEvent {
    #[topic]
    pub epoch: u64,
    pub signers_hash: BytesN<32>,
}

/// Testnet-only admin power (spec §13.2, §4.3).
#[contractevent(topics = ["upgrade"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeEvent {
    pub new_wasm_hash: BytesN<32>,
}

#[contractevent(topics = ["refund"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundEvent {
    #[topic]
    pub index: u64,
    pub from: Address,
    pub amount: i128,
}
