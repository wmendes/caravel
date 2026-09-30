//! The SDK's standard pipeline on the test app (spec §20.4.3): accounts,
//! the app's kind, session keys, withdrawals, commitments and every fatal case.

mod common;

use caravel_app_sdk::crypto::native::{DiagnosticCrypto, NativeCrypto};
use caravel_app_sdk::testapp::{
    TestApp, COUNT, EVENT_BLOCK_COUNTED, EVENT_COUNTED, PERM_COUNT, ZERO_COUNT,
};
use caravel_app_sdk::{conserved, genesis, step, AccessMode, AppEngine, SdkState, BLOCK_LEVEL};
use caravel_core::block::Entry;
use caravel_core::codes::{fatal, receipt as codes};
use caravel_core::inbox::InboxKind;
use caravel_core::receipts::{event_type, DepositOutcome, PlatformEvent, PSEUDO_BLOCK_END};
use caravel_core::state::StateFrameV1;
use caravel_core::tx::{SigScheme, StandardBody};
use common::*;

const A: u8 = 0xA1;
const B: u8 = 0xB2;
const K: u8 = 0x5E; // a session key

fn session(lane: &Lane, owner: u8, key: u8, perms: u8) -> Entry {
    lane.owner(
        owner,
        StandardBody::AddSessionKey {
            session_key: pk(key),
            expires_at_ms: lane.now + 3_600_000,
            permissions: perms,
        },
    )
}

#[test]
fn genesis_state_is_the_layout_with_system_accounts() {
    let lane = Lane::new(config());
    let st = lane.st();
    assert_eq!(st.magic, TestApp::STATE_MAGIC);
    assert_eq!(st.config_hash, lane.config_hash);
    assert_eq!(st.accounts.len(), 2);
    assert!(st
        .accounts
        .iter()
        .all(|a| a.is_system() && a.balance == 0 && a.ext == 0u64.to_le_bytes()));
    assert_eq!(st.app_globals, 0u64.to_le_bytes());
    // The platform reads it as a state frame.
    let f = StateFrameV1::read(&lane.state).unwrap();
    assert_eq!(
        (f.magic, f.lane_id, f.height),
        (*b"CVSTTST1", config().lane_id, 0)
    );
    assert_eq!(st.encode().unwrap(), lane.state);
}

#[test]
fn genesis_refuses_bad_configs() {
    let refuse = |c: caravel_app_sdk::AppGenesisV1| {
        genesis::<TestApp>(&c.encode().unwrap(), &NativeCrypto)
            .map(|_| ())
            .unwrap_err()
            .code
    };
    let mut c = config();
    c.template = caravel_app_sdk::template_id("payments");
    assert_eq!(refuse(c), fatal::BAD_CONFIG);
    let mut c = config();
    c.template_version = 2;
    assert_eq!(refuse(c), fatal::BAD_CONFIG);
    let mut c = config();
    c.app_params = vec![1];
    assert_eq!(refuse(c), fatal::BAD_CONFIG, "the test app takes no params");
    let mut c = config();
    c.system_keys = vec![];
    assert_eq!(refuse(c), fatal::BAD_CONFIG);
    let mut c = config();
    c.system_keys = vec![pk(1), pk(1)];
    assert_eq!(refuse(c), fatal::BAD_CONFIG);
    let mut c = config();
    c.allowlist = vec![pk(3), pk(2)];
    assert_eq!(refuse(c), fatal::BAD_CONFIG);
    let mut c = config();
    c.max_accounts = 2;
    assert_eq!(refuse(c), fatal::BAD_CONFIG, "room for at least one user");
    let mut c = config();
    c.max_block_bytes = 48_001;
    assert_eq!(refuse(c), fatal::BAD_CONFIG);
    let mut c = config();
    c.min_withdrawal = 0;
    assert_eq!(refuse(c), fatal::BAD_CONFIG);
    let mut bytes = config().encode().unwrap();
    bytes.push(0);
    assert_eq!(
        genesis::<TestApp>(&bytes, &NativeCrypto).unwrap_err().code,
        fatal::BAD_CONFIG
    );
}

