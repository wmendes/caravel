//! Constructor, inbox, golden vector, recipient encoding, TTL, upgrade and
//! the full-batch budget.

use caravel_types::inbox::inbox_acc_preimage;
use soroban_sdk::testutils::storage::{Instance as _, Persistent as _};
use soroban_sdk::testutils::{Events as _, Ledger as _};
use soroban_sdk::{Event as _, IntoVal, Val};

use super::*;
use crate::ACCOUNT_XDR_PREFIX;

const DAY: u32 = 17_280;
const TARGET: u32 = 120 * DAY;
const THRESHOLD: u32 = 30 * DAY;

fn hex(s: &str) -> StdVec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn bytes_n<const N: usize>(env: &Env, s: &str) -> BytesN<N> {
    BytesN::from_array(env, &hex(s).try_into().unwrap())
}

// --- Golden vector and recipient encoding --------------------------------------------

#[test]
fn golden_header_vector_verifies_in_the_contract() {
    let file: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../lanes/perps/engine/test-vectors/checkpoint_header.json"
    ))
    .unwrap();
    let v = &file["vectors"][0];
    let bytes = hex(v["hex"].as_str().unwrap());
    assert_eq!(bytes.len(), 442);
    assert_eq!(sha256(&bytes).to_vec(), hex(v["hash"].as_str().unwrap()));
    let header = CheckpointHeaderV1::decode(&bytes).unwrap();
    assert_eq!(header.seq, 1);
    let threshold: u32 = file["context"]["signers_threshold"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let h = Harness::new();
    let env = &h.env;
    let mut signers = Vec::new(env);
    let mut sigs = Vec::new(env);
    for s in v["fields"]["signatures"].as_array().unwrap() {
        signers.push_back(WeightedSigner {
            key: bytes_n(env, s["key"].as_str().unwrap()),
            weight: 1,
        });
        sigs.push_back(Sig {
            signer_index: s["signer_index"].as_str().unwrap().parse().unwrap(),
            signature: bytes_n(env, s["signature"].as_str().unwrap()),
        });
    }
    let set = WeightedSigners { signers, threshold };
    let msg = BytesN::from_array(env, &sha256(&bytes));
    env.as_contract(&h.id, || {
        crate::signers::validate(&set).unwrap();
        crate::signers::verify(env, &set, &msg, &sigs).unwrap();
        let mut two = Vec::new(env);
        two.push_back(sigs.get(0).unwrap());
        two.push_back(sigs.get(2).unwrap());
        crate::signers::verify(env, &set, &msg, &two).unwrap();
        let mut one = Vec::new(env);
        one.push_back(sigs.get(1).unwrap());
        assert_eq!(
            crate::signers::verify(env, &set, &msg, &one),
            Err(Error::BelowThreshold)
        );
    });
}

#[test]
fn account_xdr_prefix() {
    let env = Env::default();
    for key in [[0u8; 32], [0xFF; 32], caravel_types::vectors::pk(A)] {
        let xdr = account_address(&env, &key).to_xdr(&env).to_alloc_vec();
        assert_eq!(xdr.len(), 44);
        assert_eq!(xdr[..12], ACCOUNT_XDR_PREFIX);
        assert_eq!(xdr[12..], key);
    }
    let contract = Address::generate(&env).to_xdr(&env).to_alloc_vec();
    assert_ne!(contract[..12], ACCOUNT_XDR_PREFIX);
}

// --- Constructor ----------------------------------------------------------------------

fn construct(f: impl FnOnce(&Env) -> (WeightedSigners, Params)) {
    let env = Env::default();
    let (signers, params) = f(&env);
    let a = Address::generate(&env);
    let z = BytesN::from_array(&env, &[0; 32]);
    env.register(
        Settlement,
        (
            a.clone(),
            a,
            z.clone(),
            z.clone(),
            z.clone(),
            z,
            signers,
            params,
        ),
    );
}

fn three(env: &Env) -> WeightedSigners {
    let keys: StdVec<SigningKey> = (0..3).map(validator).collect();
    signer_set(env, &keys, 2).0
}

