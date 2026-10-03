//! The sequencer core (spec §14.1–§14.3) on a fake clock: block building,
//! checkpoints every 10 blocks, persistence and restart, quarantine of fatal
//! entries, the inbox chain check, pre-validation and withdrawal proofs.

mod common;

use common::harness::*;

use caravel_core::batch::BatchV1;
use caravel_core::block::{BlockInputV1, Entry};
use caravel_core::checkpoint::CheckpointHeaderV1;
use caravel_core::inbox::{InboxKind, InboxMsgV1};
use caravel_core::state::StateFrameV1;
use caravel_core::tx::TxEnvelopeV1;
use caravel_perps_node::{app, PerpsApp};
use caravel_runtime::builder::{self, BuildInput};
use caravel_runtime::checkpoint::{self, settlement_addr_hash, sha256, BatchBudget};
use caravel_runtime::mempool::{Mempool, Reject};
use caravel_runtime::sequencer::{self, Executor, InboxReport, Incident};
use caravel_runtime::store::{CheckpointStatus, Store};
use caravel_testkit::lane::{config, seeds, BTC, BTC_PRICE, TICK, USDC};
use caravel_types::fatal;
use caravel_types::state::StateV1;
use caravel_types::tx::{LaneTxV1, PlaceOrder, Side, SigScheme, Tif, TxBody};
use caravel_types::vectors::pk;

/// Re-executes every stored block natively from genesis, checks each
/// `state_hash_after`, and rebuilds every checkpoint header (what a validator does).
fn replay(store: &Store, height: u64) -> ([u8; 32], Vec<CheckpointHeaderV1>) {
    let (mut state, _) = genesis();
    let mut prev_header = [0u8; 32];
    let mut batch = Vec::new();
    let mut headers = Vec::new();
    for h in 1..=height {
        let (record, _) = store.block(h).unwrap().unwrap();
        let out = caravel_perps::step(&state, &record.input, &caravel_perps::native::NativeCrypto)
            .unwrap();
        assert_eq!(sha256(&out.state), record.state_hash_after, "block {h}");
        state = out.state;
        batch.push(record.clone());
        if BlockInputV1::decode(&record.input).unwrap().checkpoint_end {
            let (_, header) = checkpoint::assemble(&ids(), prev_header, &batch, &state).unwrap();
            prev_header = sha256(&header.encode());
            headers.push(header);
            batch.clear();
        }
    }
    (sha256(&state), headers)
}

#[test]
fn checkpoints_every_10_blocks_and_replay_matches() {
    let mut t = T::native();
    busy_lane(&mut t, 25);
    let store = t.core.store();
    for (seq, first, last) in [(1, 1, 10), (2, 11, 20)] {
        let row = store.checkpoint(seq).unwrap().expect("sealed");
        assert_eq!(
            (row.first_height, row.last_height, row.status),
            (first, last, CheckpointStatus::Sequenced)
        );
        let header = CheckpointHeaderV1::decode(&row.header).unwrap();
        assert_eq!(header.batch_hash, sha256(&row.batch));
        assert_eq!(
            BatchV1::decode(&row.batch).unwrap().blocks.len() as u64,
            last - first + 1
        );
        assert!(t.core.precheck_row(&row).is_ok());
    }
    assert!(store.checkpoint(3).unwrap().is_none());
    let (hash, headers) = replay(store, 25);
    assert_eq!(hash, t.core.state_hash());
    for h in &headers {
        assert_eq!(
            h.encode().to_vec(),
            store.checkpoint(h.seq).unwrap().unwrap().header
        );
    }
    assert_eq!(headers[1].prev_header_hash, sha256(&headers[0].encode()));
    // The cross filled and the withdrawal is a leaf of checkpoint 1.
    let st = t.core.state();
    let a = st.accounts.iter().find(|x| x.key == pk(seeds::A)).unwrap();
    assert_eq!(a.positions[0].lots, 4);
    assert_eq!(headers[0].withdrawal_count, 1);
}

#[test]
fn wasm_core_matches_native_core() {
    let mut native = T::native();
    let mut wasm = T::with(Executor::Wasm(common::wasm()), store_in_memory());
    busy_lane(&mut native, 12);
    busy_lane(&mut wasm, 12);
    assert_eq!(native.core.state_hash(), wasm.core.state_hash());
    assert_eq!(
        native.core.store().checkpoint(1).unwrap().unwrap().header,
        wasm.core.store().checkpoint(1).unwrap().unwrap().header
    );
}