#[test]
fn deposits_create_accounts_and_the_app_counts() {
    let mut lane = Lane::new(config());
    let (da, db) = (lane.deposit(A, 100 * USDC), lane.deposit(B, 50 * USDC));
    let r = lane.run(vec![da, db], false).unwrap();
    assert_eq!(
        r.receipts[0].events[0].platform().unwrap().unwrap(),
        PlatformEvent::Deposit {
            key: pk(A),
            amount: 100 * USDC,
            outcome: DepositOutcome::Created
        }
    );
    let st = lane.st();
    assert_eq!(st.accounts.len(), 4);
    assert_eq!(
        st.accounts[2].next_nonce, T0,
        "a new account's nonce starts at the block time"
    );
    // COUNT by the owner, then zero (rejected, nonce consumed), then again.
    let n = lane.nonce(A);
    let r = lane
        .run(
            vec![
                lane.count(A, A, n, 5),
                lane.count(A, A, n + 1, 0),
                lane.count(A, A, n + 2, 2),
            ],
            false,
        )
        .unwrap();
    assert_eq!(
        r.receipts.iter().map(|x| x.code).collect::<Vec<_>>(),
        [0, ZERO_COUNT, 0, 0]
    );
    assert_eq!(r.receipts[0].events[0].type_id, EVENT_COUNTED);
    assert!(
        r.receipts[1].events.is_empty(),
        "a rejection keeps no events"
    );
    // The block-end pseudo entry carries the app's end_block event.
    let end = r.receipts.last().unwrap();
    assert_eq!(
        (end.entry_index, end.events[0].type_id),
        (PSEUDO_BLOCK_END, EVENT_BLOCK_COUNTED)
    );
    let st = lane.st();
    assert_eq!(st.accounts[2].ext, 7u64.to_le_bytes());
    assert_eq!(st.app_globals, 7u64.to_le_bytes());
    assert_eq!((st.app_word, st.app_flags), (1, 0));
    assert_eq!(st.accounts[2].next_nonce, n + 3);
    assert!(conserved(&st, 0));
}

#[test]
fn signer_rules_nonces_and_rate_limits() {
    let mut lane = Lane::new(config());
    let d = lane.deposit(A, 100 * USDC);
    lane.run(vec![d], false).unwrap();
    let n = lane.nonce(A);
    // A session key with PERM_COUNT counts; it can't withdraw or add keys, and can't use SEP-53.
    assert_eq!(lane.codes(vec![session(&lane, A, K, PERM_COUNT)]), [0]);
    let n = n + 1;
    let withdraw = StandardBody::Withdraw { amount: USDC };
    let by_key = lane.tx(
        A,
        K,
        SigScheme::RawEd25519,
        n,
        withdraw.kind(),
        withdraw.encode(),
    );
    let sep53_by_key = lane.tx(
        A,
        K,
        SigScheme::Sep53,
        n,
        COUNT,
        1u64.to_le_bytes().to_vec(),
    );
    assert_eq!(
        lane.codes(vec![
            lane.count(A, K, n, 1),
            Entry::User(by_key),
            Entry::User(sep53_by_key)
        ]),
        [0, codes::UNAUTHORIZED_SIGNER, codes::UNAUTHORIZED_SIGNER]
    );
    // A key without the permission, an unknown key, a stranger.
    assert_eq!(lane.codes(vec![session(&lane, A, 0x5F, 0)]), [0]);
    let n = lane.nonce(A);
    assert_eq!(
        lane.codes(vec![
            lane.count(A, 0x5F, n, 1),
            lane.count(A, 0x60, n, 1),
            lane.count(0x77, 0x77, 0, 1)
        ]),
        [
            codes::UNAUTHORIZED_SIGNER,
            codes::UNAUTHORIZED_SIGNER,
            codes::UNKNOWN_ACCOUNT
        ]
    );
    // The owner signs with SEP-53. A stale nonce and an expired tx don't consume it.
    let sep = lane.tx(
        A,
        A,
        SigScheme::Sep53,
        n,
        COUNT,
        1u64.to_le_bytes().to_vec(),
    );
    let mut expired = lane.tx(
        A,
        A,
        SigScheme::RawEd25519,
        n + 1,
        COUNT,
        1u64.to_le_bytes().to_vec(),
    );
    expired.expiry_ms = lane.now - 1;
    let expired = lane.tx_resigned(expired);
    assert_eq!(
        lane.codes(vec![
            Entry::User(sep),
            lane.count(A, A, n, 1),
            Entry::User(expired)
        ]),
        [0, codes::BAD_NONCE, codes::EXPIRED]
    );
    assert_eq!(lane.nonce(A), n + 1);
    // Rate limit: max_txs_per_account_per_block counts accepted-nonce txs.
    let mut c = config();
    c.max_txs_per_account_per_block = 2;
    let mut lane = Lane::new(c);
    let d = lane.deposit(A, 100 * USDC);
    lane.run(vec![d], false).unwrap();
    let n = lane.nonce(A);
    assert_eq!(
        lane.codes(vec![
            lane.count(A, A, n, 1),
            lane.count(A, A, n + 1, 1),
            lane.count(A, A, n + 2, 1)
        ]),
        [0, 0, codes::RATE_LIMITED]
    );
}

