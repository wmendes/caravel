//! `submit_checkpoint` (spec §13.3): the happy path, then one rejection per
//! numbered check, in order.

use soroban_sdk::testutils::Events as _;
use soroban_sdk::Event as _;

use super::*;

/// A deposits 100 USDC; the lane processes it, then A withdraws 30 USDC and
/// the lane closes checkpoint 1.
fn ready() -> (Harness, Checkpoint) {
    let mut h = Harness::new();
    h.deposit(A, 100 * USDC);
    h.sync_inbox();
    h.lane.block();
    h.lane.all_ok();
    h.lane.withdraw(A, 30 * USDC);
    h.lane.block();
    h.lane.all_ok();
    let cp = h.checkpoint();
    (h, cp)
}

/// Re-signs `cp` with 2 of 3 at epoch 1 and expects `want`.
fn rejects(h: &Harness, cp: &Checkpoint, want: Error) {
    let sigs = h.sign(&cp.header, &[0, 2]);
    expect_err(h.try_submit(cp, 1, &sigs), want);
}

fn mutated(f: impl FnOnce(&mut CheckpointHeaderV1), want: Error) {
    let (h, mut cp) = ready();
    f(&mut cp.header);
    rejects(&h, &cp, want);
}

#[test]
fn happy_path_deposit_checkpoint_claim() {
    let (mut h, cp) = ready();
    assert_eq!(cp.header.seq, 1);
    assert_eq!(
        cp.withdrawals,
        std::vec![(caravel_harness::pk(A), 30 * USDC)]
    );
    h.accept(&cp);
    let header_hash = BytesN::from_array(&h.env, &sha256(&cp.header.encode()));
    let event = CheckpointEvent {
        seq: 1,
        header_hash: header_hash.clone(),
        last_block_height: 3,
        withdrawals_total: 30 * USDC,
    };
    assert_eq!(
        h.env
            .events()
            .all()
            .filter_by_contract(&h.id)
            .events()
            .last(),
        Some(&event.to_xdr(&h.env, &h.id))
    );

    // Check 9 effects.
    let last = h.c().last_checkpoint();
    assert_eq!(last.seq, 1);
    assert_eq!(last.header_hash, header_hash);
    assert_eq!(last.last_block_height, 3);
    assert_eq!(last.last_block_hash.to_array(), h.lane.prev_block_hash);
    assert_eq!(last.state_hash.to_array(), h.lane.state_hash());
    // A, plus the accounts the genesis config creates.
    assert_eq!(last.account_count as usize, h.lane.state().accounts.len());
    assert_eq!(last.account_count as usize, cp.accounts.len());
    assert_eq!(
        last.escape_total,
        cp.accounts.iter().map(|(_, e)| e).sum::<i128>()
    );
    assert!(cp.accounts.contains(&(caravel_harness::pk(A), 70 * USDC)));
    assert_eq!(last.inbox_through, 1);
    assert_eq!(last.accepted_at, START);
    let record = h.c().checkpoint(&1).expect("stored");
    assert_eq!(
        (
            record.header_hash,
            record.withdrawal_count,
            record.withdrawals_total,
            record.claimed_total
        ),
        (header_hash, 1, 30 * USDC, 0)
    );
    assert_eq!(record.stellar_ledger, 1_000);
    assert_eq!(h.c().totals(), (30 * USDC, 0));

    // Claim.
    let who = h.user(A, 0);
    let proof = h.withdrawal_proof(&cp, 0);
    h.c()
        .claim_withdrawal(&who, &key32(&h.env, A), &1, &0, &(30 * USDC), &proof);
    let event = ClaimedEvent {
        seq: 1,
        index: 0,
        recipient: who.clone(),
        amount: 30 * USDC,
    };
    assert_eq!(
        h.env
            .events()
            .all()
            .filter_by_contract(&h.id)
            .events()
            .last(),
        Some(&event.to_xdr(&h.env, &h.id))
    );
    assert_eq!(h.token().balance(&who), 30 * USDC);
    assert_eq!(h.token().balance(&h.id), 70 * USDC);
    assert!(h.c().is_claimed(&1, &0));
    assert_eq!(h.c().checkpoint(&1).unwrap().claimed_total, 30 * USDC);
    assert_eq!(h.c().totals(), (30 * USDC, 30 * USDC));
}

#[test]
fn checkpoints_chain() {
    let (mut h, cp1) = ready();
    h.accept(&cp1);
    h.deposit(B, 10 * USDC);
    h.sync_inbox();
    h.lane.block();
    let cp2 = h.checkpoint();
    assert_eq!(
        (
            cp2.header.seq,
            cp2.header.first_block_height,
            cp2.header.inbox_through
        ),
        (2, 4, 2)
    );
    assert_eq!(cp2.header.prev_header_hash, sha256(&cp1.header.encode()));
    h.accept(&cp2);
    assert_eq!(h.c().last_checkpoint().seq, 2);
    assert_eq!(h.c().last_checkpoint().inbox_through, 2);
}

