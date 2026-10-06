//! Caravel settlement contract (spec §13): the lane's USDC vault on Stellar,
//! the Stellar → lane inbox, checkpoint verification, withdrawals against a
//! Merkle root, and the freeze / escape path.
//!
//! The contract does not execute lane blocks and does not parse the batch: it
//! checks a 2-of-3 (weighted) validator signature over a header that commits to
//! the batch, and keeps the batch only as a transaction argument (calldata DA,
//! DEC-005). Validators re-execute; anyone can replay (spec §2.1).
#![no_std]
#![deny(clippy::float_arithmetic)]

extern crate alloc;

mod signers;
mod storage;
pub mod types;

use alloc::vec::Vec as AllocVec;

use caravel_core::checkpoint::{CheckpointHeaderV1, CHECKPOINT_HEADER_LEN};
use caravel_core::fixed::mul_div_floor_wide;
use caravel_core::inbox::{inbox_acc_preimage, InboxKind, InboxMsgV1};
use caravel_core::merkle::SorobanSha256;
use caravel_core::preimage::{
    account_leaf_preimage, rotate_message_preimage, withdrawal_leaf_preimage,
};
use soroban_sdk::token::TokenClient;
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{
    contract, contractimpl, panic_with_error, Address, Bytes, BytesN, ContractExecutable, Env, Vec,
};

use storage::{
    bump_instance, config, i128_of, last, require_not_frozen, set, set_instance, u64_of,
};
use types::*;

/// `ScVal::Address(ScAddress::Account(PublicKey::Ed25519(key)))` XDR without the key (spec §13.5).
pub const ACCOUNT_XDR_PREFIX: [u8; 12] = [0, 0, 0, 0x12, 0, 0, 0, 0, 0, 0, 0, 0];
/// Batch size cap (spec §3.3), the same one the lane's codec enforces.
pub const MAX_BATCH_BYTES: u32 = caravel_core::batch::MAX_BATCH_BYTES as u32;

fn sha256(env: &Env, data: &Bytes) -> BytesN<32> {
    env.crypto().sha256(data).into()
}

fn bytes32(env: &Env, a: &[u8; 32]) -> BytesN<32> {
    BytesN::from_array(env, a)
}

/// `H(ScVal XDR of this contract's address)`.
fn settlement_addr_hash(env: &Env) -> BytesN<32> {
    sha256(env, &env.current_contract_address().to_xdr(env))
}

/// Only the owner's `G...` account can receive: its XDR is the prefix and the key.
fn require_owner(env: &Env, who: &Address, lane_account: &BytesN<32>) -> Result<(), Error> {
    let mut expected = Bytes::from_slice(env, &ACCOUNT_XDR_PREFIX);
    expected.append(&lane_account.clone().into());
    if who.clone().to_xdr(env) == expected {
        Ok(())
    } else {
        Err(Error::NotOwner)
    }
}

fn inbox_msg(env: &Env, index: u64) -> Option<InboxMsg> {
    storage::get(env, &DataKey::Inbox(index))
}

/// `cum_deposits_after` of message `n − 1`, or 0 for `n = 0`.
fn cum_deposits(env: &Env, n: u64) -> i128 {
    if n == 0 {
        0
    } else {
        inbox_msg(env, n - 1).map_or(0, |m| m.cum_deposits_after)
    }
}

fn balance(env: &Env) -> i128 {
    TokenClient::new(env, &config(env).usdc).balance(&env.current_contract_address())
}

/// `balance − outstanding withdrawals − deposits the lane has not processed`.
fn available(env: &Env, inbox_through: u64) -> i128 {
    let outstanding = i128_of(env, &DataKey::TotalWithdrawalsCommitted)
        - i128_of(env, &DataKey::TotalWithdrawalsClaimed);
    let unprocessed =
        cum_deposits(env, u64_of(env, &DataKey::InboxCount)) - cum_deposits(env, inbox_through);
    balance(env) - outstanding - unprocessed
}