#[test]
fn session_key_rules() {
    let mut lane = Lane::new(config());
    let d = lane.deposit(A, 100 * USDC);
    lane.run(vec![d], false).unwrap();
    let add = |lane: &Lane, key: [u8; 32], exp: u64, perms: u8| {
        lane.owner(
            A,
            StandardBody::AddSessionKey {
                session_key: key,
                expires_at_ms: exp,
                permissions: perms,
            },
        )
    };
    let week = 7 * 24 * 3_600_000;
    // Each block's clock is 1 s later; the expiries are relative to it.
    for (key, exp, perms, want) in [
        (pk(A), 1, 1, codes::BAD_SESSION_KEY), // the owner itself
        (pk(K), 0, 1, codes::BAD_SESSION_KEY), // already expired
        (pk(K), week + 1, 1, codes::BAD_SESSION_KEY), // more than 7 days
        (pk(K), 1, 0x02, codes::BAD_SESSION_KEY), // not the app's bit
        (pk(K), week, 1, codes::OK),
    ] {
        let entry = add(&lane, key, lane.now + exp, perms);
        assert_eq!(lane.codes(vec![entry]), [want]);
    }
    assert_eq!(
        lane.codes(vec![add(&lane, pk(K), lane.now + 1, 1)]),
        [codes::BAD_SESSION_KEY],
        "already registered"
    );
    for s in 0..3u8 {
        assert_eq!(
            lane.codes(vec![add(&lane, pk(0x30 + s), lane.now + 1, 1)]),
            [codes::OK]
        );
    }
    assert_eq!(
        lane.codes(vec![add(&lane, pk(0x40), lane.now + 1, 1)]),
        [codes::TOO_MANY_SESSION_KEYS]
    );
    let revoke =
        |lane: &Lane, key| lane.owner(A, StandardBody::RevokeSessionKey { session_key: key });
    assert_eq!(lane.codes(vec![revoke(&lane, pk(K))]), [codes::OK]);
    assert_eq!(
        lane.codes(vec![revoke(&lane, pk(K))]),
        [codes::BAD_SESSION_KEY]
    );
    let keys: Vec<_> = lane.st().accounts[2]
        .session_keys
        .iter()
        .map(|k| k.key)
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "kept sorted by key");
}