#[test]
fn restart_resumes_from_sqlite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lane.sqlite");
    let (state, config_hash) = genesis();
    let open = || Store::open(&path, &config().lane_id, &config_hash, &state).unwrap();

    let mut t = T::with(Executor::Native, open());
    busy_lane(&mut t, 13);
    let (hash, height, now) = (t.core.state_hash(), t.core.height(), t.now);
    drop(t);

    let mut t = T::with(Executor::Native, open());
    assert_eq!((t.core.state_hash(), t.core.height()), (hash, height));
    t.now = now;
    while t.core.height() < 20 {
        t.block();
    }
    let row = t
        .core
        .store()
        .checkpoint(2)
        .unwrap()
        .expect("sealed after restart");
    assert_eq!((row.first_height, row.last_height), (11, 20));
    let (replayed, headers) = replay(t.core.store(), 20);
    assert_eq!(replayed, t.core.state_hash());
    assert_eq!(headers[1].encode().to_vec(), row.header);
    // Inbox reports survive the restart: the next index continues the chain.
    t.deposit(seeds::C, 50 * USDC);
    t.block();
    assert!(t
        .core
        .state()
        .accounts
        .iter()
        .any(|a| a.key == pk(seeds::C)));
}

#[test]
fn oracle_updates_need_a_configured_key_and_a_valid_signature() {
    let mut t = T::native();
    let good = t.signer.oracle_update(seeds::ORACLE, BTC, BTC_PRICE, t.now);
    let mut forged = good;
    forged.signature[0] ^= 1;
    let stranger = t.signer.oracle_update(seeds::A, BTC, BTC_PRICE, t.now);
    assert!(!t.core.report_feed(&forged.encode()).unwrap());
    assert!(!t.core.report_feed(&stranger.encode()).unwrap());
    assert!(t.core.report_feed(&good.encode()).unwrap());
    // Bytes that are not an oracle update are refused too.
    assert!(!t.core.report_feed(&[1, 2, 3]).unwrap());
}

#[test]
fn a_rotation_sends_old_epoch_signatures_back_for_signing() {
    let mut t = T::native();
    busy_lane(&mut t, 25);
    let store = t.core.store_mut();
    store.set_signed(1, 1, "[]").unwrap();
    store.set_accepted(1, "tx1", 7).unwrap();
    store.set_signed(2, 1, "[\"old\"]").unwrap();
    // The same epoch changes nothing.
    assert!(store.unsign_before_epoch(1).unwrap().is_empty());
    // Epoch 2: the signed-but-not-accepted checkpoint waits for signatures
    // again, keeping the old ones for reuse; the accepted one stays accepted.
    assert_eq!(store.unsign_before_epoch(2).unwrap(), vec![2]);
    let row = store.checkpoint(2).unwrap().unwrap();
    assert_eq!(
        (row.status, row.epoch, row.sigs.as_deref()),
        (CheckpointStatus::Sequenced, None, Some("[\"old\"]"))
    );
    assert_eq!(
        store.checkpoint(1).unwrap().unwrap().status,
        CheckpointStatus::Accepted
    );
    store.set_signed(2, 2, "[\"new\"]").unwrap();
    assert!(store.unsign_before_epoch(2).unwrap().is_empty());
    assert_eq!(store.checkpoint(2).unwrap().unwrap().epoch, Some(2));
}

#[test]
fn a_different_lane_database_is_refused() {
    let (state, _) = genesis();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lane.sqlite");
    Store::open(&path, &config().lane_id, &[1; 32], &state).unwrap();
    assert!(matches!(
        Store::open(&path, &config().lane_id, &[2; 32], &state),
        Err(caravel_runtime::store::StoreError::WrongLane)
    ));
}

#[test]
fn withdrawal_proofs_verify_against_the_header() {
    let mut t = T::native();
    busy_lane(&mut t, 10);
    let row = t.core.store().checkpoint(1).unwrap().unwrap();
    let header = CheckpointHeaderV1::decode(&row.header).unwrap();
    let leaves = sequencer::parse_leaves(&row.withdrawals).unwrap();
    assert_eq!(leaves.len(), 1);
    assert_eq!(
        (leaves[0].key, leaves[0].amount),
        (pk(seeds::A), 100 * USDC)
    );
    let hashes = checkpoint::withdrawal_hashes(&header, &leaves);
    let proof = checkpoint::proof(&hashes, 0).unwrap();
    assert!(caravel_merkle::verify(
        &caravel_merkle::NativeSha256,
        &hashes[0],
        0,
        header.withdrawal_count,
        &proof,
        &header.withdrawals_root
    ));
    // Escape leaves from the snapshot give the header's accounts root.
    let (_, snap) = t.core.store().snapshot(1).unwrap().unwrap();
    let accounts = app::account_leaves(&header, &StateV1::decode(&snap).unwrap()).unwrap();
    assert_eq!(accounts.len() as u32, header.account_count);
}