// --- 1. Not frozen ------------------------------------------------------------------

#[test]
fn check_1_rejects_when_frozen() {
    let (h, cp) = ready();
    h.advance(PARAMS.escape_timeout_secs + 1);
    h.c().freeze();
    rejects(&h, &cp, Error::Frozen);
}

// --- 2. Encoding --------------------------------------------------------------------

#[test]
fn check_2_rejects_bad_encoding() {
    let (h, cp) = ready();
    let good = cp.header.encode();
    let sigs = h.sign(&cp.header, &[0, 2]);
    let batch = Bytes::from_slice(&h.env, &cp.batch);
    let submit = |raw: &[u8]| {
        h.c()
            .try_submit_checkpoint(&Bytes::from_slice(&h.env, raw), &batch, &1, &sigs)
    };
    expect_err(submit(&good[..441]), Error::BadHeaderEncoding);
    let mut long = good.to_vec();
    long.push(0);
    expect_err(submit(&long), Error::BadHeaderEncoding);
    let mut magic = good.to_vec();
    magic[0] ^= 1;
    expect_err(submit(&magic), Error::BadHeaderEncoding);
    let mut version = good.to_vec();
    version[8..10].copy_from_slice(&2u16.to_le_bytes());
    expect_err(submit(&version), Error::BadHeaderEncoding);
}

// --- 3. Identity --------------------------------------------------------------------

#[test]
fn check_3_rejects_another_lane() {
    mutated(|h| h.lane_id[0] ^= 1, Error::WrongLane);
}

#[test]
fn check_3_rejects_another_network() {
    mutated(
        |h| h.network_id = sha256(b"Standalone Network ; February 2017"),
        Error::WrongNetwork,
    );
}

#[test]
fn check_3_rejects_another_settlement_contract() {
    let (h, mut cp) = ready();
    let other = Address::generate(&h.env);
    cp.header.settlement_addr_hash = sha256(&other.to_xdr(&h.env).to_alloc_vec());
    rejects(&h, &cp, Error::WrongSettlement);
}

#[test]
fn check_3_rejects_another_engine() {
    mutated(|h| h.engine_wasm_hash = [0xAB; 32], Error::WrongEngine);
}

// --- 4. Chain -----------------------------------------------------------------------

#[test]
fn check_4_rejects_a_gap_in_seq() {
    mutated(|h| h.seq = 2, Error::BadSeq);
}

#[test]
fn check_4_rejects_a_replayed_checkpoint() {
    let (mut h, cp) = ready();
    h.accept(&cp);
    rejects(&h, &cp, Error::BadSeq);
}

#[test]
fn check_4_rejects_a_wrong_prev_header() {
    mutated(|h| h.prev_header_hash = [1; 32], Error::BadPrevHeader);
}

#[test]
fn check_4_rejects_a_wrong_first_block() {
    mutated(|h| h.first_block_height = 2, Error::BadFirstBlock);
}

#[test]
fn check_4_rejects_an_empty_block_range() {
    mutated(|h| h.last_block_height = 0, Error::BadBlockRange);
}

#[test]
fn check_4_rejects_time_going_backwards() {
    let (mut h, cp1) = ready();
    h.accept(&cp1);
    h.lane.block();
    let mut cp2 = h.checkpoint();
    cp2.header.last_block_timestamp_ms = cp1.header.last_block_timestamp_ms - 1;
    rejects(&h, &cp2, Error::TimeRegression);
    // Equal is allowed.
    cp2.header.last_block_timestamp_ms = cp1.header.last_block_timestamp_ms;
    h.accept(&cp2);
}

#[test]
fn check_4_rejects_time_in_the_future() {
    let (mut h, mut cp) = ready();
    let limit = (START + 60) * 1000;
    cp.header.last_block_timestamp_ms = limit + 1;
    rejects(&h, &cp, Error::TimeInFuture);
    // The bound itself is allowed.
    cp.header.last_block_timestamp_ms = limit;
    h.accept(&cp);
}

// --- 5. Batch -----------------------------------------------------------------------

#[test]
fn check_5_rejects_a_batch_that_does_not_match() {
    let (h, mut cp) = ready();
    let last = cp.batch.len() - 1;
    cp.batch[last] ^= 1;
    rejects(&h, &cp, Error::BadBatchHash);
}