fn proof_array(proof: &Vec<BytesN<32>>) -> AllocVec<[u8; 32]> {
    proof.iter().map(|p| p.to_array()).collect()
}

fn or_panic<T>(env: &Env, r: Result<T, Error>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => panic_with_error!(env, e),
    }
}

#[contract]
pub struct Settlement;

#[contractimpl]
impl Settlement {
    #[allow(clippy::too_many_arguments)]
    pub fn __constructor(
        env: Env,
        admin: Address,
        usdc: Address,
        lane_id: BytesN<32>,
        engine_wasm_hash: BytesN<32>,
        genesis_state_hash: BytesN<32>,
        config_hash: BytesN<32>,
        signers: WeightedSigners,
        params: Params,
    ) {
        or_panic(&env, signers::validate(&signers));
        if params.min_deposit < 1 {
            panic_with_error!(&env, Error::BadParams);
        }
        let hash = signers::signers_hash(&env, &signers);
        let now = env.ledger().timestamp();
        let zero = BytesN::from_array(&env, &[0; 32]);
        set_instance(
            &env,
            &DataKey::Config,
            &Config {
                admin,
                usdc,
                lane_id,
                engine_wasm_hash,
                genesis_state_hash: genesis_state_hash.clone(),
                config_hash,
                params,
            },
        );
        set_instance(&env, &DataKey::Epoch, &1u64);
        set_instance(&env, &DataKey::MinValidEpoch, &1u64);
        set_instance(&env, &DataKey::InboxCount, &0u64);
        set_instance(&env, &DataKey::LastRotationAt, &now);
        set_instance(&env, &DataKey::TotalWithdrawalsCommitted, &0i128);
        set_instance(&env, &DataKey::TotalWithdrawalsClaimed, &0i128);
        set_instance(
            &env,
            &DataKey::LastCkpt,
            &LastCheckpoint {
                seq: 0,
                header_hash: zero.clone(),
                last_block_height: 0,
                last_block_hash: zero.clone(),
                last_block_timestamp_ms: 0,
                state_hash: genesis_state_hash,
                accounts_root: zero,
                account_count: 0,
                escape_total: 0,
                inbox_through: 0,
                accepted_at: now,
            },
        );
        set(&env, &DataKey::Signers(1), &signers);
        set(&env, &DataKey::SignersEpoch(hash), &1u64);
        bump_instance(&env);
    }

    // --- Inbox (Stellar → lane) --------------------------------------------------

    /// Moves USDC into the vault and queues a DEPOSIT for the lane. Returns the inbox index.
    pub fn deposit(env: Env, from: Address, amount: i128, lane_account: BytesN<32>) -> u64 {
        from.require_auth();
        bump_instance(&env);
        or_panic(&env, require_not_frozen(&env));
        if amount < config(&env).params.min_deposit {
            panic_with_error!(&env, Error::BelowMinDeposit);
        }
        TokenClient::new(&env, &config(&env).usdc).transfer(
            &from,
            env.current_contract_address(),
            &amount,
        );
        Self::append_inbox(&env, InboxKind::Deposit, from, lane_account, amount)
    }

    /// Queues a FORCED_WITHDRAWAL; only the account's owner may ask (spec §13.2).
    pub fn request_forced_withdrawal(
        env: Env,
        owner: Address,
        lane_account: BytesN<32>,
        amount: i128,
    ) -> u64 {
        owner.require_auth();
        bump_instance(&env);
        or_panic(&env, require_not_frozen(&env));
        or_panic(&env, require_owner(&env, &owner, &lane_account));
        if amount <= 0 {
            panic_with_error!(&env, Error::BadAmount);
        }
        Self::append_inbox(
            &env,
            InboxKind::ForcedWithdrawal,
            owner,
            lane_account,
            amount,
        )
    }

