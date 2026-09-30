//! Payments scenarios P1–P12 and the INV-PAY1 property (spec §20.4.4),
//! natively on the harness lane. `parity.rs` replays them through the Wasm.
//! The harness checks SDK-INV1 after every block; for payments, which holds
//! nothing of its own, that is INV-PAY1.

mod common;

use caravel_app_sdk::{AppEngine, SdkState};
use caravel_core::block::Entry;
use caravel_core::codes::{fatal, receipt as platform};
use caravel_core::receipts::{DepositOutcome, EventV1, PlatformEvent, PSEUDO_BLOCK_START};
use caravel_core::tx::{kind, SigScheme, StandardBody};
use caravel_harness::{pk, Lane, USDC};
use caravel_payments::{codes, Params, Payments, TransferEvent, PERM_TRANSFER, TRANSFER};
use common::*;
use proptest::prelude::*;

/// The codes of the last block's entries (pseudo entries left out).
fn codes_of(l: &Lane<Payments>) -> Vec<u16> {
    l.receipts
        .receipts
        .iter()
        .filter(|r| r.entry_index < PSEUDO_BLOCK_START)
        .map(|r| r.code)
        .collect()
}

fn events_of(l: &Lane<Payments>) -> Vec<EventV1> {
    l.receipts
        .receipts
        .iter()
        .flat_map(|r| r.events.clone())
        .collect()
}

fn transfers_of(l: &Lane<Payments>) -> Vec<TransferEvent> {
    events_of(l)
        .iter()
        .filter_map(TransferEvent::decode)
        .collect()
}

fn platform_events(l: &Lane<Payments>) -> Vec<PlatformEvent> {
    events_of(l)
        .iter()
        .filter_map(|e| e.platform())
        .map(|e| e.unwrap())
        .collect()
}

fn index_of(l: &Lane<Payments>, seed: u8) -> Option<usize> {
    l.state().accounts.iter().position(|a| a.key == pk(seed))
}

/// A lane with A = 100 USDC and B = 50 USDC.
fn funded(fee: i128) -> Lane<Payments> {
    let mut l = lane(fee);
    l.deposit(A, 100 * USDC);
    l.deposit(B, 50 * USDC);
    l.block();
    l.all_ok();
    l
}

fn session_key(l: &mut Lane<Payments>, owner: u8, key: u8, permissions: u8) {
    let expires_at_ms = l.now + 3_600_000;
    l.standard(
        owner,
        StandardBody::AddSessionKey {
            session_key: pk(key),
            expires_at_ms,
            permissions,
        },
    );
}

#[test]
fn p1_deposits_create_accounts_after_the_treasury() {
    let l = funded(FEE);
    let st = l.state();
    assert_eq!(st.accounts.len(), 3);
    assert_eq!(st.accounts[0].key, pk(T));
    assert!(st.accounts[0].is_system());
    assert_eq!((index_of(&l, A), index_of(&l, B)), (Some(1), Some(2)));
    assert_eq!(
        (l.balance(T), l.balance(A), l.balance(B)),
        (0, 100 * USDC, 50 * USDC)
    );
    let created = |seed| PlatformEvent::Deposit {
        key: pk(seed),
        amount: 0,
        outcome: DepositOutcome::Created,
    };
    let outcomes: Vec<_> = platform_events(&l)
        .into_iter()
        .map(|e| match e {
            PlatformEvent::Deposit { key, outcome, .. } => PlatformEvent::Deposit {
                key,
                amount: 0,
                outcome,
            },
            e => e,
        })
        .collect();
    assert_eq!(outcomes, vec![created(A), created(B)]);
    // A second deposit credits the existing account.
    let mut l = l;
    l.deposit(A, 5 * USDC);
    l.block();
    assert!(matches!(
        platform_events(&l)[..],
        [PlatformEvent::Deposit { outcome: DepositOutcome::Credited, amount, .. }] if amount == 5 * USDC
    ));
    assert_eq!(l.balance(A), 105 * USDC);
    check_wasm(&l);
}

