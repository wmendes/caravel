//! Store pruning (DEC-105, spec §14.4, §15): snapshots older than the last
//! accepted checkpoint minus 3 and accepted batches older than that go;
//! blocks, headers, withdrawals and every proof still answer.

mod common;

use common::harness::*;

use axum::response::IntoResponse;
use caravel_perps_node::PerpsApp;
use caravel_runtime::store::{CheckpointStatus, History, Pruned, Store, KEEP_ACCEPTED_SNAPSHOTS};
use caravel_testkit::lane::seeds;
use caravel_types::vectors::pk;
use serde_json::Value;

async fn body(r: caravel_node::api::ApiResult) -> Value {
    let resp = r.into_response();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn accept_all(store: &mut Store) -> u64 {
    let mut last = 0;
    for row in store.checkpoints_with(CheckpointStatus::Sequenced).unwrap() {
        store.set_signed(row.seq, 1, "[]").unwrap();
        store
            .set_accepted(row.seq, &format!("{:064x}", row.seq), row.seq as u32)
            .unwrap();
        last = row.seq;
    }
    last
}

#[tokio::test]
async fn prune_keeps_what_proofs_need() {
    let mut t = T::native();
    busy_lane(&mut t, 125); // 12 checkpoints of 10 blocks
    let accepted = accept_all(t.core.store_mut());
    assert_eq!(accepted, 12);
    let keep_from = accepted - KEEP_ACCEPTED_SNAPSHOTS;

    let a = pk(seeds::A);
    let withdrawals_before = body(caravel_node::api::withdrawal_proofs(t.core.store(), &a)).await;
    let escape_before = body(caravel_node::api::escape_proof(
        &PerpsApp,
        t.core.store(),
        &a,
        Some(accepted),
    ))
    .await;
    let sizes: Vec<usize> = (1..=accepted)
        .map(|s| t.core.store().checkpoint(s).unwrap().unwrap().batch.len())
        .collect();
    assert!(withdrawals_before["withdrawals"]
        .as_array()
        .is_some_and(|w| !w.is_empty()));

    // Bounded passes: 3 rows of each kind at a time, until nothing is left.
    let store = t.core.store_mut();
    let mut total = Pruned::default();
    loop {
        let p = store.prune(3).unwrap();
        assert!(p.snapshots <= 3 && p.batches <= 3);
        if p == Pruned::default() {
            break;
        }
        total.snapshots += p.snapshots;
        total.batches += p.batches;
    }
    assert_eq!(
        total,
        Pruned {
            snapshots: (keep_from - 1) as usize,
            batches: (keep_from - 1) as usize
        }
    );

    // Genesis, then the last accepted and the 3 before it, are kept.
    for seq in 0..=accepted {
        let kept = t.core.store().snapshot(seq).unwrap().is_some();
        assert_eq!(kept, seq == 0 || seq >= keep_from, "snapshot {seq}");
    }
    // Old batches are empty, but their size is still reported; blocks stay.
    for seq in 1..=accepted {
        let row = t.core.store().checkpoint(seq).unwrap().unwrap();
        assert_eq!(row.batch.is_empty(), seq < keep_from, "batch {seq}");
        assert_eq!(row.batch_len, sizes[seq as usize - 1], "batch_len {seq}");
    }
    assert!(t.core.store().block(1).unwrap().is_some());

    // Every proof answers as before.
    assert_eq!(
        body(caravel_node::api::withdrawal_proofs(t.core.store(), &a)).await,
        withdrawals_before
    );
    assert_eq!(
        body(caravel_node::api::escape_proof(
            &PerpsApp,
            t.core.store(),
            &a,
            Some(accepted)
        ))
        .await,
        escape_before
    );
    let v = body(caravel_node::api::checkpoint_json(t.core.store(), 1)).await;
    assert_eq!(v["batch_bytes"], sizes[0]);

    // The lane goes on: new checkpoints seal and are kept until accepted.
    busy_more(&mut t, 145);
    assert_eq!(t.core.store_mut().prune(100).unwrap(), Pruned::default());
}

/// A file-backed core on `path`, as a node opens it.
fn on_file(path: &std::path::Path) -> T {
    let (state, config_hash) = genesis();
    let store = Store::open(
        path,
        &caravel_testkit::lane::config().lane_id,
        &config_hash,
        &state,
    )
    .unwrap();
    T::with(caravel_runtime::sequencer::Executor::Native, store)
}

/// F-08: the sequencer archives the blocks of old accepted checkpoints and
/// serves them byte for byte as before; it restarts and goes on.
#[tokio::test]
async fn the_sequencer_serves_archived_blocks_as_before() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sequencer.sqlite");
    let mut t = on_file(&path);
    busy_lane(&mut t, 125);
    let accepted = accept_all(t.core.store_mut());
    let views: Vec<Value> = futures_blocks(&t, 125).await;
    let store = t.core.store_mut();
    while store.prune_history(History::Archive, 4).unwrap() > 0 {}
    let archived = (accepted - KEEP_ACCEPTED_SNAPSHOTS) * 10;
    let raw: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row("SELECT MIN(height) FROM blocks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(raw as u64, archived + 1);
    assert_eq!(
        futures_blocks(&t, 125).await,
        views,
        "every block view is the same"
    );
    let (hash, height, now) = (t.core.state_hash(), t.core.height(), t.now);
    drop(t);
    let mut t = on_file(&path);
    assert_eq!((t.core.state_hash(), t.core.height()), (hash, height));
    t.now = now;
    busy_more(&mut t, 135);
}

/// F-08: a validator drops old blocks; asked for one, it says so.
#[tokio::test]
async fn dropped_blocks_answer_pruned() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("validator.sqlite");
    let mut t = on_file(&path);
    busy_lane(&mut t, 125);
    let accepted = accept_all(t.core.store_mut());
    let store = t.core.store_mut();
    while store.prune(100).unwrap() != Pruned::default() {}
    while store.prune_history(History::Drop, 4).unwrap() > 0 {}
    let first = (accepted - KEEP_ACCEPTED_SNAPSHOTS) * 10 + 1;
    assert_eq!(t.core.store().first_block().unwrap(), Some(first));
    let gone = caravel_node::api::block_json(&PerpsApp, t.core.store(), first - 1)
        .expect_err("pruned")
        .into_response();
    assert_eq!(gone.status(), axum::http::StatusCode::GONE);
    assert!(caravel_node::api::block_json(&PerpsApp, t.core.store(), first).is_ok());
    let missing = caravel_node::api::block_json(&PerpsApp, t.core.store(), 500)
        .expect_err("past the tip")
        .into_response();
    assert_eq!(missing.status(), axum::http::StatusCode::NOT_FOUND);
    let now = t.now;
    drop(t);
    let mut t = on_file(&path);
    t.now = now;
    busy_more(&mut t, 135);
}