#[test]
fn constructor_accepts_the_demo_setup() {
    let h = Harness::new();
    let cfg = h.c().config();
    assert_eq!(
        (cfg.admin, cfg.usdc, cfg.params),
        (h.admin.clone(), h.usdc.clone(), PARAMS)
    );
    assert_eq!(cfg.genesis_state_hash.to_array(), h.lane.state_hash());
    assert_eq!(
        (h.c().epoch(), h.c().min_valid_epoch(), h.c().inbox_count()),
        (1, 1, 0)
    );
    let last = h.c().last_checkpoint();
    assert_eq!(
        (last.seq, last.state_hash.to_array(), last.accepted_at),
        (0, h.lane.state_hash(), START)
    );
    assert!(!h.c().frozen());
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn constructor_rejects_a_bad_signer_set() {
    construct(|env| {
        let mut set = three(env);
        set.threshold = 4;
        (set, PARAMS)
    });
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn constructor_rejects_a_zero_min_deposit() {
    construct(|env| {
        (
            three(env),
            Params {
                min_deposit: 0,
                ..PARAMS
            },
        )
    });
}

// --- Inbox ----------------------------------------------------------------------------

#[test]
fn deposits_and_forced_withdrawals_chain_the_inbox() {
    let mut h = Harness::new();
    assert_eq!(h.deposit(A, 100 * USDC), 0);
    h.advance(5);
    assert_eq!(h.deposit(B, 50 * USDC), 1);
    let a = h.user(A, 0);
    assert_eq!(
        h.c()
            .request_forced_withdrawal(&a, &key32(&h.env, A), &(20 * USDC)),
        2
    );
    assert_eq!(h.c().inbox_count(), 3);
    assert_eq!(h.token().balance(&h.id), 150 * USDC);

    let mut acc = [0u8; 32];
    let mut cum = 0;
    for i in 0..3u64 {
        let m = h.c().inbox(&i).unwrap();
        let kind = if m.kind == 0 {
            InboxKind::Deposit
        } else {
            InboxKind::ForcedWithdrawal
        };
        let msg = InboxMsgV1 {
            kind,
            index: i,
            lane_account: m.lane_account.to_array(),
            amount: m.amount,
            enqueued_at: m.enqueued_at,
        };
        acc = sha256(&inbox_acc_preimage(&acc, &msg.encode()));
        if kind == InboxKind::Deposit {
            cum += m.amount;
        }
        assert_eq!(
            (m.acc_after.to_array(), m.cum_deposits_after, m.refunded),
            (acc, cum, false)
        );
    }
    assert_eq!(h.c().inbox(&1).unwrap().enqueued_at, START + 5);
    assert_eq!(h.c().inbox(&2).unwrap().kind, 1);
    assert_eq!(cum, 150 * USDC);

    // The lane folds the same chain.
    h.sync_inbox();
    h.lane.block();
    let st = h.lane.state();
    assert_eq!((st.inbox_through, st.inbox_acc), (3, acc));
}

#[test]
fn deposit_event() {
    let h = Harness::new();
    let who = h.user(A, 100 * USDC);
    h.c().deposit(&who, &(100 * USDC), &key32(&h.env, A));
    let msg = InboxMsgV1 {
        kind: InboxKind::Deposit,
        index: 0,
        lane_account: caravel_types::vectors::pk(A),
        amount: 100 * USDC,
        enqueued_at: START,
    };
    let acc_after = BytesN::from_array(
        &h.env,
        &sha256(&inbox_acc_preimage(&[0; 32], &msg.encode())),
    );
    let event = InboxEvent {
        index: 0,
        kind: 0,
        lane_account: key32(&h.env, A),
        amount: 100 * USDC,
        enqueued_at: START,
        acc_after,
        from: who,
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
}

#[test]
fn deposit_rules() {
    let h = Harness::new();
    let who = h.user(A, 100 * USDC);
    let key = key32(&h.env, A);
    expect_err(
        h.c().try_deposit(&who, &(USDC - 1), &key),
        Error::BelowMinDeposit,
    );
    expect_err(h.c().try_deposit(&who, &-1, &key), Error::BelowMinDeposit);
    // A deposit may credit any lane account.
    h.c().deposit(&who, &USDC, &key32(&h.env, B));
    assert_eq!(h.c().inbox(&0).unwrap().lane_account, key32(&h.env, B));
    // The sender must authorize it.
    h.env.set_auths(&[]);
    let r = h.c().try_deposit(&who, &USDC, &key);
    assert!(r.is_err() && code_of(r).is_none());
    h.env.mock_all_auths();
    h.c().deposit(&who, &USDC, &key);
    assert_eq!(h.env.auths().first().map(|(a, _)| a.clone()), Some(who));
}

#[test]
fn forced_withdrawal_rules() {
    let h = Harness::new();
    let a = h.user(A, 0);
    let b = h.user(B, 0);
    let key = key32(&h.env, A);
    expect_err(
        h.c().try_request_forced_withdrawal(&b, &key, &USDC),
        Error::NotOwner,
    );
    expect_err(
        h.c()
            .try_request_forced_withdrawal(&Address::generate(&h.env), &key, &USDC),
        Error::NotOwner,
    );
    expect_err(
        h.c().try_request_forced_withdrawal(&a, &key, &0),
        Error::BadAmount,
    );
    expect_err(
        h.c().try_request_forced_withdrawal(&a, &key, &-5),
        Error::BadAmount,
    );
    h.env.set_auths(&[]);
    let r = h.c().try_request_forced_withdrawal(&a, &key, &USDC);
    assert!(r.is_err() && code_of(r).is_none());
    assert_eq!(h.c().inbox_count(), 0);
}

#[test]
fn forced_withdrawal_reaches_a_claim() {
    let mut h = Harness::new();
    h.deposit(A, 100 * USDC);
    h.sync_inbox();
    h.lane.block();
    let a = h.user(A, 0);
    h.c()
        .request_forced_withdrawal(&a, &key32(&h.env, A), &(40 * USDC));
    h.sync_inbox();
    h.lane.block();
    let cp = h.checkpoint();
    assert_eq!(
        cp.withdrawals,
        std::vec![(caravel_types::vectors::pk(A), 40 * USDC)]
    );
    h.accept(&cp);
    h.c().claim_withdrawal(
        &a,
        &key32(&h.env, A),
        &1,
        &0,
        &(40 * USDC),
        &h.withdrawal_proof(&cp, 0),
    );
    assert_eq!(h.token().balance(&a), 40 * USDC);
}

// --- TTL (spec §13.1) ------------------------------------------------------------------

fn ttls(h: &Harness, key: &DataKey) -> (u32, u32) {
    h.env.as_contract(&h.id, || {
        (
            h.env.storage().instance().get_ttl(),
            h.env.storage().persistent().get_ttl(key),
        )
    })
}

#[test]
fn entries_live_120_days() {
    let mut h = Harness::new();
    assert_eq!(ttls(&h, &DataKey::Signers(1)), (TARGET, TARGET));
    h.deposit(A, 100 * USDC);
    assert_eq!(ttls(&h, &DataKey::Inbox(0)), (TARGET, TARGET));
    h.sync_inbox();
    h.lane.block();
    let cp = h.checkpoint();
    h.accept(&cp);
    assert_eq!(ttls(&h, &DataKey::Ckpt(1)).1, TARGET);
}

#[test]
fn reads_extend_entries_below_30_days() {
    let mut h = Harness::new();
    h.deposit(A, 100 * USDC);
    // 91 days later the entry has 29 days left; reading it extends it.
    let gone = TARGET - THRESHOLD + DAY;
    h.env.ledger().with_mut(|l| l.sequence_number += gone);
    assert_eq!(
        ttls(&h, &DataKey::Inbox(0)),
        (THRESHOLD - DAY, THRESHOLD - DAY)
    );
    h.c().inbox(&0).unwrap();
    assert_eq!(ttls(&h, &DataKey::Inbox(0)).1, TARGET);
    // Above 30 days, a read leaves it alone.
    h.env.ledger().with_mut(|l| l.sequence_number += DAY);
    h.c().inbox(&0).unwrap();
    assert_eq!(ttls(&h, &DataKey::Inbox(0)).1, TARGET - DAY);
}

#[test]
fn ttl_is_clamped_to_the_network_maximum() {
    let mut h = Harness::new();
    h.env.ledger().set_max_entry_ttl(100_000);
    h.deposit(A, 100 * USDC);
    assert_eq!(ttls(&h, &DataKey::Inbox(0)).1, 100_000);
    // The instance already had 120 days, above the clamped threshold: left alone.
    assert_eq!(ttls(&h, &DataKey::Inbox(0)).0, TARGET);
}

// --- Upgrade (testnet only) --------------------------------------------------------------

#[test]
fn upgrade_needs_the_admin() {
    let h = Harness::new();
    let hash = h
        .env
        .deployer()
        .upload_contract_wasm(settlement_wasm().as_slice());
    h.env.set_auths(&[]);
    let r = h.c().try_upgrade(&hash);
    assert!(r.is_err() && code_of(r).is_none());
    h.env.mock_all_auths();
    h.c().upgrade(&hash);
    assert_eq!(
        h.env.auths().first().map(|(a, _)| a.clone()),
        Some(h.admin.clone())
    );
    let event = UpgradeEvent {
        new_wasm_hash: hash,
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
}

#[test]
fn upgrade_keeps_the_state() {
    let mut h = Harness::new();
    h.deposit(A, 100 * USDC);
    let hash = h
        .env
        .deployer()
        .upload_contract_wasm(settlement_wasm().as_slice());
    h.c().upgrade(&hash);
    assert_eq!(h.c().inbox_count(), 1);
    assert_eq!(h.c().config().params, PARAMS);
    h.deposit(B, 10 * USDC);
    assert_eq!(h.c().inbox_count(), 2);
}

// --- Budget (spec §13.7) ----------------------------------------------------------------

/// Live testnet limits (`stellar network settings --network testnet`, 2026-09-29).
const TX_MAX_INSTRUCTIONS: i64 = 400_000_000;
const TX_MEMORY_LIMIT: i64 = 41_943_040;
const TX_MAX_SIZE_BYTES: usize = 132_096;
const TX_MAX_WRITE_BYTES: u32 = 132_096;
const TX_MAX_WRITE_ENTRIES: u32 = 200;
const TX_MAX_DISK_READ_ENTRIES: u32 = 200;
const TX_MAX_EVENTS_BYTES: u32 = 16_384;

#[test]
fn a_full_batch_with_3_signatures_fits_testnet_limits() {
    let mut h = Harness::new_wasm();
    h.deposit(A, 100 * USDC);
    h.sync_inbox();
    h.lane.block();
    h.lane.withdraw(A, 30 * USDC);
    h.lane.block();
    let mut cp = h.checkpoint();
    // The real batch, padded to the cap (the contract hashes it, it does not parse it).
    let real = cp.batch.len();
    cp.batch
        .extend((real..MAX_BATCH_BYTES as usize).map(|i| (i % 251) as u8));
    assert_eq!(cp.batch.len(), 96_000);
    cp.header.batch_hash = sha256(&cp.batch);
    let sigs = h.sign(&cp.header, &[0, 1, 2]);

    let args: soroban_sdk::Vec<Val> = (
        Bytes::from_slice(&h.env, &cp.header.encode()),
        Bytes::from_slice(&h.env, &cp.batch),
        1u64,
        sigs.clone(),
    )
        .into_val(&h.env);
    let args_xdr = args.to_xdr(&h.env).len() as usize;

    h.env
        .cost_estimate()
        .budget()
        .reset_limits(TX_MAX_INSTRUCTIONS as u64, TX_MEMORY_LIMIT as u64);
    h.accept_with(&cp, 1, &sigs);
    let r = h.env.cost_estimate().resources();
    let fee = h.env.cost_estimate().fee();
    std::println!(
        "submit_checkpoint, 96,000 B batch, 3 signatures: {} instructions, {} memory bytes, {} disk reads ({} B), {} writes ({} B), {} event bytes, args {} B; fee estimate {} stroops (instructions {}, persistent rent {})",
        r.instructions, r.mem_bytes, r.disk_read_entries, r.disk_read_bytes, r.write_entries, r.write_bytes, r.contract_events_size_bytes, args_xdr, fee.total, fee.instructions, fee.persistent_entry_rent
    );
    assert!(
        r.instructions < TX_MAX_INSTRUCTIONS / 4,
        "instructions {}",
        r.instructions
    );
    assert!(r.mem_bytes < TX_MEMORY_LIMIT, "memory {}", r.mem_bytes);
    assert!(
        r.disk_read_entries <= TX_MAX_DISK_READ_ENTRIES && r.write_entries <= TX_MAX_WRITE_ENTRIES
    );
    assert!(r.write_bytes <= TX_MAX_WRITE_BYTES);
    assert!(r.contract_events_size_bytes <= TX_MAX_EVENTS_BYTES);
    // The envelope adds the source account, footprint, fee and one signature:
    // a few hundred bytes. Keep 4 KiB of room.
    assert!(args_xdr + 4096 <= TX_MAX_SIZE_BYTES, "args {args_xdr} B");
}

// --- Storage layout read by validators off-chain --------------------------------------------

/// Validators and replay read storage straight from ledger entries with
/// `getLedgerEntries` (caravel-node `stellar_rpc`, `replay`). Unit variants
/// are `Vec[Symbol(name)]`, tuple variants `Vec[Symbol(name), fields...]`,
/// and structs maps keyed by field-name symbols.
#[test]
fn storage_layout_read_off_chain() {
    use soroban_sdk::xdr::{ScMap, ScMapEntry, ScSymbol, ScVal, ScVec};
    use soroban_sdk::TryFromVal;
    let mut h = Harness::new();
    h.deposit(A, 100 * USDC);
    h.sync_inbox();
    h.lane.block();
    h.lane.withdraw(A, 30 * USDC);
    h.lane.block();
    let cp = h.checkpoint();
    h.accept(&cp);
    let who = h.user(A, 0);
    h.c().claim_withdrawal(
        &who,
        &key32(&h.env, A),
        &1,
        &0,
        &(30 * USDC),
        &h.withdrawal_proof(&cp, 0),
    );
    let env = &h.env;
    let sym = |s: &str| ScVal::Symbol(ScSymbol(s.try_into().unwrap()));
    let xdr_of = |v: Val| ScVal::try_from_val(env, &v).unwrap();
    let vec_of = |items: StdVec<ScVal>| ScVal::Vec(Some(ScVec(items.try_into().unwrap())));
    let field = |m: &ScMap, name: &str| {
        m.0.iter()
            .find(|e: &&ScMapEntry| e.key == sym(name))
            .map(|e| e.val.clone())
            .unwrap()
    };
    let map_of = |v: ScVal| match v {
        ScVal::Map(Some(m)) => m,
        other => panic!("not a map: {other:?}"),
    };

    // Keys.
    assert_eq!(
        xdr_of(DataKey::LastCkpt.into_val(env)),
        vec_of(std::vec![sym("LastCkpt")])
    );
    assert_eq!(
        xdr_of(DataKey::Config.into_val(env)),
        vec_of(std::vec![sym("Config")])
    );
    assert_eq!(
        xdr_of(DataKey::Ckpt(1).into_val(env)),
        vec_of(std::vec![sym("Ckpt"), ScVal::U64(1)])
    );
    assert_eq!(
        xdr_of(DataKey::Claimed(1, 0).into_val(env)),
        vec_of(std::vec![sym("Claimed"), ScVal::U64(1), ScVal::U32(0)])
    );

    // Values, as stored.
    let header_hash = sha256(&cp.header.encode());
    let last = map_of(xdr_of(env.as_contract(&h.id, || {
        env.storage()
            .instance()
            .get::<DataKey, Val>(&DataKey::LastCkpt)
            .unwrap()
    })));
    assert_eq!(field(&last, "seq"), ScVal::U64(1));
    assert!(
        matches!(field(&last, "header_hash"), ScVal::Bytes(b) if b.0.as_slice() == header_hash.as_slice())
    );
    let config = map_of(xdr_of(env.as_contract(&h.id, || {
        env.storage()
            .instance()
            .get::<DataKey, Val>(&DataKey::Config)
            .unwrap()
    })));
    for (name, want) in [
        ("lane_id", caravel_testkit::lane::config().lane_id),
        ("engine_wasm_hash", ENGINE_HASH),
        ("config_hash", h.lane.config_hash),
    ] {
        assert!(
            matches!(field(&config, name), ScVal::Bytes(b) if b.0.as_slice() == want.as_slice()),
            "{name}"
        );
    }
    assert!(matches!(field(&config, "genesis_state_hash"), ScVal::Bytes(b) if b.0.len() == 32));
    let record = map_of(xdr_of(env.as_contract(&h.id, || {
        env.storage()
            .persistent()
            .get::<DataKey, Val>(&DataKey::Ckpt(1))
            .unwrap()
    })));
    assert!(
        matches!(field(&record, "header_hash"), ScVal::Bytes(b) if b.0.as_slice() == header_hash.as_slice())
    );
    assert_eq!(field(&record, "withdrawal_count"), ScVal::U32(1));
    assert_eq!(field(&record, "stellar_ledger"), ScVal::U32(1_000));
    assert!(env.as_contract(&h.id, || env
        .storage()
        .persistent()
        .has(&DataKey::Claimed(1, 0))));
}
