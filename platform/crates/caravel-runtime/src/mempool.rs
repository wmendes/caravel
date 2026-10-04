//! The sequencer's FIFO mempool and pre-validation (spec §14.1), for any app.
//!
//! A bad signature on a user transaction or a feed update is fatal to the
//! whole block (spec §8.3), so every signature is checked here with the
//! engine's own rule (`verify_strict`) before anything reaches a block.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use caravel_core::state::StateFrameV1;
use caravel_core::tx::{sep53_preimage, sep53_tx_message, SigScheme, TxEnvelopeV1};

use crate::app::LaneApp;
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
    /// Another queued transaction of the account has this nonce: only one
    /// of them could run, so the second is refused at once.
    NonceQueued,
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
            Self::NonceQueued => "NONCE_QUEUED",
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
            Self::NonceQueued => {
                "a queued transaction of this account has this nonce: sign with the next one"
            }
            Self::MempoolFull => "the mempool is full",
            Self::AccountQueueFull => "too many queued transactions for this account",
        }
    }
}

/// ed25519 `verify_strict` over a 32-byte message, the host's rule (spec §8.4).
/// A key that does not decode is simply not valid.
pub fn verify_strict(key: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> bool {
    let Ok(vk) = ed25519_dalek::VerifyingKey::from_bytes(key) else {
        return false;
    };
    vk.verify_strict(msg, &ed25519_dalek::Signature::from_bytes(sig))
        .is_ok()
}

/// `H(TAG_TX || config_hash || unsigned tx)`.
pub fn tx_hash(tx: &TxEnvelopeV1, config_hash: &[u8; 32]) -> [u8; 32] {
    sha256(&tx.tx_hash_preimage(config_hash))
}

/// The engine's signature check for a user transaction (spec §11.3).
pub fn tx_signature_ok(tx: &TxEnvelopeV1, config_hash: &[u8; 32]) -> bool {
    let hash = tx_hash(tx, config_hash);
    match tx.sig_scheme {
        SigScheme::RawEd25519 => verify_strict(&tx.signer, &hash, &tx.signature),
        SigScheme::Sep53 => {
            let msg = sha256(&sep53_preimage(&sep53_tx_message(&hash)));
            verify_strict(&tx.signer, &msg, &tx.signature)
        }
    }
}

/// Pre-validates encoded transaction bytes against the current state.
pub fn prevalidate<A: LaneApp>(
    app: &A,
    bytes: &[u8],
    state: &A::State,
    frame: &StateFrameV1,
    config_hash: &[u8; 32],
    now_ms: u64,
) -> Result<([u8; 32], TxEnvelopeV1), Reject> {
    let tx = TxEnvelopeV1::decode(bytes).map_err(|_| Reject::Decode)?;
    if !app.tx_decodes(&tx) {
        return Err(Reject::Decode);
    }
    if tx.lane_id != frame.lane_id {
        return Err(Reject::WrongLane);
    }
    if !tx_signature_ok(&tx, config_hash) {
        return Err(Reject::BadSignature);
    }
    if tx.expiry_ms <= now_ms {
        return Err(Reject::Expired);
    }
    let Some(next_nonce) = app.next_nonce(state, &tx.account) else {
        return Err(Reject::UnknownAccount);
    };
    if tx.nonce < next_nonce {
        return Err(Reject::StaleNonce);
    }
    Ok((tx_hash(&tx, config_hash), tx))
}

#[derive(Clone, Debug)]
pub struct Queued {
    pub hash: [u8; 32],
    pub tx: TxEnvelopeV1,
}

/// FIFO by arrival, deduplicated by tx hash.
#[derive(Debug)]
pub struct Mempool {
    queue: VecDeque<Queued>,
    hashes: HashSet<[u8; 32]>,
    /// Each account's queued nonces (F-12): the per-transaction checks and
    /// `next_queued_nonce` no longer scan the whole queue.
    by_account: HashMap<[u8; 32], BTreeSet<u64>>,
    max_total: usize,
    max_per_account: usize,
}

impl Mempool {
    pub fn new(max_total: usize, max_per_account: usize) -> Self {
        Self {
            queue: VecDeque::new(),
            hashes: HashSet::new(),
            by_account: HashMap::new(),
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

    /// The nonce after `account`'s queued transactions, if it has any.
    pub fn next_queued_nonce(&self, account: &[u8; 32]) -> Option<u64> {
        self.by_account
            .get(account)
            .and_then(|n| n.last())
            .map(|n| n.saturating_add(1))
    }

    pub fn push(&mut self, hash: [u8; 32], tx: TxEnvelopeV1) -> Result<(), Reject> {
        if self.hashes.contains(&hash) {
            return Err(Reject::Duplicate);
        }
        let queued = self.by_account.get(&tx.account);
        if queued.is_some_and(|n| n.contains(&tx.nonce)) {
            return Err(Reject::NonceQueued);
        }
        if self.queue.len() >= self.max_total {
            return Err(Reject::MempoolFull);
        }
        if queued.map_or(0, |n| n.len()) >= self.max_per_account {
            return Err(Reject::AccountQueueFull);
        }
        self.by_account
            .entry(tx.account)
            .or_default()
            .insert(tx.nonce);
        self.hashes.insert(hash);
        self.queue.push_back(Queued { hash, tx });
        Ok(())
    }

    /// Removes the given transactions (included in a block, stale or quarantined).
    pub fn remove(&mut self, hashes: &HashSet<[u8; 32]>) {
        if hashes.is_empty() {
            return;
        }
        let index = &mut self.by_account;
        self.queue.retain(|q| {
            let keep = !hashes.contains(&q.hash);
            if !keep {
                if let Some(n) = index.get_mut(&q.tx.account) {
                    n.remove(&q.tx.nonce);
                    if n.is_empty() {
                        index.remove(&q.tx.account);
                    }
                }
            }
            keep
        });
        for h in hashes {
            self.hashes.remove(h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx(account: u8, nonce: u64) -> TxEnvelopeV1 {
        TxEnvelopeV1 {
            lane_id: [0; 32],
            account: [account; 32],
            signer: [account; 32],
            nonce,
            expiry_ms: 0,
            kind: 0,
            sig_scheme: SigScheme::RawEd25519,
            body: vec![],
            signature: [0; 64],
        }
    }

    #[test]
    fn the_account_index_follows_pushes_and_removals() {
        let mut m = Mempool::new(10, 2);
        m.push([1; 32], tx(1, 5)).unwrap();
        assert_eq!(m.push([2; 32], tx(1, 5)), Err(Reject::NonceQueued));
        m.push([3; 32], tx(1, 6)).unwrap();
        assert_eq!(m.push([4; 32], tx(1, 7)), Err(Reject::AccountQueueFull));
        m.push([5; 32], tx(2, 1)).unwrap();
        m.remove(&HashSet::from([[1; 32], [5; 32]]));
        assert_eq!(m.len(), 1);
        assert!(!m.contains(&[1; 32]));
        // The removed nonce and the room it held are free again.
        m.push([6; 32], tx(1, 5)).unwrap();
        assert_eq!(m.next_queued_nonce(&[1; 32]), Some(7));
        assert_eq!(m.next_queued_nonce(&[2; 32]), None);
        m.remove(&HashSet::from([[3; 32], [6; 32]]));
        assert!(m.is_empty() && m.by_account.is_empty());
    }

    #[test]
    fn next_queued_nonce_is_after_the_accounts_highest() {
        let mut m = Mempool::new(10, 10);
        assert_eq!(m.next_queued_nonce(&[1; 32]), None);
        m.push([1; 32], tx(1, 7)).unwrap();
        m.push([2; 32], tx(1, 5)).unwrap();
        m.push([3; 32], tx(2, 40)).unwrap();
        assert_eq!(m.next_queued_nonce(&[1; 32]), Some(8));
        assert_eq!(m.next_queued_nonce(&[2; 32]), Some(41));
        m.remove(&HashSet::from([[1; 32]]));
        assert_eq!(m.next_queued_nonce(&[1; 32]), Some(6));
    }

    #[test]
    fn a_second_transaction_with_a_queued_nonce_is_refused() {
        let mut m = Mempool::new(10, 10);
        m.push([1; 32], tx(1, 7)).unwrap();
        assert_eq!(m.push([1; 32], tx(1, 7)), Err(Reject::Duplicate));
        let mut other = tx(1, 7);
        other.kind = 1;
        assert_eq!(m.push([2; 32], other), Err(Reject::NonceQueued));
        // Another account's nonce 7 is its own.
        m.push([3; 32], tx(2, 7)).unwrap();
        m.remove(&HashSet::from([[1; 32]]));
        m.push([4; 32], tx(1, 7)).unwrap();
    }
}