#[test]
fn a_fatal_entry_is_quarantined_and_the_block_rebuilt() {
    let mut t = T::native();
    t.deposit(seeds::A, 1_000 * USDC);
    t.deposit(seeds::B, 1_000 * USDC);
    t.prices();
    t.block();
    // Skip pre-validation: a transaction with a broken signature.
    let mut bad = t.signed(
        seeds::A,
        t.next_nonce(seeds::A),
        TxBody::CancelAll { market_id: BTC },
    );
    bad.signature[5] ^= 1;
    t.core
        .mempool
        .push([0xBA; 32], TxEnvelopeV1::decode(&bad.encode()).unwrap())
        .unwrap();
    let good = t.order(seeds::B, BTC, Side::Buy, BTC_PRICE - 10 * TICK, 1);
    let height = t.core.height() + 1;
    let p = t.block();
    assert_eq!(p.height, height);
    assert!(
        p.incidents.iter().any(|i| matches!(i, Incident::Quarantined { code, entry_index: 0, .. } if *code == fatal::BAD_SIGNATURE)),
        "{:?}",
        p.incidents
    );
    let input = BlockInputV1::decode(&p.record.input).unwrap();
    assert_eq!(input.entries.len(), 1);
    assert!(!t.core.mempool.contains(&[0xBA; 32]) && !t.core.mempool.contains(&good));
}

#[test]
fn the_inbox_chain_is_checked() {
    let mut t = T::native();
    t.deposit(seeds::A, 100 * USDC);
    // A gap, then a message whose acc_after is not our fold.
    let msg = InboxMsgV1 {
        kind: InboxKind::Deposit,
        index: 5,
        lane_account: pk(seeds::B),
        amount: USDC,
        enqueued_at: 1,
    };
    assert_eq!(
        t.core.report_inbox(msg, [0; 32]).unwrap(),
        InboxReport::Gap { expected: 1 }
    );
    // Re-reporting a known message is fine.
    let first = t.core.store().inbox_from(0, 1).unwrap()[0];
    assert_eq!(
        t.core.report_inbox(first.0, first.1).unwrap(),
        InboxReport::AlreadyKnown
    );
    let msg = InboxMsgV1 { index: 1, ..msg };
    assert_eq!(
        t.core.report_inbox(msg, [9; 32]).unwrap(),
        InboxReport::Mismatch
    );
    assert!(t.core.inbox_halted());
    // The lane stops taking inbox messages, even the good one it already had.
    let p = t.block();
    assert!(BlockInputV1::decode(&p.record.input)
        .unwrap()
        .entries
        .iter()
        .all(|e| !matches!(e, Entry::Inbox(_))));
}

#[test]
fn prevalidation_rejects_what_the_engine_would_not_take() {
    let mut t = T::native();
    t.deposit(seeds::A, 100 * USDC);
    t.block();
    let body = TxBody::CancelAll { market_id: BTC };
    let n = t.next_nonce(seeds::A);
    let ok = t.signed(seeds::A, n, body);
    let submit = |t: &mut T, tx: &LaneTxV1| t.core.submit_tx(&tx.encode(), t.now);
    let mut bad_sig = ok;
    bad_sig.signature[0] ^= 1;
    assert_eq!(submit(&mut t, &bad_sig), Err(Reject::BadSignature));
    let wrong_lane = t.signer.signed(
        seeds::A,
        seeds::A,
        SigScheme::RawEd25519,
        n,
        t.now + 60_000,
        body,
    );
    let mut wl = wrong_lane;
    wl.lane_id = [1; 32];
    assert_eq!(submit(&mut t, &wl), Err(Reject::WrongLane));
    let expired = t.signer.signed(
        seeds::A,
        seeds::A,
        SigScheme::RawEd25519,
        n,
        t.now - 1,
        body,
    );
    assert_eq!(submit(&mut t, &expired), Err(Reject::Expired));
    let stranger = t.signer.signed(
        seeds::E,
        seeds::E,
        SigScheme::RawEd25519,
        0,
        t.now + 60_000,
        body,
    );
    assert_eq!(submit(&mut t, &stranger), Err(Reject::UnknownAccount));
    assert_eq!(t.core.submit_tx(&[1, 2, 3], t.now), Err(Reject::Decode));
    // SEP-53 signatures are checked the same way as raw ones.
    let sep53 = t.signer.signed(
        seeds::A,
        seeds::A,
        SigScheme::Sep53,
        n,
        t.now + 60_000,
        body,
    );
    submit(&mut t, &sep53).unwrap();
    assert_eq!(submit(&mut t, &sep53), Err(Reject::Duplicate));
    // Another transaction with a queued nonce could never run (DEC-085).
    assert_eq!(submit(&mut t, &ok), Err(Reject::NonceQueued));
    t.block();
    assert_eq!(submit(&mut t, &ok), Err(Reject::StaleNonce));
}

