//! Freeze, escape and refunds (spec §13.6).

use soroban_sdk::testutils::Events as _;
use soroban_sdk::Event as _;

use super::*;

fn pk(seed: u8) -> [u8; 32] {
    caravel_harness::pk(seed)
}

/// A (100 USDC) and B (50 USDC) are in the lane. Checkpoint 1 says A has 120
/// and B 60 of escape equity (the lane lost money to them elsewhere) and owes
/// A a 30 USDC withdrawal. Then C deposits 20 USDC the lane never processes.
fn underwater() -> (Harness, Checkpoint) {
    let mut h = Harness::new();
    h.deposit(A, 100 * USDC);
    h.deposit(B, 50 * USDC);
    h.sync_inbox();
    let cp = h.crafted(
        std::vec![(pk(A), 120 * USDC), (pk(B), 60 * USDC)],
        std::vec![(pk(A), 30 * USDC)],
    );
    h.accept(&cp);
    h.deposit(C, 20 * USDC);
    (h, cp)
}

fn escape(
    h: &Harness,
    cp: &Checkpoint,
    seed: u8,
    index: u32,
    equity: i128,
) -> Result<(), soroban_sdk::Error> {
    let who = h.user(seed, 0);
    match h.c().try_escape_claim(
        &who,
        &key32(&h.env, seed),
        &index,
        &equity,
        &h.account_proof(cp, index),
    ) {
        Ok(_) => Ok(()),
        Err(Ok(e)) => Err(e),
        Err(Err(e)) => panic!("invoke error {e:?}"),
    }
}

#[test]
fn freeze_a_after_the_escape_timeout() {
    // Every inbox message is processed, so only the checkpoint timeout counts.
    let mut h = Harness::new();
    h.deposit(A, 100 * USDC);
    h.sync_inbox();
    h.lane.block();
    let cp = h.checkpoint();
    h.advance(300);
    h.accept(&cp);
    h.advance(PARAMS.escape_timeout_secs);
    expect_err(h.c().try_freeze(), Error::FreezeNotAllowed);
    h.advance(1);
    h.c().freeze();
    assert!(h.c().frozen());
    let info = h.c().frozen_info().unwrap();
    assert_eq!(info.at, START + 300 + PARAMS.escape_timeout_secs + 1);
    assert_eq!((info.payout_num, info.payout_den), (100 * USDC, 100 * USDC));
}

#[test]
fn freeze_a_on_a_lane_that_never_checkpointed() {
    let h = Harness::new();
    h.advance(PARAMS.escape_timeout_secs + 1);
    h.c().freeze();
    assert_eq!(
        h.c().frozen_info(),
        Some(FrozenInfo {
            at: START + PARAMS.escape_timeout_secs + 1,
            payout_num: 0,
            payout_den: 0
        })
    );
}

#[test]
fn freeze_b_with_an_overdue_inbox_message() {
    let (h, _) = underwater();
    // C's deposit (index 2) was enqueued at START; the last checkpoint too.
    h.advance(PARAMS.force_inclusion_window_secs);
    expect_err(h.c().try_freeze(), Error::FreezeNotAllowed);
    h.advance(1);
    const { assert!(PARAMS.force_inclusion_window_secs + 1 < PARAMS.escape_timeout_secs) };
    h.c().freeze();
    assert!(h.c().frozen());
}

#[test]
fn freeze_b_needs_an_unprocessed_message() {
    let mut h = Harness::new();
    h.deposit(A, 100 * USDC);
    h.sync_inbox();
    h.lane.block();
    let cp = h.checkpoint();
    h.accept(&cp);
    // Everything in the inbox was processed: waiting past the window is not enough.
    h.advance(PARAMS.force_inclusion_window_secs + 1);
    expect_err(h.c().try_freeze(), Error::FreezeNotAllowed);
}