#[test]
fn withdrawals_forced_withdrawals_and_the_commitment() {
    let mut c = config();
    c.max_pending_withdrawals = 3;
    let mut lane = Lane::new(c);
    let (da, db) = (lane.deposit(A, 100 * USDC), lane.deposit(B, 10 * USDC));
    lane.run(vec![da, db], false).unwrap();
    let w = |lane: &Lane, seed, amount| lane.owner(seed, StandardBody::Withdraw { amount });
    assert_eq!(
        lane.codes(vec![w(&lane, A, USDC - 1)]),
        [codes::BELOW_MIN_WITHDRAWAL]
    );
    assert_eq!(
        lane.codes(vec![w(&lane, A, 101 * USDC)]),
        [codes::INSUFFICIENT_FREE_BALANCE]
    );
    assert_eq!(lane.codes(vec![w(&lane, A, 30 * USDC)]), [codes::OK]);
    // A forced withdrawal takes at most the balance.
    let fw = lane.inbox(InboxKind::ForcedWithdrawal, B, 50 * USDC);
    let r = lane.run(vec![fw], false).unwrap();
    assert_eq!(
        r.receipts[0].events[0].platform().unwrap().unwrap(),
        PlatformEvent::ForcedWithdrawalProcessed {
            key: pk(B),
            amount: 10 * USDC
        }
    );
    // Unknown account: processed, no event.
    let fw = lane.inbox(InboxKind::ForcedWithdrawal, 0x77, USDC);
    assert!(lane.run(vec![fw], false).unwrap().receipts[0]
        .events
        .is_empty());
    assert_eq!(lane.codes(vec![w(&lane, A, 10 * USDC)]), [codes::OK]);
    assert_eq!(
        lane.codes(vec![w(&lane, A, 10 * USDC)]),
        [codes::WITHDRAWAL_QUEUE_FULL]
    );
    let st = lane.st();
    assert_eq!(st.pending.len(), 3);
    assert!(conserved(&st, 0));
    // The commitment empties the queue into the withdrawals root.
    let r = lane.run(vec![], true).unwrap();
    let end = r.receipts.last().unwrap();
    assert_eq!(end.entry_index, PSEUDO_BLOCK_END);
    assert_eq!(
        end.events
            .iter()
            .find(|e| e.type_id == event_type::COMMITMENT)
            .unwrap()
            .platform()
            .unwrap()
            .unwrap(),
        PlatformEvent::Commitment {
            seq: 1,
            withdrawals_total: 50 * USDC,
            escape_total: 60 * USDC
        }
    );
    let st = lane.st();
    assert!(st.pending.is_empty());
    assert_eq!(
        (st.checkpoint_seq, st.withdrawals_committed_total),
        (1, 50 * USDC)
    );
    let c = st.last_commitment;
    assert_eq!(
        (c.seq, c.account_count, c.withdrawal_count, c.escape_total),
        (1, 4, 3, 60 * USDC)
    );
    assert_eq!(
        (c.inbox_through, c.inbox_acc),
        (st.inbox_through, st.inbox_acc)
    );
    // The leaves the platform rebuilds give the same root.
    let leaves: Vec<[u8; 32]> = st
        .accounts
        .iter()
        .enumerate()
        .map(|(j, a)| {
            sha256(&caravel_core::preimage::account_leaf_preimage(
                &st.lane_id,
                1,
                j as u32,
                &a.key,
                a.balance,
            ))
        })
        .collect();
    assert_eq!(
        caravel_core::merkle::root(&caravel_core::merkle::NativeSha256, &leaves).unwrap(),
        c.accounts_root
    );
    assert!(conserved(&st, 0));
    // Lane liquidity: nothing may leave beyond what the lane holds.
    let mut lane = Lane::new(config());
    let d = lane.deposit(A, 5 * USDC);
    lane.run(vec![d], false).unwrap();
    assert_eq!(lane.codes(vec![w(&lane, A, 5 * USDC)]), [codes::OK]);
    assert!(conserved(&lane.st(), 0));
}

