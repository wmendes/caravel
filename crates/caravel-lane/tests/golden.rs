//! P-01 (M0.5 platform split): a golden trace of the M0 sequencer core.
//!
//! A scripted workload on a fake clock (deposits, a forced withdrawal,
//! prices, crosses, resting orders, cancels, withdrawals, a quarantined
//! entry and an inbox mismatch) runs through `Core`. Every block, receipt,
//! checkpoint, withdrawal and escape leaf, proof and view is written to
//! `tests/golden/m0-trace.json`, and the store it leaves behind is kept as
//! `tests/fixtures/m0-sequencer.sqlite`. The split must reproduce both
//! (plan P-05). `UPDATE_GOLDEN=1` rewrites them; never on a split branch.

mod common;

use std::path::PathBuf;

use common::harness::*;

use caravel_lane::checkpoint::{self, sha256};
use caravel_lane::sequencer::{self, hex, Executor, InboxReport, Produced};
use caravel_lane::store::Store;
use caravel_lane::views;
use caravel_testkit::lane::{
    config, seeds, BTC, BTC_PRICE, ETH, ETH_PRICE, TICK, USDC, XLM, XLM_PRICE,
};
use caravel_types::block::BlockInputV1;
use caravel_types::checkpoint::CheckpointHeaderV1;
use caravel_types::inbox::{InboxKind, InboxMsgV1};
use caravel_types::state::StateV1;
use caravel_types::tx::{Side, TxBody};
use caravel_types::vectors::pk;
use serde_json::{json, Value};

const ACCOUNTS: [u8; 3] = [seeds::A, seeds::B, seeds::C];

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn updating() -> bool {
    std::env::var_os("UPDATE_GOLDEN").is_some()
}

fn block_json(t: &T, p: &Produced) -> Value {
    let input = BlockInputV1::decode(&p.record.input).unwrap();
    let mut v = json!({
        "height": p.height,
        "record_hex": hex(&p.record.encode()),
        "receipts_hex": hex(&p.receipts_bytes),
        "state_hash": hex(&t.core.state_hash()),
        "view": views::block(&p.record, &p.receipts_bytes),
        "fills": views::fills(&p.state, p.height, input.timestamp_ms, &p.receipts),
        "incidents": p.incidents.iter().map(|i| format!("{i:?}")).collect::<Vec<_>>(),
    });
    if let Some(row) = &p.checkpoint {
        let header = CheckpointHeaderV1::decode(&row.header).unwrap();
        let withdrawals = sequencer::parse_leaves(&row.withdrawals).unwrap();
        let (_, snap) = t.core.store().snapshot(row.seq).unwrap().unwrap();
        let accounts =
            checkpoint::account_leaves(&header, &StateV1::decode(&snap).unwrap()).unwrap();
        let w_hashes = checkpoint::withdrawal_hashes(&header, &withdrawals);
        let a_hashes = checkpoint::account_hashes(&header, &accounts);
        v["checkpoint"] = json!({
            "seq": row.seq,
            "first_height": row.first_height,
            "last_height": row.last_height,
            "header_hex": hex(&row.header),
            "header": views::header(&header),
            "batch_hash": hex(&sha256(&row.batch)),
            "snapshot_hash": hex(&sha256(&snap)),
            "withdrawal_leaves": row.withdrawals,
            "account_leaves": sequencer::leaves_json(&accounts),
            "withdrawal_proofs": ACCOUNTS.iter().flat_map(|s| views::proofs_for(&header, &withdrawals, &w_hashes, &pk(*s))).collect::<Vec<_>>(),
            "escape_proofs": ACCOUNTS.iter().flat_map(|s| views::proofs_for(&header, &accounts, &a_hashes, &pk(*s))).collect::<Vec<_>>(),
        });
    }
    v
}