    fn append_inbox(
        env: &Env,
        kind: InboxKind,
        from: Address,
        lane_account: BytesN<32>,
        amount: i128,
    ) -> u64 {
        let index = u64_of(env, &DataKey::InboxCount);
        let enqueued_at = env.ledger().timestamp();
        let msg = InboxMsgV1 {
            kind,
            index,
            lane_account: lane_account.to_array(),
            amount,
            enqueued_at,
        }
        .encode();
        let (acc_prev, cum_prev) = match index.checked_sub(1).and_then(|i| inbox_msg(env, i)) {
            Some(m) => (m.acc_after.to_array(), m.cum_deposits_after),
            None => ([0u8; 32], 0),
        };
        let acc_after = sha256(
            env,
            &Bytes::from_slice(env, &inbox_acc_preimage(&acc_prev, &msg)),
        );
        let cum_deposits_after = if kind == InboxKind::Deposit {
            cum_prev + amount
        } else {
            cum_prev
        };
        let kind = kind as u32;
        set(
            env,
            &DataKey::Inbox(index),
            &InboxMsg {
                kind,
                from: from.clone(),
                lane_account: lane_account.clone(),
                amount,
                enqueued_at,
                acc_after: acc_after.clone(),
                cum_deposits_after,
                refunded: false,
            },
        );
        set_instance(env, &DataKey::InboxCount, &(index + 1));
        env.events().publish_event(&InboxEvent {
            index,
            kind,
            lane_account,
            amount,
            enqueued_at,
            acc_after,
            from,
        });
        index
    }

    // --- Checkpoints (spec §13.3) --------------------------------------------------

    /// Accepts a checkpoint signed by the current (or a recent) validator set.
    /// The batch is only hashed here; validators and replayers parse it.
    pub fn submit_checkpoint(env: Env, header: Bytes, batch: Bytes, epoch: u64, sigs: Vec<Sig>) {
        bump_instance(&env);
        or_panic(
            &env,
            Self::check_and_accept(&env, &header, &batch, epoch, &sigs),
        );
    }