#[test]
fn deposits_reuse_empty_slots_and_bounce_otherwise() {
    let mut c = config();
    c.max_accounts = 3; // two system accounts and one user
    let mut lane = Lane::new(c);
    let d = lane.deposit(A, 5 * USDC);
    lane.run(vec![d], false).unwrap();
    // B has no slot: bounced into the pending queue.
    let d = lane.deposit(B, 2 * USDC);
    let r = lane.run(vec![d], false).unwrap();
    assert_eq!(
        r.receipts[0].events[0].platform().unwrap().unwrap(),
        PlatformEvent::Deposit {
            key: pk(B),
            amount: 2 * USDC,
            outcome: DepositOutcome::Bounced
        }
    );
    // A empties its account; after the commitment its slot is free for B.
    let wd = lane.owner(A, StandardBody::Withdraw { amount: 5 * USDC });
    lane.run(vec![wd], true).unwrap();
    let d = lane.deposit(B, 3 * USDC);
    let r = lane.run(vec![d], false).unwrap();
    assert_eq!(
        r.receipts[0].events[0].platform().unwrap().unwrap(),
        PlatformEvent::Deposit {
            key: pk(B),
            amount: 3 * USDC,
            outcome: DepositOutcome::Created
        }
    );
    let st = lane.st();
    assert_eq!((st.accounts.len(), st.accounts[2].key), (3, pk(B)));
    assert!(conserved(&st, 0));
    // An allowlist lane bounces strangers.
    let mut c = config();
    c.access_mode = AccessMode::Allowlist;
    c.allowlist = vec![pk(A)];
    let mut lane = Lane::new(c);
    let (da, db) = (lane.deposit(A, USDC), lane.deposit(B, USDC));
    let r = lane.run(vec![da, db], false).unwrap();
    let outcome = |i: usize| match r.receipts[i].events[0].platform().unwrap().unwrap() {
        PlatformEvent::Deposit { outcome, .. } => outcome,
        _ => unreachable!(),
    };
    assert_eq!(
        (outcome(0), outcome(1)),
        (DepositOutcome::Created, DepositOutcome::Bounced)
    );
}

#[test]
fn fatal_blocks() {
    let mut lane = Lane::new(config());
    let d = lane.deposit(A, 100 * USDC);
    lane.run(vec![d], false).unwrap();
    let n = lane.nonce(A);
    let fatal_of = |lane: &Lane, entries: Vec<Entry>| {
        let bytes = lane.block_bytes(entries, false);
        step::<TestApp, _>(&lane.state, &bytes, &DiagnosticCrypto).unwrap_err()
    };
    // A bad signature, with the entry it's in.
    let mut bad = lane.tx(
        A,
        A,
        SigScheme::RawEd25519,
        n,
        COUNT,
        1u64.to_le_bytes().to_vec(),
    );
    bad.signature[3] ^= 1;
    let e = fatal_of(&lane, vec![lane.count(A, A, n, 1), Entry::User(bad)]);
    assert_eq!((e.code, e.entry_index), (fatal::BAD_SIGNATURE, 1));
    // Unknown kinds, reserved kinds and wrong body lengths: bad encoding.
    for (kind, body) in [
        (17u8, vec![0u8; 8]),
        (7, vec![]),
        (COUNT, vec![0u8; 7]),
        (4, vec![0u8; 15]),
    ] {
        let tx = lane.tx(A, A, SigScheme::RawEd25519, n, kind, body);
        let e = fatal_of(&lane, vec![Entry::User(tx)]);
        assert_eq!(
            (e.code, e.entry_index),
            (fatal::BAD_ENTRY_ENCODING, 0),
            "kind {kind}"
        );
    }
    // No feeds in this app; entry order; an inbox gap.
    assert_eq!(
        fatal_of(&lane, vec![Entry::Feed(vec![1, 2, 3])]).code,
        fatal::UNKNOWN_FEED_KEY
    );
    let mut l2 = Lane::new(config());
    let d = l2.deposit(A, USDC);
    let e = fatal_of(&l2, vec![l2.count(A, A, 0, 1), d]);
    assert_eq!((e.code, e.entry_index), (fatal::ENTRY_ORDER, 1));
    l2.inbox_n = 5;
    let gap = l2.deposit(A, USDC);
    assert_eq!(fatal_of(&l2, vec![gap]).code, fatal::INBOX_GAP);
    // Block-level: time, height, previous hash, size.
    lane.now -= 10_000;
    let e = fatal_of(&lane, vec![]);
    assert_eq!(
        (e.code, e.entry_index),
        (fatal::TIME_REGRESSION, BLOCK_LEVEL)
    );
    lane.now += 20_000;
    let mut bytes = lane.block_bytes(vec![], false);
    bytes[40] ^= 1; // height
    assert_eq!(
        step::<TestApp, _>(&lane.state, &bytes, &NativeCrypto)
            .unwrap_err()
            .code,
        fatal::BAD_HEIGHT
    );
    let mut bytes = lane.block_bytes(vec![], false);
    bytes[60] ^= 1; // prev_block_hash
    assert_eq!(
        step::<TestApp, _>(&lane.state, &bytes, &NativeCrypto)
            .unwrap_err()
            .code,
        fatal::BAD_PREV_HASH
    );
    let big: Vec<Entry> = (0..200).map(|i| lane.count(A, A, n + i, 1)).collect();
    assert_eq!(fatal_of(&lane, big).code, fatal::BLOCK_TOO_LARGE);
    // Another app's state is not this app's.
    let mut other = lane.state.clone();
    other[..8].copy_from_slice(b"CVSTPAY1");
    let bytes = lane.block_bytes(vec![], false);
    assert_eq!(
        step::<TestApp, _>(&other, &bytes, &NativeCrypto)
            .unwrap_err()
            .code,
        fatal::BAD_STATE_ENCODING
    );
}