/// The scripted M0 workload; returns the trace.
fn workload(t: &mut T) -> Value {
    let mut blocks = Vec::new();
    let mut push = |t: &mut T, p: Produced| {
        let v = block_json(t, &p);
        blocks.push(v);
    };
    t.deposit(seeds::A, 10_000 * USDC);
    t.deposit(seeds::B, 10_000 * USDC);
    t.deposit(seeds::C, 5_000 * USDC);
    t.prices();
    let p = t.block();
    push(t, p);
    // A cross, resting orders on every market, and a withdrawal.
    t.order(seeds::A, BTC, Side::Buy, BTC_PRICE, 10);
    t.order(seeds::B, BTC, Side::Sell, BTC_PRICE, 4);
    t.order(seeds::B, ETH, Side::Sell, ETH_PRICE + 10 * TICK, 3);
    t.order(seeds::C, XLM, Side::Buy, XLM_PRICE - 5 * TICK, 7);
    t.tx(seeds::A, TxBody::Withdraw { amount: 100 * USDC });
    let p = t.block();
    push(t, p);
    // A transaction that skipped pre-validation: quarantined, block rebuilt.
    let mut bad = t.signed(
        seeds::A,
        t.next_nonce(seeds::A),
        TxBody::CancelAll { market_id: BTC },
    );
    bad.signature[5] ^= 1;
    t.core.mempool.push([0xBA; 32], bad).unwrap();
    t.order(seeds::C, BTC, Side::Sell, BTC_PRICE - 2 * TICK, 2);
    let p = t.block();
    push(t, p);
    let mut height_target = 12;
    while t.core.height() < height_target {
        if t.core.height().is_multiple_of(3) {
            t.prices();
        }
        let p = t.block();
        push(t, p);
    }
    // A forced withdrawal through the inbox, cancels, more crosses, a second withdrawal.
    t.inbox(InboxKind::ForcedWithdrawal, seeds::B, 50 * USDC);
    t.tx(seeds::B, TxBody::CancelAll { market_id: ETH });
    t.order(seeds::B, ETH, Side::Buy, ETH_PRICE, 2);
    t.order(seeds::A, ETH, Side::Sell, ETH_PRICE - TICK, 2);
    t.tx(seeds::C, TxBody::Withdraw { amount: 25 * USDC });
    height_target = 45;
    while t.core.height() < height_target {
        if t.core.height().is_multiple_of(4) {
            t.prices();
        }
        if t.core.height().is_multiple_of(7) {
            t.order(seeds::C, XLM, Side::Buy, XLM_PRICE - TICK, 1);
            t.order(seeds::A, XLM, Side::Sell, XLM_PRICE - TICK, 1);
        }
        let p = t.block();
        push(t, p);
    }
    // An inbox report whose acc_after is not our fold halts inbox inclusion.
    let msg = InboxMsgV1 {
        kind: InboxKind::Deposit,
        index: t.inbox_n,
        lane_account: pk(seeds::D),
        amount: 10 * USDC,
        enqueued_at: t.now / 1000,
    };
    assert_eq!(
        t.core.report_inbox(msg, [0xEE; 32]).unwrap(),
        InboxReport::Mismatch
    );
    while t.core.height() < 70 {
        if t.core.height().is_multiple_of(5) {
            t.prices();
        }
        let p = t.block();
        push(t, p);
    }
    let st = t.core.state();
    json!({
        "workload": "crates/caravel-lane/tests/golden.rs",
        "config_hash": hex(&t.core.config_hash()),
        "final": {
            "height": t.core.height(),
            "state_hash": hex(&t.core.state_hash()),
            "inbox_halted": t.core.inbox_halted(),
            "accounts": ACCOUNTS.iter().map(|s| views::account(&st, &pk(*s))).collect::<Vec<_>>(),
            "markets": views::markets(&st),
            "books": ([BTC, ETH, XLM].iter().map(|m| views::book(&st, *m, 20)).collect::<Vec<_>>()),
        },
        "blocks": blocks,
    })
}

fn run(path: &std::path::Path) -> Value {
    let (state, config_hash) = genesis();
    let store = Store::open(path, &config().lane_id, &config_hash, &state).unwrap();
    let mut t = T::with(Executor::Native, store);
    workload(&mut t)
}

#[test]
fn m0_sequencer_trace_is_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("lane.sqlite");
    let trace = run(&db);
    let text = serde_json::to_string_pretty(&trace).unwrap() + "\n";
    let golden = dir().join("golden/m0-trace.json");
    let fixture = dir().join("fixtures/m0-sequencer.sqlite");
    if updating() {
        std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
        std::fs::create_dir_all(fixture.parent().unwrap()).unwrap();
        std::fs::write(&golden, &text).unwrap();
        std::fs::copy(&db, &fixture).unwrap();
        return;
    }
    let want =
        std::fs::read_to_string(&golden).expect("golden trace (UPDATE_GOLDEN=1 on the M0 code)");
    if text != want {
        let line = text
            .lines()
            .zip(want.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(0);
        panic!(
            "the M0 trace changed at line {}: got {:?}, want {:?}",
            line + 1,
            text.lines().nth(line),
            want.lines().nth(line)
        );
    }
}

#[test]
fn the_workload_covers_what_the_split_must_keep() {
    let tmp = tempfile::tempdir().unwrap();
    let trace = run(&tmp.path().join("lane.sqlite"));
    let blocks = trace["blocks"].as_array().unwrap();
    let incidents: Vec<&str> = blocks
        .iter()
        .flat_map(|b| b["incidents"].as_array().unwrap())
        .filter_map(|i| i.as_str())
        .collect();
    assert!(
        incidents.iter().any(|i| i.starts_with("Quarantined")),
        "{incidents:?}"
    );
    let checkpoints: Vec<&Value> = blocks.iter().filter_map(|b| b.get("checkpoint")).collect();
    assert!(checkpoints.len() >= 6, "{} checkpoints", checkpoints.len());
    assert!(checkpoints
        .iter()
        .any(|c| !c["withdrawal_proofs"].as_array().unwrap().is_empty()));
    assert!(blocks
        .iter()
        .any(|b| !b["fills"].as_array().unwrap().is_empty()));
    assert_eq!(trace["final"]["inbox_halted"], true);
}

#[test]
fn the_fixture_store_is_the_trace_s_end_state() {
    let golden: Value =
        serde_json::from_str(&std::fs::read_to_string(dir().join("golden/m0-trace.json")).unwrap())
            .unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let copy = tmp.path().join("lane.sqlite");
    std::fs::copy(dir().join("fixtures/m0-sequencer.sqlite"), &copy).unwrap();
    let (state, config_hash) = genesis();
    let store = Store::open(&copy, &config().lane_id, &config_hash, &state).unwrap();
    let t = T::with(Executor::Native, store);
    assert_eq!(t.core.height(), golden["final"]["height"].as_u64().unwrap());
    assert_eq!(
        hex(&t.core.state_hash()),
        golden["final"]["state_hash"].as_str().unwrap()
    );
}
