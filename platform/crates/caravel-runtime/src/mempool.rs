//! The sequencer's FIFO mempool and pre-validation (spec §14.1).
//!
//! A bad signature on a user transaction or an oracle update is fatal to the
//! whole block (spec §8.3), so every signature is checked here with the
//! engine's own rule (`verify_strict`) before anything reaches a block.

use std::collections::{HashSet, VecDeque};

use caravel_perps::native::verify_strict;
use caravel_types::oracle::OracleUpdateV1;
use caravel_types::state::StateV1;
use caravel_types::tx::{sep53_preimage, sep53_tx_message, LaneTxV1, SigScheme};

use crate::checkpoint::sha256;

/// Why a transaction was refused at the API (`POST /v1/tx` 400 `{error, code}`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    Decode,
    WrongLane,
    BadSignature,
    Expired,
    /// `nonce < next_nonce`: already used.
    StaleNonce,
    UnknownAccount,
    Duplicate,
    MempoolFull,
    AccountQueueFull,
}

impl Reject {
    pub fn code(self) -> &'static str {
        match self {
            Self::Decode => "DECODE",
            Self::WrongLane => "WRONG_LANE",
            Self::BadSignature => "BAD_SIGNATURE",
            Self::Expired => "EXPIRED",
            Self::StaleNonce => "BAD_NONCE",
            Self::UnknownAccount => "UNKNOWN_ACCOUNT",
            Self::Duplicate => "DUPLICATE",
            Self::MempoolFull => "MEMPOOL_FULL",
            Self::AccountQueueFull => "ACCOUNT_QUEUE_FULL",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Decode => "the transaction does not decode as LaneTxV1",
            Self::WrongLane => "the transaction is for another lane",
            Self::BadSignature => "the signature does not verify",
            Self::Expired => "the transaction has expired",
            Self::StaleNonce => "the nonce was already used",
            Self::UnknownAccount => "no lane account for this key; deposit first",
            Self::Duplicate => "the transaction is already queued",
            Self::MempoolFull => "the mempool is full",
            Self::AccountQueueFull => "too many queued transactions for this account",
        }
    }
}

/// `H(TAG_TX || config_hash || unsigned tx)`.
pub fn tx_hash(tx: &LaneTxV1, config_hash: &[u8; 32]) -> [u8; 32] {
    sha256(&tx.tx_hash_preimage(config_hash))
}

/// The engine's signature check for a user transaction (spec §11.3).
pub fn tx_signature_ok(tx: &LaneTxV1, config_hash: &[u8; 32]) -> bool {
    let hash = tx_hash(tx, config_hash);
    match tx.sig_scheme {
        SigScheme::RawEd25519 => verify_strict(&tx.signer, &hash, &tx.signature),
        SigScheme::Sep53 => {
            let msg = sha256(&sep53_preimage(&sep53_tx_message(&hash)));
            verify_strict(&tx.signer, &msg, &tx.signature)
        }
    }
}

/// The engine's fatal oracle checks (spec §11.5): a configured key and a valid signature.
pub fn oracle_ok(u: &OracleUpdateV1, state: &StateV1) -> bool {
    state
        .config
        .oracle_keys
        .binary_search(&u.oracle_key)
        .is_ok()
        && verify_strict(
            &u.oracle_key,
            &sha256(&u.signing_preimage(&state.lane_id)),
            &u.signature,
        )
}

/// Pre-validates encoded transaction bytes against the current state.
pub fn prevalidate(
    bytes: &[u8],
    state: &StateV1,
    config_hash: &[u8; 32],
    now_ms: u64,
) -> Result<([u8; 32], LaneTxV1), Reject> {
    let tx = LaneTxV1::decode(bytes).map_err(|_| Reject::Decode)?;
    if tx.lane_id != state.lane_id {
        return Err(Reject::WrongLane);
    }
    if !tx_signature_ok(&tx, config_hash) {
        return Err(Reject::BadSignature);
    }
    if tx.expiry_ms <= now_ms {
        return Err(Reject::Expired);
    }
    let Some(account) = state.accounts.iter().find(|a| a.key == tx.account) else {
        return Err(Reject::UnknownAccount);
    };
    if tx.nonce < account.next_nonce {
        return Err(Reject::StaleNonce);
    }
    Ok((tx_hash(&tx, config_hash), tx))
}

#[derive(Clone, Debug)]
pub struct Queued {
    pub hash: [u8; 32],
    pub tx: LaneTxV1,
}

/// FIFO by arrival, deduplicated by tx hash.
#[derive(Debug)]
pub struct Mempool {
    queue: VecDeque<Queued>,
    hashes: HashSet<[u8; 32]>,
    max_total: usize,
    max_per_account: usize,
}

impl Mempool {
    pub fn new(max_total: usize, max_per_account: usize) -> Self {
        Self {
            queue: VecDeque::new(),
            hashes: HashSet::new(),
            max_total,
            max_per_account,
        }
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Queued> {
        self.queue.iter()
    }

    pub fn contains(&self, hash: &[u8; 32]) -> bool {
        self.hashes.contains(hash)
    }

    pub fn push(&mut self, hash: [u8; 32], tx: LaneTxV1) -> Result<(), Reject> {
        if self.hashes.contains(&hash) {
            return Err(Reject::Duplicate);
        }
        if self.queue.len() >= self.max_total {
            return Err(Reject::MempoolFull);
        }
        if self
            .queue
            .iter()
            .filter(|q| q.tx.account == tx.account)
            .count()
            >= self.max_per_account
        {
            return Err(Reject::AccountQueueFull);
        }
        self.hashes.insert(hash);
        self.queue.push_back(Queued { hash, tx });
        Ok(())
    }

    /// Removes the given transactions (included in a block, stale or quarantined).
    pub fn remove(&mut self, hashes: &HashSet<[u8; 32]>) {
        if hashes.is_empty() {
            return;
        }
        self.queue.retain(|q| !hashes.contains(&q.hash));
        self.hashes.retain(|h| !hashes.contains(h));
    }
}