#[test]
fn nonces_out_of_order_wait_for_their_turn() {
    let mut t = T::native();
    t.deposit(seeds::A, 1_000 * USDC);
    t.prices();
    t.block();
    let body = |p| {
        TxBody::PlaceOrder(PlaceOrder {
            market_id: BTC,
            side: Side::Buy,
            tif: Tif::Gtc,
            reduce_only: false,
            price: p,
            lots: 1,
            client_order_id: 0,
        })
    };
    let n = t.next_nonce(seeds::A);
    let second = t.signed(seeds::A, n + 1, body(BTC_PRICE - 20 * TICK));
    let first = t.signed(seeds::A, n, body(BTC_PRICE - 10 * TICK));
    t.core.submit_tx(&second.encode(), t.now).unwrap();
    t.core.submit_tx(&first.encode(), t.now).unwrap();
    let p = t.block();
    assert_eq!(
        BlockInputV1::decode(&p.record.input).unwrap().entries.len(),
        1
    );
    assert_eq!(t.core.mempool.len(), 1);
    t.block();
    assert!(t.core.mempool.is_empty());
    assert_eq!(
        t.core
            .state()
            .accounts
            .iter()
            .find(|a| a.key == pk(seeds::A))
            .unwrap()
            .next_nonce,
        n + 2
    );
}

#[test]
fn builder_respects_byte_and_entry_budgets() {
    let t = {
        let mut t = T::native();
        t.deposit(seeds::A, 10_000 * USDC);
        t.prices();
        t.block();
        for i in 0..40 {
            t.order(seeds::A, BTC, Side::Buy, BTC_PRICE - (10 + i) * TICK, 1);
        }
        t
    };
    let st = t.core.state();
    let frame = StateFrameV1::read(t.core.state_bytes()).unwrap();
    let feeds = Default::default();
    let mut mempool = Mempool::new(100, 100);
    for q in t.core.mempool.iter() {
        mempool.push(q.hash, q.tx.clone()).unwrap();
    }
    let build = |byte_budget: usize, user_cap: usize| {
        builder::build(&BuildInput {
            app: &PerpsApp,
            state: &st,
            frame: &frame,
            prev_block_hash: [0; 32],
            now_ms: t.now,
            inbox: &[],
            feeds: &feeds,
            mempool: &mempool,
            byte_budget,
            user_cap,
            include_inbox: true,
        })
    };
    let full = build(12_000, 256);
    assert_eq!(full.block.entries.len(), 40);
    let small = build(2_000, 256);
    assert!(small.encoded_len <= 2_000 && small.block.encode().unwrap().len() == small.encoded_len);
    assert!(small.block.entries.len() < 40 && !small.block.entries.is_empty());
    assert_eq!(build(12_000, 7).block.entries.len(), 7);
}

#[test]
fn checkpoint_policy() {
    let budget = BatchBudget::new(96_000, 12_000);
    // (a) the 10th block of a batch.
    let mut b = budget;
    for _ in 0..9 {
        b.add(100);
    }
    assert!(builder::checkpoint_end(&b, 100, 10, 0, 512));
    assert!(!builder::checkpoint_end(&budget, 100, 10, 0, 512));
    // (b) the next block might not fit.
    let mut b = budget;
    for _ in 0..7 {
        b.add(11_940);
    }
    assert!(builder::checkpoint_end(&b, 100, 100, 0, 512));
    // (c) half the pending queue is used.
    assert!(builder::checkpoint_end(&budget, 100, 100, 256, 512));
    assert!(!builder::checkpoint_end(&budget, 100, 100, 255, 512));
}

#[test]
fn batch_budget_matches_the_encoded_batch() {
    let mut t = T::native();
    busy_lane(&mut t, 10);
    let row = t.core.store().checkpoint(1).unwrap().unwrap();
    let mut budget = BatchBudget::new(96_000, 12_000);
    for (record, _) in t.core.store().blocks(1, 10).unwrap() {
        budget.add(record.input.len());
    }
    assert_eq!(budget.used(), row.batch.len());
}

#[test]
fn settlement_addr_hash_is_the_contract_address_xdr() {
    use soroban_env_host::xdr::{ContractId, Hash, Limits, ScAddress, ScVal, WriteXdr};
    let id = [0x5A; 32];
    let xdr = ScVal::Address(ScAddress::Contract(ContractId(Hash(id))))
        .to_xdr(Limits::none())
        .unwrap();
    assert_eq!(settlement_addr_hash(&id), sha256(&xdr));
}