    fn check_and_accept(
        env: &Env,
        header: &Bytes,
        batch: &Bytes,
        epoch: u64,
        sigs: &Vec<Sig>,
    ) -> Result<(), Error> {
        // 1. Not frozen.
        require_not_frozen(env)?;
        // 2. Encoding: 442 bytes, magic, version.
        if header.len() as usize != CHECKPOINT_HEADER_LEN {
            return Err(Error::BadHeaderEncoding);
        }
        let mut raw = [0u8; CHECKPOINT_HEADER_LEN];
        header.copy_into_slice(&mut raw);
        let h = CheckpointHeaderV1::decode(&raw).map_err(|_| Error::BadHeaderEncoding)?;
        let cfg = config(env);
        let last = last(env);
        // 3. Identity.
        if h.lane_id != cfg.lane_id.to_array() {
            return Err(Error::WrongLane);
        }
        if h.network_id != env.ledger().network_id().to_array() {
            return Err(Error::WrongNetwork);
        }
        if h.settlement_addr_hash != settlement_addr_hash(env).to_array() {
            return Err(Error::WrongSettlement);
        }
        if h.engine_wasm_hash != cfg.engine_wasm_hash.to_array() {
            return Err(Error::WrongEngine);
        }
        // 4. Chain.
        if h.seq != last.seq + 1 {
            return Err(Error::BadSeq);
        }
        if h.prev_header_hash != last.header_hash.to_array() {
            return Err(Error::BadPrevHeader);
        }
        if h.first_block_height != last.last_block_height + 1 {
            return Err(Error::BadFirstBlock);
        }
        if h.last_block_height < h.first_block_height {
            return Err(Error::BadBlockRange);
        }
        if h.last_block_timestamp_ms < last.last_block_timestamp_ms {
            return Err(Error::TimeRegression);
        }
        if h.last_block_timestamp_ms
            > env
                .ledger()
                .timestamp()
                .saturating_add(60)
                .saturating_mul(1000)
        {
            return Err(Error::TimeInFuture);
        }
        // 5. The header commits to the batch.
        if sha256(env, batch).to_array() != h.batch_hash {
            return Err(Error::BadBatchHash);
        }
        if batch.len() > MAX_BATCH_BYTES {
            return Err(Error::BatchTooLarge);
        }
        // 6. Inbox: the lane processed exactly the messages it claims.
        let inbox_count = u64_of(env, &DataKey::InboxCount);
        if h.inbox_through < last.inbox_through || h.inbox_through > inbox_count {
            return Err(Error::BadInboxThrough);
        }
        let expected_acc = match h.inbox_through.checked_sub(1) {
            None => [0u8; 32],
            Some(i) => inbox_msg(env, i)
                .ok_or(Error::BadInboxThrough)?
                .acc_after
                .to_array(),
        };
        if h.inbox_acc != expected_acc {
            return Err(Error::BadInboxAcc);
        }
        // 7. Signatures.
        let current = u64_of(env, &DataKey::Epoch);
        let min_valid = u64_of(env, &DataKey::MinValidEpoch);
        if epoch < min_valid
            || epoch > current
            || current - epoch > u64::from(cfg.params.signer_retention_epochs)
        {
            return Err(Error::BadEpoch);
        }
        let set_: WeightedSigners =
            storage::get(env, &DataKey::Signers(epoch)).ok_or(Error::UnknownSigners)?;
        let header_hash = sha256(env, header);
        signers::verify(env, &set_, &header_hash, sigs)?;
        // 8. Solvency of the new withdrawals.
        if h.withdrawals_total > available(env, h.inbox_through) {
            return Err(Error::Insolvent);
        }
        // 9. Effects. Only a checkpoint with withdrawals keeps a record: the
        // claims read it. Without one, a checkpoint is `LastCkpt` until the
        // next, and its `ckpt` event (M0.10 K-04, DEC-124). The record's rent
        // was ~97% of a checkpoint's fee.
        if h.withdrawal_count > 0 {
            set(
                env,
                &DataKey::Ckpt(h.seq),
                &CheckpointRecord {
                    header_hash: header_hash.clone(),
                    withdrawals_root: bytes32(env, &h.withdrawals_root),
                    withdrawal_count: h.withdrawal_count,
                    withdrawals_total: h.withdrawals_total,
                    claimed_total: 0,
                    stellar_ledger: env.ledger().sequence(),
                },
            );
        }
        set_instance(
            env,
            &DataKey::LastCkpt,
            &LastCheckpoint {
                seq: h.seq,
                header_hash: header_hash.clone(),
                last_block_height: h.last_block_height,
                last_block_hash: bytes32(env, &h.last_block_hash),
                last_block_timestamp_ms: h.last_block_timestamp_ms,
                state_hash: bytes32(env, &h.state_hash),
                accounts_root: bytes32(env, &h.accounts_root),
                account_count: h.account_count,
                escape_total: h.escape_total,
                inbox_through: h.inbox_through,
                accepted_at: env.ledger().timestamp(),
            },
        );
        let committed = i128_of(env, &DataKey::TotalWithdrawalsCommitted) + h.withdrawals_total;
        set_instance(env, &DataKey::TotalWithdrawalsCommitted, &committed);
        env.events().publish_event(&CheckpointEvent {
            seq: h.seq,
            header_hash,
            last_block_height: h.last_block_height,
            withdrawals_total: h.withdrawals_total,
        });
        Ok(())
    }

    // --- Withdrawals (spec §13.5) -------------------------------------------------