#[test]
fn check_5_rejects_a_batch_over_96000_bytes() {
    let (h, mut cp) = ready();
    cp.batch = std::vec![7u8; MAX_BATCH_BYTES as usize + 1];
    cp.header.batch_hash = sha256(&cp.batch);
    rejects(&h, &cp, Error::BatchTooLarge);
}

// --- 6. Inbox -----------------------------------------------------------------------

#[test]
fn check_6_rejects_inbox_through_past_the_inbox() {
    mutated(|h| h.inbox_through = 2, Error::BadInboxThrough);
}

#[test]
fn check_6_rejects_inbox_through_going_backwards() {
    let (mut h, cp1) = ready();
    h.accept(&cp1);
    h.deposit(B, 10 * USDC);
    h.sync_inbox();
    h.lane.block();
    let mut cp2 = h.checkpoint();
    cp2.header.inbox_through = 0;
    cp2.header.inbox_acc = [0; 32];
    rejects(&h, &cp2, Error::BadInboxThrough);
}

#[test]
fn check_6_rejects_a_wrong_inbox_acc() {
    mutated(|h| h.inbox_acc = [9; 32], Error::BadInboxAcc);
}

#[test]
fn check_6_accepts_a_lane_behind_the_inbox() {
    let (mut h, cp) = ready();
    // A deposit the lane has not seen yet: inbox_through 1 of 2 is fine.
    h.deposit(B, 10 * USDC);
    h.accept(&cp);
    assert_eq!(
        (h.c().inbox_count(), h.c().last_checkpoint().inbox_through),
        (2, 1)
    );
}

// --- 7. Signatures ------------------------------------------------------------------

#[test]
fn check_7_rejects_an_epoch_that_does_not_exist() {
    let (h, cp) = ready();
    let sigs = h.sign(&cp.header, &[0, 2]);
    expect_err(h.try_submit(&cp, 0, &sigs), Error::BadEpoch);
    expect_err(h.try_submit(&cp, 2, &sigs), Error::BadEpoch);
}

#[test]
fn check_7_rejects_below_threshold() {
    let (h, cp) = ready();
    expect_err(
        h.try_submit(&cp, 1, &h.sign(&cp.header, &[1])),
        Error::BelowThreshold,
    );
    expect_err(
        h.try_submit(&cp, 1, &Vec::new(&h.env)),
        Error::BelowThreshold,
    );
}

#[test]
fn check_7_rejects_duplicate_or_unordered_indexes() {
    let (h, cp) = ready();
    expect_err(
        h.try_submit(&cp, 1, &h.sign(&cp.header, &[0, 0])),
        Error::BadSignerIndex,
    );
    expect_err(
        h.try_submit(&cp, 1, &h.sign(&cp.header, &[2, 0])),
        Error::BadSignerIndex,
    );
    let mut out_of_range = h.sign(&cp.header, &[0, 2]);
    let mut last = out_of_range.get(1).unwrap();
    last.signer_index = 3;
    out_of_range.set(1, last);
    expect_err(h.try_submit(&cp, 1, &out_of_range), Error::BadSignerIndex);
}

#[test]
fn check_7_bad_signature_traps() {
    let (h, cp) = ready();
    // Validator 0 signs another header.
    let mut other = cp.header;
    other.seq = 9;
    let mut sigs = h.sign(&cp.header, &[0, 2]);
    sigs.set(0, h.sign(&other, &[0]).get(0).unwrap());
    let r = h.try_submit(&cp, 1, &sigs);
    assert!(r.is_err(), "a bad signature must fail");
    assert_eq!(
        code_of(r),
        None,
        "ed25519_verify traps; it is not a contract error"
    );
    // A key outside the set, claiming index 1.
    let stranger = SigningKey::from_bytes(&[0x77; 32]);
    let mut sigs = h.sign(&cp.header, &[0]);
    sigs.push_back(
        sign_with(
            &h.env,
            &[stranger.clone(), stranger],
            &cp.header.encode(),
            &[1],
        )
        .get(0)
        .unwrap(),
    );
    let r = h.try_submit(&cp, 1, &sigs);
    assert!(r.is_err() && code_of(r).is_none());
    assert_eq!(h.c().last_checkpoint().seq, 0);
}

#[test]
fn check_7_accepts_any_two_of_three() {
    for which in [[0, 1], [0, 2], [1, 2]] {
        let (mut h, cp) = ready();
        let sigs = h.sign(&cp.header, &which);
        h.accept_with(&cp, 1, &sigs);
    }
    let (mut h, cp) = ready();
    let sigs = h.sign(&cp.header, &[0, 1, 2]);
    h.accept_with(&cp, 1, &sigs);
}

// --- 8. Solvency --------------------------------------------------------------------