#[test]
fn p2_a_transfer_pays_its_fee_to_the_treasury() {
    let mut l = funded(FEE);
    transfer(&mut l, A, A, B, 10 * USDC);
    l.block();
    l.all_ok();
    assert_eq!(l.balance(A), 90 * USDC - FEE);
    assert_eq!(l.balance(B), 60 * USDC);
    assert_eq!(l.balance(T), FEE);
    assert_eq!(
        transfers_of(&l),
        vec![TransferEvent {
            from_idx: 1,
            to_idx: 2,
            amount: 10 * USDC,
            fee: FEE,
            memo: 7
        }]
    );
    // Many transfers in one block, both ways.
    for _ in 0..5 {
        transfer(&mut l, A, A, B, USDC);
        transfer(&mut l, B, B, A, 2 * USDC);
    }
    l.block();
    l.all_ok();
    assert_eq!(l.balance(A), 95 * USDC - 6 * FEE);
    assert_eq!(l.balance(B), 55 * USDC - 5 * FEE);
    assert_eq!(l.balance(T), 11 * FEE);
    // The treasury is an account like any other: it can receive a transfer.
    transfer(&mut l, A, A, T, USDC);
    l.block();
    l.all_ok();
    assert_eq!(l.balance(T), USDC + 12 * FEE);
    check_wasm(&l);
}

#[test]
fn p3_the_balance_must_cover_amount_and_fee() {
    let mut l = funded(FEE);
    let nonce = l.state().accounts[1].next_nonce;
    transfer(&mut l, A, A, B, 100 * USDC);
    transfer(&mut l, A, A, B, 100 * USDC - FEE + 1);
    // amount + fee overflows i128: a rejection, not a fatal.
    transfer(&mut l, A, A, B, i128::MAX);
    transfer(&mut l, A, A, B, 100 * USDC - FEE);
    l.block();
    assert_eq!(
        codes_of(&l),
        vec![
            codes::INSUFFICIENT_BALANCE,
            codes::INSUFFICIENT_BALANCE,
            codes::INSUFFICIENT_BALANCE,
            0
        ]
    );
    assert_eq!(
        (l.balance(A), l.balance(B), l.balance(T)),
        (0, 150 * USDC - FEE, FEE)
    );
    // Rejections drop their events and still consume the nonce.
    assert_eq!(transfers_of(&l).len(), 1);
    assert_eq!(l.state().accounts[1].next_nonce, nonce + 4);
    transfer(&mut l, A, A, B, 1);
    l.block();
    assert_eq!(codes_of(&l), vec![codes::INSUFFICIENT_BALANCE]);
    check_wasm(&l);
}

#[test]
fn p4_the_recipient_must_have_an_account() {
    let mut l = funded(FEE);
    transfer(&mut l, A, A, C, USDC);
    l.block();
    assert_eq!(codes_of(&l), vec![codes::UNKNOWN_RECIPIENT]);
    assert_eq!(index_of(&l, C), None);
    assert_eq!(l.balance(A), 100 * USDC);
    // Once C deposits, the same transfer goes through.
    l.deposit(C, USDC);
    transfer(&mut l, A, A, C, USDC);
    l.block();
    l.all_ok();
    assert_eq!(l.balance(C), 2 * USDC);
    check_wasm(&l);
}

#[test]
fn p5_no_transfer_to_self() {
    let mut l = funded(FEE);
    transfer(&mut l, A, A, A, USDC);
    l.block();
    assert_eq!(codes_of(&l), vec![codes::SELF_TRANSFER]);
    assert_eq!((l.balance(A), l.balance(T)), (100 * USDC, 0));
    check_wasm(&l);
}