    /// Pays a withdrawal leaf of checkpoint `seq` to its owner. Anyone may submit
    /// it; the funds only go to the owner's `G...` address. Works when frozen.
    pub fn claim_withdrawal(
        env: Env,
        recipient: Address,
        lane_account: BytesN<32>,
        seq: u64,
        index: u32,
        amount: i128,
        proof: Vec<BytesN<32>>,
    ) {
        bump_instance(&env);
        if require_owner(&env, &recipient, &lane_account).is_err() {
            panic_with_error!(&env, Error::WrongRecipient);
        }
        let key = DataKey::Ckpt(seq);
        let mut record: CheckpointRecord = or_panic(
            &env,
            storage::get(&env, &key).ok_or(Error::UnknownCheckpoint),
        );
        if index >= record.withdrawal_count {
            panic_with_error!(&env, Error::BadIndex);
        }
        let claimed = DataKey::Claimed(seq, index);
        if storage::has(&env, &claimed) {
            panic_with_error!(&env, Error::AlreadyClaimed);
        }
        let cfg = config(&env);
        let leaf_pre = withdrawal_leaf_preimage(
            &cfg.lane_id.to_array(),
            seq,
            index,
            &lane_account.to_array(),
            amount,
        );
        let leaf = sha256(&env, &Bytes::from_slice(&env, &leaf_pre)).to_array();
        if !caravel_core::merkle::verify(
            &SorobanSha256(&env),
            &leaf,
            index,
            record.withdrawal_count,
            &proof_array(&proof),
            &record.withdrawals_root.to_array(),
        ) {
            panic_with_error!(&env, Error::BadProof);
        }
        set(&env, &claimed, &true);
        record.claimed_total += amount;
        set(&env, &key, &record);
        let total = i128_of(&env, &DataKey::TotalWithdrawalsClaimed) + amount;
        set_instance(&env, &DataKey::TotalWithdrawalsClaimed, &total);
        TokenClient::new(&env, &cfg.usdc).transfer(
            &env.current_contract_address(),
            &recipient,
            &amount,
        );
        env.events().publish_event(&ClaimedEvent {
            seq,
            index,
            recipient,
            amount,
        });
    }

    // --- Signer rotation (spec §13.4) ------------------------------------------------

    /// Rotates to `new`, signed by the current set, at least `min_rotation_delay_secs`
    /// after the last rotation. A set that was ever used cannot come back.
    pub fn rotate_signers(env: Env, new: WeightedSigners, epoch: u64, sigs: Vec<Sig>) {
        bump_instance(&env);
        let current = u64_of(&env, &DataKey::Epoch);
        if epoch != current {
            panic_with_error!(&env, Error::BadEpoch);
        }
        let cfg = config(&env);
        let now = env.ledger().timestamp();
        if now.saturating_sub(u64_of(&env, &DataKey::LastRotationAt))
            < cfg.params.min_rotation_delay_secs
        {
            panic_with_error!(&env, Error::RotationTooSoon);
        }
        or_panic(&env, signers::validate(&new));
        let new_hash = signers::signers_hash(&env, &new);
        let msg = rotate_message_preimage(
            &cfg.lane_id.to_array(),
            &env.ledger().network_id().to_array(),
            &settlement_addr_hash(&env).to_array(),
            current + 1,
            &new_hash.to_array(),
        );
        let msg_hash = sha256(&env, &Bytes::from_slice(&env, &msg));
        let set_: WeightedSigners = or_panic(
            &env,
            storage::get(&env, &DataKey::Signers(current)).ok_or(Error::UnknownSigners),
        );
        or_panic(&env, signers::verify(&env, &set_, &msg_hash, &sigs));
        Self::install_signers(&env, new, new_hash.clone(), now);
        env.events().publish_event(&SignersRotatedEvent {
            epoch: current + 1,
            signers_hash: new_hash,
        });
    }

    /// **Testnet only** (spec §4.3): the admin rotates at once, and every older
    /// set stops being valid immediately.
    pub fn admin_rotate_signers(env: Env, new: WeightedSigners) {
        let cfg = config(&env);
        cfg.admin.require_auth();
        bump_instance(&env);
        or_panic(&env, signers::validate(&new));
        let new_hash = signers::signers_hash(&env, &new);
        let epoch = Self::install_signers(&env, new, new_hash.clone(), env.ledger().timestamp());
        set_instance(&env, &DataKey::MinValidEpoch, &epoch);
        env.events().publish_event(&AdminRotateEvent {
            epoch,
            signers_hash: new_hash,
        });
    }