#[test]
fn check_8_rejects_withdrawals_above_the_vault() {
    let mut h = Harness::new();
    h.deposit(A, 100 * USDC);
    h.sync_inbox();
    let a = caravel_harness::pk(A);
    let cp = h.crafted(std::vec![(a, 0)], std::vec![(a, 100 * USDC + 1)]);
    rejects(&h, &cp, Error::Insolvent);
    let cp = h.crafted(std::vec![(a, 0)], std::vec![(a, 100 * USDC)]);
    h.accept(&cp);
}

#[test]
fn check_8_excludes_deposits_the_lane_has_not_processed() {
    let mut h = Harness::new();
    h.deposit(A, 100 * USDC);
    h.sync_inbox();
    h.lane.block();
    // B's deposit is on Stellar but not in the lane: it cannot fund withdrawals.
    h.deposit(B, 50 * USDC);
    let a = caravel_harness::pk(A);
    let cp = h.crafted(std::vec![(a, 0)], std::vec![(a, 100 * USDC + 1)]);
    assert_eq!(cp.header.inbox_through, 1);
    rejects(&h, &cp, Error::Insolvent);
    let cp = h.crafted(std::vec![(a, 0)], std::vec![(a, 100 * USDC)]);
    h.accept(&cp);
}

#[test]
fn check_8_counts_withdrawals_not_yet_claimed() {
    let mut h = Harness::new();
    h.deposit(A, 100 * USDC);
    h.sync_inbox();
    let a = caravel_harness::pk(A);
    let cp = h.crafted(std::vec![(a, 40 * USDC)], std::vec![(a, 60 * USDC)]);
    h.accept(&cp);
    // 60 USDC is owed: only 40 USDC is left for new withdrawals, claimed or not.
    let cp2 = h.crafted(std::vec![(a, 0)], std::vec![(a, 40 * USDC + 1)]);
    rejects(&h, &cp2, Error::Insolvent);
    let who = h.user(A, 0);
    h.c().claim_withdrawal(
        &who,
        &key32(&h.env, A),
        &1,
        &0,
        &(60 * USDC),
        &h.withdrawal_proof(&cp, 0),
    );
    rejects(&h, &cp2, Error::Insolvent);
    let cp2 = h.crafted(std::vec![(a, 0)], std::vec![(a, 40 * USDC)]);
    h.accept(&cp2);
}

/// K-04: a checkpoint without withdrawals keeps no record. `LastCkpt` and
/// its `ckpt` event say what it was; the next checkpoint chains to it as
/// before, and one with withdrawals keeps its record for the claims.
#[test]
fn only_checkpoints_with_withdrawals_keep_a_record() {
    let mut h = Harness::new();
    h.deposit(A, 100 * USDC);
    h.sync_inbox();
    h.lane.block();
    h.lane.all_ok();
    let cp1 = h.checkpoint();
    assert_eq!(cp1.header.withdrawal_count, 0);
    h.accept(&cp1);
    let hash1 = BytesN::from_array(&h.env, &sha256(&cp1.header.encode()));
    // The event first: any later call starts a new event buffer.
    let event = CheckpointEvent {
        seq: 1,
        header_hash: hash1.clone(),
        last_block_height: cp1.header.last_block_height,
        withdrawals_total: 0,
    };
    assert_eq!(
        h.env
            .events()
            .all()
            .filter_by_contract(&h.id)
            .events()
            .last(),
        Some(&event.to_xdr(&h.env, &h.id))
    );
    assert_eq!(h.c().checkpoint(&1), None, "no record");
    assert!(!h.env.as_contract(&h.id, || h
        .env
        .storage()
        .persistent()
        .has(&DataKey::Ckpt(1))));
    assert_eq!(h.c().last_checkpoint().header_hash, hash1);
    // A claim against it finds nothing.
    let who = h.user(A, 0);
    let none =
        h.c()
            .try_claim_withdrawal(&who, &key32(&h.env, A), &1, &0, &USDC, &Vec::new(&h.env));
    assert!(none.is_err());

    // Checkpoint 2 carries a withdrawal: it chains to 1 and keeps a record.
    h.lane.withdraw(A, 30 * USDC);
    h.lane.block();
    h.lane.all_ok();
    let cp2 = h.checkpoint();
    assert_eq!(cp2.header.seq, 2);
    h.accept(&cp2);
    let record = h.c().checkpoint(&2).expect("stored");
    assert_eq!(
        (record.withdrawal_count, record.withdrawals_total),
        (1, 30 * USDC)
    );
    let proof = h.withdrawal_proof(&cp2, 0);
    h.c()
        .claim_withdrawal(&who, &key32(&h.env, A), &2, &0, &(30 * USDC), &proof);
    assert_eq!(h.token().balance(&who), 30 * USDC);
    assert_eq!(h.c().totals(), (30 * USDC, 30 * USDC));
}