#[test]
fn p6_amounts_below_min_transfer() {
    let mut l = funded(FEE);
    for amount in [0, -1, i128::MIN] {
        transfer(&mut l, A, A, B, amount);
    }
    transfer(&mut l, A, A, B, 1);
    l.block();
    assert_eq!(
        codes_of(&l),
        vec![
            codes::BELOW_MIN_TRANSFER,
            codes::BELOW_MIN_TRANSFER,
            codes::BELOW_MIN_TRANSFER,
            0
        ]
    );
    assert_eq!(l.balance(B), 50 * USDC + 1);
    check_wasm(&l);
    // A lane with min_transfer = 1 USDC.
    let mut c = config(FEE);
    c.app_params = Params {
        transfer_fee: FEE,
        min_transfer: USDC,
    }
    .encode();
    let mut l = Lane::<Payments>::new(c);
    l.deposit(A, 10 * USDC);
    l.deposit(B, 10 * USDC);
    l.block();
    transfer(&mut l, A, A, B, USDC - 1);
    transfer(&mut l, A, A, B, USDC);
    l.block();
    assert_eq!(codes_of(&l), vec![codes::BELOW_MIN_TRANSFER, 0]);
    check_wasm(&l);
}

#[test]
fn p7_session_keys_transfer_with_perm_transfer_only() {
    let mut l = funded(FEE);
    const K0: u8 = 0x5F;
    session_key(&mut l, A, K, PERM_TRANSFER);
    session_key(&mut l, A, K0, 0);
    // A bit payments does not define.
    session_key(&mut l, A, 0x60, 0x02);
    l.block();
    assert_eq!(codes_of(&l), vec![0, 0, platform::BAD_SESSION_KEY]);

    transfer(&mut l, K, A, B, USDC);
    transfer(&mut l, K0, A, B, USDC);
    // A session key never signs a standard kind, nor with SEP-53.
    l.tx_as(
        K,
        A,
        SigScheme::RawEd25519,
        kind::WITHDRAW,
        StandardBody::Withdraw { amount: USDC }.encode(),
    );
    let body = caravel_payments::Transfer {
        to: pk(B),
        amount: USDC,
        memo: 0,
    }
    .encode();
    l.tx_as(K, A, SigScheme::Sep53, TRANSFER, body);
    // Nor for another account.
    transfer(&mut l, K, B, A, USDC);
    l.block();
    assert_eq!(
        codes_of(&l),
        vec![
            0,
            platform::UNAUTHORIZED_SIGNER,
            platform::UNAUTHORIZED_SIGNER,
            platform::UNAUTHORIZED_SIGNER,
            platform::UNAUTHORIZED_SIGNER,
        ]
    );
    assert_eq!(l.balance(A), 99 * USDC - FEE);
    assert_eq!(l.balance(B), 51 * USDC);

    // Revoked, then expired.
    l.standard(A, StandardBody::RevokeSessionKey { session_key: pk(K) });
    l.block();
    l.all_ok();
    transfer(&mut l, K, A, B, USDC);
    l.block();
    assert_eq!(codes_of(&l), vec![platform::UNAUTHORIZED_SIGNER]);
    session_key(&mut l, A, K, PERM_TRANSFER);
    l.block();
    l.all_ok();
    l.advance(3_600_000);
    transfer(&mut l, K, A, B, USDC);
    l.block();
    assert_eq!(codes_of(&l), vec![platform::UNAUTHORIZED_SIGNER]);
    check_wasm(&l);
}

#[test]
fn p8_a_zero_fee_lane() {
    let mut l = funded(0);
    transfer(&mut l, A, A, B, 100 * USDC);
    l.block();
    l.all_ok();
    assert_eq!(
        (l.balance(A), l.balance(B), l.balance(T)),
        (0, 150 * USDC, 0)
    );
    assert_eq!(transfers_of(&l)[0].fee, 0);
    check_wasm(&l);
}