    fn install_signers(env: &Env, new: WeightedSigners, hash: BytesN<32>, now: u64) -> u64 {
        if storage::has(env, &DataKey::SignersEpoch(hash.clone())) {
            panic_with_error!(env, Error::SignersReused);
        }
        let epoch = u64_of(env, &DataKey::Epoch) + 1;
        set_instance(env, &DataKey::Epoch, &epoch);
        set(env, &DataKey::Signers(epoch), &new);
        set(env, &DataKey::SignersEpoch(hash), &epoch);
        set_instance(env, &DataKey::LastRotationAt, &now);
        epoch
    }

    // --- Freeze and escape (spec §13.6) ----------------------------------------------

    /// Anyone may freeze once the lane stopped checkpointing for
    /// `escape_timeout_secs`, or left an inbox message unprocessed for longer
    /// than `force_inclusion_window_secs`. There is no unfreeze.
    pub fn freeze(env: Env) {
        bump_instance(&env);
        or_panic(&env, require_not_frozen(&env));
        let cfg = config(&env);
        let last = last(&env);
        let now = env.ledger().timestamp();
        let stalled = now.saturating_sub(last.accepted_at) > cfg.params.escape_timeout_secs;
        let censored = u64_of(&env, &DataKey::InboxCount) > last.inbox_through
            && inbox_msg(&env, last.inbox_through).is_some_and(|m| {
                now.saturating_sub(m.enqueued_at) > cfg.params.force_inclusion_window_secs
            });
        if !stalled && !censored {
            panic_with_error!(&env, Error::FreezeNotAllowed);
        }
        let payout_den = last.escape_total;
        let payout_num = available(&env, last.inbox_through).min(payout_den);
        set_instance(
            &env,
            &DataKey::Frozen,
            &FrozenInfo {
                at: now,
                payout_num,
                payout_den,
            },
        );
        env.events().publish_event(&FrozenEvent {
            payout_num,
            payout_den,
        });
    }

    /// Pays an account its share of the last checkpointed equity, pro rata:
    /// `equity × payout_num / payout_den`.
    pub fn escape_claim(
        env: Env,
        recipient: Address,
        lane_account: BytesN<32>,
        index: u32,
        equity: i128,
        proof: Vec<BytesN<32>>,
    ) {
        bump_instance(&env);
        let frozen: FrozenInfo = or_panic(
            &env,
            storage::instance(&env, &DataKey::Frozen).ok_or(Error::NotFrozen),
        );
        if require_owner(&env, &recipient, &lane_account).is_err() {
            panic_with_error!(&env, Error::WrongRecipient);
        }
        let claimed = DataKey::EscapeClaimed(lane_account.clone());
        if storage::has(&env, &claimed) {
            panic_with_error!(&env, Error::AlreadyClaimed);
        }
        let cfg = config(&env);
        let last = last(&env);
        let leaf_pre = account_leaf_preimage(
            &cfg.lane_id.to_array(),
            last.seq,
            index,
            &lane_account.to_array(),
            equity,
        );
        let leaf = sha256(&env, &Bytes::from_slice(&env, &leaf_pre)).to_array();
        if !caravel_core::merkle::verify(
            &SorobanSha256(&env),
            &leaf,
            index,
            last.account_count,
            &proof_array(&proof),
            &last.accounts_root.to_array(),
        ) {
            panic_with_error!(&env, Error::BadProof);
        }
        // Nothing to pay when the vault held nothing for escapes (payout_num ≤ 0).
        // Otherwise exact, with a 256-bit intermediate: a payout that doesn't
        // compute refuses the claim, which stays unclaimed (issue #145, S-01).
        let amount = if frozen.payout_den <= 0 || frozen.payout_num <= 0 || equity <= 0 {
            0
        } else {
            or_panic(
                &env,
                mul_div_floor_wide(equity, frozen.payout_num, frozen.payout_den)
                    .map_err(|_| Error::PayoutOverflow),
            )
        };
        set(&env, &claimed, &true);
        if amount > 0 {
            TokenClient::new(&env, &cfg.usdc).transfer(
                &env.current_contract_address(),
                &recipient,
                &amount,
            );
        }
        env.events().publish_event(&EscapeClaimedEvent {
            lane_account,
            amount,
        });
    }