async fn futures_blocks(t: &T, to: u64) -> Vec<Value> {
    let mut out = Vec::new();
    for h in 1..=to {
        out.push(body(caravel_node::api::block_json(&PerpsApp, t.core.store(), h)).await);
    }
    out
}

fn busy_more(t: &mut T, height: u64) {
    while t.core.height() < height {
        t.prices();
        t.block();
    }
}

#[test]
fn an_old_store_gains_the_column_and_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.sqlite");
    {
        // The schema of releases before DEC-105.
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE checkpoints (seq INTEGER PRIMARY KEY, header BLOB NOT NULL, batch BLOB NOT NULL, first_height INTEGER NOT NULL, last_height INTEGER NOT NULL, withdrawals TEXT NOT NULL, status TEXT NOT NULL, epoch INTEGER, sigs TEXT, stellar_tx_hash TEXT, stellar_ledger INTEGER);
             INSERT INTO checkpoints VALUES (1, x'00', x'0102030405', 1, 10, '[]', 'accepted', 1, '[]', 'ab', 7);",
        )
        .unwrap();
    }
    let (state, config_hash) = genesis();
    let store = Store::open(
        &path,
        &caravel_testkit::lane::config().lane_id,
        &config_hash,
        &state,
    )
    .unwrap();
    assert_eq!(
        store.checkpoint(1).unwrap().unwrap().batch_len,
        5,
        "old rows report their batch's length"
    );
    drop(store);
    let c = rusqlite::Connection::open(&path).unwrap();
    let idx: i64 = c
        .query_row("SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'checkpoints_status_seq'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(idx, 1);
    let plan: String = c
        .query_row(
            "EXPLAIN QUERY PLAN SELECT MAX(seq) FROM checkpoints WHERE status = 'signed'",
            [],
            |r| r.get(3),
        )
        .unwrap();
    assert!(plan.contains("checkpoints_status_seq"), "{plan}");
}