#[test]
fn p9_withdraw_and_the_commitment() {
    let mut l = funded(FEE);
    transfer(&mut l, A, A, B, 10 * USDC);
    l.withdraw(A, 20 * USDC);
    l.withdraw(B, USDC - 1);
    l.withdraw(B, 61 * USDC);
    l.block();
    assert_eq!(
        codes_of(&l),
        vec![
            0,
            0,
            platform::BELOW_MIN_WITHDRAWAL,
            platform::INSUFFICIENT_FREE_BALANCE
        ]
    );
    assert_eq!(l.pending(), vec![(pk(A), 20 * USDC)]);
    assert_eq!(l.balance(A), 70 * USDC - FEE);

    l.checkpoint();
    let st = l.state();
    assert!(st.pending.is_empty());
    assert_eq!(st.withdrawals_committed_total, 20 * USDC);
    assert_eq!(st.last_commitment.withdrawals_total, 20 * USDC);
    assert_eq!(st.last_commitment.withdrawal_count, 1);
    // Escape equity is the balance; the leaves sum to escape_total.
    let leaves = l.escape_leaves();
    assert_eq!(
        leaves,
        vec![(pk(T), FEE), (pk(A), 70 * USDC - FEE), (pk(B), 60 * USDC)]
    );
    let total: i128 = leaves.iter().map(|(_, e)| e).sum();
    assert_eq!(st.last_commitment.escape_total, total);
    assert_eq!(
        total + st.withdrawals_committed_total,
        st.deposits_credited_total
    );
    assert!(platform_events(&l).contains(&PlatformEvent::Commitment {
        seq: 1,
        withdrawals_total: 20 * USDC,
        escape_total: total,
    }));
    check_wasm(&l);
}

#[test]
fn p10_forced_withdrawals_are_capped_by_the_balance() {
    let mut l = funded(FEE);
    l.forced_withdrawal(A, 1_000 * USDC);
    l.forced_withdrawal(B, 5 * USDC);
    // No account: processed as a no-op.
    l.forced_withdrawal(C, USDC);
    l.block();
    l.all_ok();
    assert_eq!(
        platform_events(&l),
        vec![
            PlatformEvent::ForcedWithdrawalProcessed {
                key: pk(A),
                amount: 100 * USDC
            },
            PlatformEvent::ForcedWithdrawalProcessed {
                key: pk(B),
                amount: 5 * USDC
            },
        ]
    );
    assert_eq!(l.pending(), vec![(pk(A), 100 * USDC), (pk(B), 5 * USDC)]);
    assert_eq!((l.balance(A), l.balance(B)), (0, 45 * USDC));
    // A drained account can't pay a transfer; a second forced withdrawal queues nothing.
    transfer(&mut l, A, A, B, 1);
    l.forced_withdrawal(A, USDC);
    l.block();
    assert_eq!(codes_of(&l), vec![0, codes::INSUFFICIENT_BALANCE]);
    assert_eq!(
        platform_events(&l),
        vec![PlatformEvent::ForcedWithdrawalProcessed {
            key: pk(A),
            amount: 0
        }]
    );
    check_wasm(&l);
}

#[test]
fn p11_an_empty_slot_is_reused_and_its_old_key_is_unknown() {
    let mut c = config(FEE);
    c.max_accounts = 3; // the treasury, A and B
    let mut l = Lane::<Payments>::new(c);
    l.deposit(A, 10 * USDC);
    l.deposit(B, 10 * USDC);
    l.block();
    // Full, and nothing empty: C's deposit bounces.
    l.deposit(C, 2 * USDC);
    transfer(&mut l, A, A, B, 10 * USDC - FEE);
    l.block();
    assert_eq!(codes_of(&l), vec![0, 0]);
    assert_eq!(l.pending(), vec![(pk(C), 2 * USDC)]);
    assert_eq!(l.balance(A), 0);
    // A is empty now, so C's next deposit takes A's slot.
    l.deposit(C, 3 * USDC);
    l.block();
    assert_eq!(index_of(&l, C), Some(1));
    assert_eq!(index_of(&l, A), None);
    assert_eq!(l.balance(C), 3 * USDC);
    // A's key is gone: it neither sends nor receives.
    transfer(&mut l, B, B, A, USDC);
    transfer(&mut l, A, A, B, USDC);
    transfer(&mut l, B, B, C, USDC);
    l.block();
    assert_eq!(
        codes_of(&l),
        vec![codes::UNKNOWN_RECIPIENT, platform::UNKNOWN_ACCOUNT, 0]
    );
    assert_eq!(l.balance(C), 4 * USDC);
    // A slot with a pending withdrawal is not empty.
    l.withdraw(C, 4 * USDC);
    l.block();
    l.all_ok();
    l.deposit(A, USDC);
    l.block();
    assert_eq!(index_of(&l, A), None);
    assert_eq!(
        l.pending(),
        vec![(pk(C), 2 * USDC), (pk(C), 4 * USDC), (pk(A), USDC)]
    );
    check_wasm(&l);
}