#[test]
fn state_decoding_is_strict() {
    let lane = Lane::new(config());
    let mut extra = lane.state.clone();
    extra.push(0);
    assert!(SdkState::decode(&extra, &TestApp::STATE_MAGIC).is_err());
    assert!(SdkState::decode(&lane.state[..lane.state.len() - 1], &TestApp::STATE_MAGIC).is_err());
}

mod props {
    use super::*;
    use proptest::prelude::*;

    #[derive(Clone, Debug)]
    enum Op {
        Deposit(u8, i128),
        Forced(u8, i128),
        Withdraw(u8, i128),
        Count(u8, u64),
        Checkpoint,
    }

    fn op() -> impl Strategy<Value = Op> {
        let who = 0u8..4;
        prop_oneof![
            (who.clone(), 1i128..200).prop_map(|(w, x)| Op::Deposit(w, x * USDC)),
            (who.clone(), 1i128..200).prop_map(|(w, x)| Op::Forced(w, x * USDC)),
            (who.clone(), 1i128..200).prop_map(|(w, x)| Op::Withdraw(w, x * USDC)),
            (who, 0u64..5).prop_map(|(w, x)| Op::Count(w, x)),
            Just(Op::Checkpoint),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        /// SDK-INV1 and a canonical state after every block of a random workload.
        #[test]
        fn conservation_holds(ops in proptest::collection::vec(op(), 1..40)) {
            let mut lane = Lane::new(config());
            for op in ops {
                let seed = |w: u8| 0xA0 + w;
                // The sequencer never lets the pending queue overflow (spec §14.2).
                let op = if lane.st().pending.len() >= 15 { Op::Checkpoint } else { op };
                let (entries, cp) = match op {
                    Op::Deposit(w, x) => (vec![lane.deposit(seed(w), x)], false),
                    Op::Forced(w, x) => (vec![lane.inbox(InboxKind::ForcedWithdrawal, seed(w), x)], false),
                    Op::Withdraw(w, x) if lane.st().accounts.iter().any(|a| a.key == pk(seed(w))) =>
                        (vec![lane.owner(seed(w), StandardBody::Withdraw { amount: x })], false),
                    Op::Count(w, x) if lane.st().accounts.iter().any(|a| a.key == pk(seed(w))) =>
                        (vec![lane.count(seed(w), seed(w), lane.nonce(seed(w)), x)], false),
                    Op::Checkpoint => (vec![], true),
                    _ => (vec![], false),
                };
                lane.run(entries, cp).unwrap();
                let st = lane.st();
                prop_assert!(conserved(&st, 0));
                prop_assert_eq!(st.encode().unwrap(), lane.state.clone());
            }
        }
    }
}