#[test]
fn escape_pays_pro_rata() {
    let (h, cp) = underwater();
    let vault = 170 * USDC;
    assert_eq!(h.token().balance(&h.id), vault);
    h.advance(PARAMS.escape_timeout_secs + 1);
    h.c().freeze();
    // available = 170 − 30 owed − 20 unprocessed = 120; escape_total = 180.
    let event = FrozenEvent {
        payout_num: 120 * USDC,
        payout_den: 180 * USDC,
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
    let info = h.c().frozen_info().unwrap();
    assert_eq!((info.payout_num, info.payout_den), (120 * USDC, 180 * USDC));

    escape(&h, &cp, A, 0, 120 * USDC).unwrap();
    escape(&h, &cp, B, 1, 60 * USDC).unwrap();
    let event = EscapeClaimedEvent {
        lane_account: key32(&h.env, B),
        amount: 40 * USDC,
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
    assert_eq!(h.token().balance(&h.user(A, 0)), 80 * USDC);
    assert_eq!(h.token().balance(&h.user(B, 0)), 40 * USDC);
    assert!(h.c().escape_claimed(&key32(&h.env, A)));

    // Withdrawals committed before the freeze are still paid in full, and C gets its deposit back.
    let a = h.user(A, 0);
    h.c().claim_withdrawal(
        &a,
        &key32(&h.env, A),
        &1,
        &0,
        &(30 * USDC),
        &h.withdrawal_proof(&cp, 0),
    );
    h.c().refund_unprocessed_deposit(&2);
    assert_eq!(h.token().balance(&a), 110 * USDC);
    assert_eq!(h.token().balance(&h.user(C, 0)), 20 * USDC);
    assert_eq!(h.token().balance(&h.id), 0);
}

#[test]
fn escape_rounds_down() {
    let mut h = Harness::new();
    h.deposit(A, 10 * USDC);
    h.sync_inbox();
    // 3 accounts share 10 USDC: 1/3 each, floored.
    let cp = h.crafted(
        std::vec![(pk(A), 10 * USDC), (pk(B), 10 * USDC), (pk(C), 10 * USDC)],
        std::vec![],
    );
    h.accept(&cp);
    h.advance(PARAMS.escape_timeout_secs + 1);
    h.c().freeze();
    for (j, seed) in [A, B, C].into_iter().enumerate() {
        escape(&h, &cp, seed, j as u32, 10 * USDC).unwrap();
        assert_eq!(h.token().balance(&h.user(seed, 0)), 10 * USDC / 3);
    }
    assert_eq!(h.token().balance(&h.id), 1);
}

#[test]
fn escape_pays_nothing_when_the_vault_is_all_owed() {
    let mut h = Harness::new();
    h.deposit(A, 100 * USDC);
    h.sync_inbox();
    let cp = h.crafted(
        std::vec![(pk(A), 50 * USDC)],
        std::vec![(pk(A), 100 * USDC)],
    );
    h.accept(&cp);
    h.advance(PARAMS.escape_timeout_secs + 1);
    h.c().freeze();
    assert_eq!(h.c().frozen_info().unwrap().payout_num, 0);
    escape(&h, &cp, A, 0, 50 * USDC).unwrap();
    assert_eq!(h.token().balance(&h.user(A, 0)), 0);
    assert!(h.c().escape_claimed(&key32(&h.env, A)));
}

#[test]
fn escape_claim_rejections() {
    let (h, cp) = underwater();
    expect_err(
        h.c().try_escape_claim(
            &h.user(A, 0),
            &key32(&h.env, A),
            &0,
            &(120 * USDC),
            &h.account_proof(&cp, 0),
        ),
        Error::NotFrozen,
    );
    h.advance(PARAMS.escape_timeout_secs + 1);
    h.c().freeze();
    // Wrong recipient, wrong equity, wrong index.
    expect_err(
        h.c().try_escape_claim(
            &h.user(B, 0),
            &key32(&h.env, A),
            &0,
            &(120 * USDC),
            &h.account_proof(&cp, 0),
        ),
        Error::WrongRecipient,
    );
    assert_eq!(
        escape(&h, &cp, A, 0, 121 * USDC),
        Err(contract_error(Error::BadProof))
    );
    assert_eq!(
        escape(&h, &cp, A, 1, 120 * USDC),
        Err(contract_error(Error::BadProof))
    );
    escape(&h, &cp, A, 0, 120 * USDC).unwrap();
    assert_eq!(
        escape(&h, &cp, A, 0, 120 * USDC),
        Err(contract_error(Error::AlreadyClaimed))
    );
}

#[test]
fn escape_uses_the_last_checkpoint() {
    let (mut h, cp1) = underwater();
    h.sync_inbox();
    let cp2 = h.crafted(
        std::vec![(pk(A), 100 * USDC), (pk(B), 50 * USDC), (pk(C), 20 * USDC)],
        std::vec![],
    );
    h.accept(&cp2);
    h.advance(PARAMS.escape_timeout_secs + 1);
    h.c().freeze();
    // Checkpoint 1's leaves are stale.
    assert_eq!(
        escape(&h, &cp1, A, 0, 120 * USDC),
        Err(contract_error(Error::BadProof))
    );
    escape(&h, &cp2, C, 2, 20 * USDC).unwrap();
}

#[test]
fn refund_rules() {
    let (h, _) = underwater();
    expect_err(h.c().try_refund_unprocessed_deposit(&2), Error::NotFrozen);
    // A forced withdrawal after the checkpoint: not a deposit.
    let a = h.user(A, 0);
    h.c()
        .request_forced_withdrawal(&a, &key32(&h.env, A), &(5 * USDC));
    h.advance(PARAMS.escape_timeout_secs + 1);
    h.c().freeze();
    // Processed by the lane (index < inbox_through), a forced withdrawal, unknown.
    expect_err(
        h.c().try_refund_unprocessed_deposit(&0),
        Error::NotRefundable,
    );
    expect_err(
        h.c().try_refund_unprocessed_deposit(&3),
        Error::NotRefundable,
    );
    expect_err(
        h.c().try_refund_unprocessed_deposit(&4),
        Error::UnknownInboxMessage,
    );
    // Anyone may trigger it; the deposit goes back to its sender once.
    h.env.set_auths(&[]);
    h.c().refund_unprocessed_deposit(&2);
    assert_eq!(h.token().balance(&h.user(C, 0)), 20 * USDC);
    assert!(h.c().inbox(&2).unwrap().refunded);
    expect_err(
        h.c().try_refund_unprocessed_deposit(&2),
        Error::NotRefundable,
    );
}

#[test]
fn calls_after_a_freeze_are_rejected() {
    let (mut h, _) = underwater();
    h.sync_inbox();
    h.lane.block();
    let cp = h.checkpoint();
    h.advance(PARAMS.escape_timeout_secs + 1);
    h.c().freeze();
    let a = h.user(A, 10 * USDC);
    expect_err(
        h.c().try_deposit(&a, &(10 * USDC), &key32(&h.env, A)),
        Error::Frozen,
    );
    expect_err(
        h.c()
            .try_request_forced_withdrawal(&a, &key32(&h.env, A), &USDC),
        Error::Frozen,
    );
    let sigs = h.sign(&cp.header, &[0, 2]);
    expect_err(h.try_submit(&cp, 1, &sigs), Error::Frozen);
    expect_err(h.c().try_freeze(), Error::Frozen);
    // There is no unfreeze.
    assert!(h.c().frozen());
}