#[test]
fn p12_fatal_blocks_and_bad_genesis() {
    let mut l = funded(FEE);
    let before = l.state_hash();
    // A body one byte short, an unknown kind, a reserved platform kind, an app kind payments lacks.
    l.tx_as(A, A, SigScheme::RawEd25519, TRANSFER, vec![0; 55]);
    let bytes = l.build(false).encode().unwrap();
    assert_eq!(fatal_both(&mut l, &bytes), fatal::BAD_ENTRY_ENCODING);
    for k in [17, 7, 1] {
        l.tx_as(A, A, SigScheme::RawEd25519, k, vec![0; 56]);
        let bytes = l.build(false).encode().unwrap();
        assert_eq!(
            (k, fatal_both(&mut l, &bytes)),
            (k, fatal::BAD_ENTRY_ENCODING)
        );
    }
    // Payments has no feed.
    let mut b = l.build(false);
    b.entries.push(Entry::Feed(vec![1, 2, 3]));
    assert_eq!(
        fatal_both(&mut l, &b.encode().unwrap()),
        fatal::UNKNOWN_FEED_KEY
    );
    assert_eq!(l.state_hash(), before);
    check_wasm(&l);

    // Genesis refuses bad app params and a foreign template.
    for params in [
        Params {
            transfer_fee: -1,
            min_transfer: 1,
        }
        .encode(),
        Params {
            transfer_fee: 0,
            min_transfer: 0,
        }
        .encode(),
        Params {
            transfer_fee: FEE,
            min_transfer: 1,
        }
        .encode()[..31]
            .to_vec(),
        [
            Params {
                transfer_fee: FEE,
                min_transfer: 1,
            }
            .encode(),
            vec![0],
        ]
        .concat(),
        vec![],
    ] {
        let mut c = config(FEE);
        c.app_params = params;
        assert_eq!(genesis_both(&c), Some(fatal::BAD_CONFIG));
    }
    let mut c = config(FEE);
    c.template = caravel_app_sdk::template_id("testapp");
    assert_eq!(genesis_both(&c), Some(fatal::BAD_CONFIG));
    let mut c = config(FEE);
    c.template_version = 2;
    assert_eq!(genesis_both(&c), Some(fatal::BAD_CONFIG));
    assert_eq!(genesis_both(&config(0)), None);
}

// --- INV-PAY1 -------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Op {
    Deposit(u8, i128),
    Transfer {
        signer: u8,
        from: u8,
        to: u8,
        amount: i128,
    },
    Withdraw(u8, i128),
    Forced(u8, i128),
    SessionKey(u8),
    Advance,
    Checkpoint,
}

const USERS: [u8; 4] = [A, B, C, 0x44];

fn user() -> impl Strategy<Value = u8> {
    prop::sample::select(USERS.to_vec())
}

