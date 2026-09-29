//! The sequencer core (spec §14.1–§14.3) on a fake clock: block building,
//! checkpoints every 10 blocks, persistence and restart, quarantine of fatal
//! entries, the inbox chain check, pre-validation and withdrawal proofs.

mod common;

use caravel_lane::builder::{self, BuildInput};
use caravel_lane::checkpoint::{
    self, network_id, settlement_addr_hash, sha256, BatchBudget, HeaderIds,
};
use caravel_lane::mempool::{Mempool, Reject};
use caravel_lane::sequencer::{self, Core, Executor, InboxReport, Incident, SequencerConfig};
use caravel_lane::store::{CheckpointStatus, Store};
use caravel_testkit::lane::{
    config, seeds, BTC, BTC_PRICE, ETH, ETH_PRICE, T0, TICK, USDC, XLM, XLM_PRICE,
};
use caravel_testkit::Lane;
use caravel_types::batch::BatchV1;
use caravel_types::block::{BlockInputV1, Entry};
use caravel_types::checkpoint::CheckpointHeaderV1;
use caravel_types::fatal;
use caravel_types::inbox::{inbox_acc_preimage, InboxKind, InboxMsgV1};
use caravel_types::state::StateV1;
use caravel_types::tx::{LaneTxV1, PlaceOrder, Side, SigScheme, Tif, TxBody};
use caravel_types::vectors::pk;

fn ids() -> HeaderIds {
    HeaderIds {
        network_id: network_id("Test SDF Network ; September 2015"),
        settlement_addr_hash: settlement_addr_hash(&[7; 32]),
        engine_wasm_hash: common::engine_hash(),
    }
}

fn seq_config() -> SequencerConfig {
    SequencerConfig {
        ids: ids(),
        checkpoint_every_blocks: 10,
        max_batch_bytes: 96_000,
        mempool_max: 10_000,
        mempool_max_per_account: 256,
    }
}

/// A core plus a testkit lane used only to sign transactions and oracle updates.
struct T {
    core: Core,
    signer: Lane,
    now: u64,
    inbox_n: u64,
    inbox_acc: [u8; 32],
}

fn genesis() -> (Vec<u8>, [u8; 32]) {
    let bytes = config().encode().unwrap();
    let state = caravel_perps::genesis(&bytes, &caravel_perps::native::NativeCrypto).unwrap();
    (state, sha256(&bytes))
}

fn store_in_memory() -> Store {
    let (state, config_hash) = genesis();
    Store::open_in_memory(&config().lane_id, &config_hash, &state).unwrap()
}

impl T {
    fn with(exec: Executor, store: Store) -> Self {
        let (_, config_hash) = genesis();
        let core = Core::open(exec, store, config_hash, seq_config()).unwrap();
        let inbox_n = core.inbox_reported();
        let inbox_acc = match inbox_n {
            0 => [0; 32],
            n => core.store().inbox_acc(n - 1).unwrap().unwrap(),
        };
        let now = core.state().last_timestamp_ms.max(T0) + 1000;
        Self {
            core,
            signer: Lane::native(config()),
            now,
            inbox_n,
            inbox_acc,
        }
    }

    fn native() -> Self {
        Self::with(Executor::Native, store_in_memory())
    }

    fn inbox(&mut self, kind: InboxKind, seed: u8, amount: i128) {
        let msg = InboxMsgV1 {
            kind,
            index: self.inbox_n,
            lane_account: pk(seed),
            amount,
            enqueued_at: self.now / 1000,
        };
        let acc = sha256(&inbox_acc_preimage(&self.inbox_acc, &msg.encode()));
        assert_eq!(
            self.core.report_inbox(msg, acc).unwrap(),
            InboxReport::Added
        );
        self.inbox_n += 1;
        self.inbox_acc = acc;
    }

    fn deposit(&mut self, seed: u8, amount: i128) {
        self.inbox(InboxKind::Deposit, seed, amount);
    }

    fn prices(&mut self) {
        for (m, p) in [(BTC, BTC_PRICE), (ETH, ETH_PRICE), (XLM, XLM_PRICE)] {
            let u = self.signer.oracle_update(seeds::ORACLE, m, p, self.now);
            assert!(self.core.report_oracle(u).unwrap());
        }
    }

    fn next_nonce(&self, seed: u8) -> u64 {
        let base = self
            .core
            .state()
            .accounts
            .iter()
            .find(|a| a.key == pk(seed))
            .map_or(0, |a| a.next_nonce);
        base + self
            .core
            .mempool
            .iter()
            .filter(|q| q.tx.account == pk(seed))
            .count() as u64
    }

    fn signed(&self, seed: u8, nonce: u64, body: TxBody) -> LaneTxV1 {
        self.signer.signed(
            seed,
            seed,
            SigScheme::RawEd25519,
            nonce,
            self.now + 60_000,
            body,
        )
    }

    fn tx(&mut self, seed: u8, body: TxBody) -> [u8; 32] {
        let tx = self.signed(seed, self.next_nonce(seed), body);
        self.core.submit_tx(&tx.encode(), self.now).unwrap()
    }

    fn order(&mut self, seed: u8, market_id: u16, side: Side, price: i64, lots: i64) -> [u8; 32] {
        self.tx(
            seed,
            TxBody::PlaceOrder(PlaceOrder {
                market_id,
                side,
                tif: Tif::Gtc,
                reduce_only: false,
                price,
                lots,
                client_order_id: 0,
            }),
        )
    }

    fn block(&mut self) -> sequencer::Produced {
        let p = self.core.produce_block(self.now).unwrap();
        self.now += 1000;
        p
    }
}

/// Deposits, prices and a cross, then blocks up to `height`.
fn busy_lane(t: &mut T, height: u64) {
    t.deposit(seeds::A, 10_000 * USDC);
    t.deposit(seeds::B, 10_000 * USDC);
    t.prices();
    t.block();
    t.order(seeds::A, BTC, Side::Buy, BTC_PRICE, 10);
    t.order(seeds::B, BTC, Side::Sell, BTC_PRICE, 4);
    t.order(seeds::B, ETH, Side::Sell, ETH_PRICE + 10 * TICK, 3);
    t.tx(seeds::A, TxBody::Withdraw { amount: 100 * USDC });
    while t.core.height() < height {
        if t.core.height().is_multiple_of(3) {
            t.prices();
        }
        t.block();
    }
}

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
            let st = StateV1::decode(&state).unwrap();
            let (_, header) =
                checkpoint::assemble(&ids(), prev_header, &batch, &st, &state).unwrap();
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
fn a_different_lane_database_is_refused() {
    let (state, _) = genesis();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lane.sqlite");
    Store::open(&path, &config().lane_id, &[1; 32], &state).unwrap();
    assert!(matches!(
        Store::open(&path, &config().lane_id, &[2; 32], &state),
        Err(caravel_lane::store::StoreError::WrongLane)
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
    let accounts = checkpoint::account_leaves(&header, &StateV1::decode(&snap).unwrap()).unwrap();
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
    t.core.mempool.push([0xBA; 32], bad).unwrap();
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
    let oracle = Default::default();
    let mut mempool = Mempool::new(100, 100);
    for q in t.core.mempool.iter() {
        mempool.push(q.hash, q.tx).unwrap();
    }
    let build = |byte_budget: usize, user_cap: usize| {
        builder::build(&BuildInput {
            state: &st,
            prev_block_hash: [0; 32],
            now_ms: t.now,
            inbox: &[],
            oracle: &oracle,
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