    /// After a freeze, returns a deposit the lane never processed to its sender, 1:1.
    pub fn refund_unprocessed_deposit(env: Env, index: u64) {
        bump_instance(&env);
        if !storage::is_frozen(&env) {
            panic_with_error!(&env, Error::NotFrozen);
        }
        let key = DataKey::Inbox(index);
        let mut msg: InboxMsg = or_panic(
            &env,
            storage::get(&env, &key).ok_or(Error::UnknownInboxMessage),
        );
        if index < last(&env).inbox_through || msg.kind != InboxKind::Deposit as u32 || msg.refunded
        {
            panic_with_error!(&env, Error::NotRefundable);
        }
        msg.refunded = true;
        set(&env, &key, &msg);
        TokenClient::new(&env, &config(&env).usdc).transfer(
            &env.current_contract_address(),
            &msg.from,
            &msg.amount,
        );
        env.events().publish_event(&RefundEvent {
            index,
            from: msg.from,
            amount: msg.amount,
        });
    }

    /// **Testnet only** (spec §4.3): the admin replaces the contract code.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
        config(&env).admin.require_auth();
        env.deployer()
            .update_current_contract(ContractExecutable::Wasm(new_wasm_hash.clone()));
        env.events().publish_event(&UpgradeEvent { new_wasm_hash });
    }

    // --- Views ---------------------------------------------------------------------

    pub fn config(env: Env) -> Config {
        config(&env)
    }

    pub fn last_checkpoint(env: Env) -> LastCheckpoint {
        last(&env)
    }

    pub fn checkpoint(env: Env, seq: u64) -> Option<CheckpointRecord> {
        storage::get(&env, &DataKey::Ckpt(seq))
    }

    pub fn inbox(env: Env, index: u64) -> Option<InboxMsg> {
        inbox_msg(&env, index)
    }

    pub fn inbox_count(env: Env) -> u64 {
        u64_of(&env, &DataKey::InboxCount)
    }

    pub fn signers(env: Env, epoch: u64) -> Option<WeightedSigners> {
        storage::get(&env, &DataKey::Signers(epoch))
    }

    pub fn epoch(env: Env) -> u64 {
        u64_of(&env, &DataKey::Epoch)
    }

    pub fn min_valid_epoch(env: Env) -> u64 {
        u64_of(&env, &DataKey::MinValidEpoch)
    }

    pub fn frozen(env: Env) -> bool {
        storage::is_frozen(&env)
    }

    pub fn frozen_info(env: Env) -> Option<FrozenInfo> {
        storage::instance(&env, &DataKey::Frozen)
    }

    pub fn is_claimed(env: Env, seq: u64, index: u32) -> bool {
        env.storage()
            .persistent()
            .has(&DataKey::Claimed(seq, index))
    }

    pub fn escape_claimed(env: Env, lane_account: BytesN<32>) -> bool {
        env.storage()
            .persistent()
            .has(&DataKey::EscapeClaimed(lane_account))
    }

    /// `(TotalWithdrawalsCommitted, TotalWithdrawalsClaimed)`.
    pub fn totals(env: Env) -> (i128, i128) {
        (
            i128_of(&env, &DataKey::TotalWithdrawalsCommitted),
            i128_of(&env, &DataKey::TotalWithdrawalsClaimed),
        )
    }
}

#[cfg(test)]
mod test;