fn amount() -> impl Strategy<Value = i128> {
    prop_oneof![
        (1i128..=200).prop_map(|x| x * USDC / 4),
        Just(FEE),
        Just(0),
        Just(-5),
        Just(i128::MAX),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (user(), (1i128..=100).prop_map(|x| x * USDC)).prop_map(|(u, a)| Op::Deposit(u, a)),
        8 => (prop_oneof![5 => user(), 1 => Just(K)], user(), prop_oneof![8 => user(), 1 => Just(T)], amount())
            .prop_map(|(signer, from, to, amount)| Op::Transfer { signer, from, to, amount }),
        2 => (user(), amount()).prop_map(|(u, a)| Op::Withdraw(u, a)),
        1 => (user(), amount()).prop_map(|(u, a)| Op::Forced(u, a)),
        1 => user().prop_map(Op::SessionKey),
        1 => Just(Op::Advance),
        2 => Just(Op::Checkpoint),
    ]
}

/// Σ balances + Σ pending + committed == credited, and the treasury holds exactly the fees.
fn inv_pay1(st: &SdkState) -> bool {
    let balances: i128 = st.accounts.iter().map(|a| a.balance).sum();
    let pending: i128 = st.pending.iter().map(|p| p.amount).sum();
    balances + pending + st.withdrawals_committed_total == st.deposits_credited_total
        && st.accounts.iter().all(|a| a.balance >= 0)
}

/// Runs `ops` as blocks of up to six (a checkpoint ends one), checking
/// INV-PAY1 after each. Returns the lane and what the treasury must hold: the
/// fees plus what users sent it (its key never signs here).
fn run_ops(fee: i128, max_accounts: u32, ops: &[Op]) -> (Lane<Payments>, i128) {
    let mut c = config(fee);
    c.max_accounts = max_accounts;
    let mut l = Lane::<Payments>::new(c);
    let mut treasury = 0i128;
    let received = |l: &Lane<Payments>| -> i128 {
        transfers_of(l)
            .iter()
            .map(|t| t.fee + if t.to_idx == 0 { t.amount } else { 0 })
            .sum()
    };
    let mut queued = 0;
    for op in ops {
        match *op {
            Op::Deposit(u, a) => {
                l.deposit(u, a);
            }
            Op::Transfer {
                signer,
                from,
                to,
                amount,
            } => transfer(&mut l, signer, from, to, amount),
            Op::Withdraw(u, a) => {
                l.withdraw(u, a);
            }
            Op::Forced(u, a) => {
                l.forced_withdrawal(u, a.max(0));
            }
            Op::SessionKey(u) => session_key(&mut l, u, K, PERM_TRANSFER),
            Op::Advance => l.advance(600_000),
            Op::Checkpoint => {}
        }
        queued += 1;
        if matches!(op, Op::Checkpoint) || queued == 6 {
            if matches!(op, Op::Checkpoint) {
                l.checkpoint();
            } else {
                l.block();
            }
            queued = 0;
            treasury += received(&l);
            let st = l.state();
            assert!(inv_pay1(&st), "INV-PAY1 broken at block {}", l.height());
            assert_eq!(st.accounts[0].balance, treasury);
            // Escape equity is the balance.
            for (j, (key, eq)) in l.escape_leaves().into_iter().enumerate() {
                assert_eq!((key, eq), (st.accounts[j].key, st.accounts[j].balance));
            }
        }
    }
    l.checkpoint();
    treasury += received(&l);
    (l, treasury)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, ..ProptestConfig::default() })]

    #[test]
    fn inv_pay1_holds(ops in prop::collection::vec(op(), 1..80), fee in prop_oneof![Just(0), Just(FEE)], small in any::<bool>()) {
        let (l, treasury) = run_ops(fee, if small { 3 } else { 16 }, &ops);
        let st = l.state();
        prop_assert!(inv_pay1(&st));
        prop_assert!(st.pending.is_empty());
        prop_assert_eq!(st.last_commitment.escape_total, st.accounts.iter().map(|a| a.balance).sum::<i128>());
        prop_assert_eq!(st.accounts[0].balance, treasury);
    }
}

#[test]
fn the_state_round_trips_with_the_payments_magic() {
    let l = funded(FEE);
    let st = l.state();
    assert_eq!(st.encode().unwrap(), l.state_bytes);
    assert_eq!(&l.state_bytes[..8], &Payments::STATE_MAGIC);
}
