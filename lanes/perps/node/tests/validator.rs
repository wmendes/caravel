//! The validator core (spec §15): it follows the sequencer by re-execution,
//! computes its own headers, halts on a tampered block, flags live-policy
//! failures, and never signs two different headers for one seq.

mod common;

use caravel_core::batch::BatchV1;
use caravel_core::block::BlockRecordV1;
use caravel_core::checkpoint::CheckpointHeaderV1;
use caravel_perps::native::verify_strict;
use caravel_perps_node::PerpsApp;
use caravel_runtime::checkpoint::sha256;
use caravel_runtime::sequencer::Executor;
use caravel_runtime::store::Store;
use caravel_runtime::validator::{FollowError, Follower, Refusal, LIVE_WINDOW_MS};
use caravel_testkit::lane::{config, seeds, BTC};
use common::harness::*;
use ed25519_dalek::SigningKey;

fn follower(exec: Executor) -> Follower<PerpsApp> {
    let (state, config_hash) = genesis();
    let store = Store::open_in_memory(&config().lane_id, &config_hash, &state).unwrap();
    Follower::open(
        PerpsApp,
        exec,
        store,
        ids(),
        SigningKey::from_bytes(&[0x61; 32]),
    )
    .unwrap()
}

/// Records `from..=to` from the sequencer, as `/v1/blocks` serves them.
fn records(t: &T, from: u64, to: u64) -> Vec<BlockRecordV1> {
    t.core
        .store()
        .blocks(from, to)
        .unwrap()
        .into_iter()
        .map(|(r, _)| r)
        .collect()
}

/// Catch-up: applied long after production, so no live checks.
fn catch_up(v: &mut Follower<PerpsApp>, t: &T) {
    let late = t.now + 3_600_000;
    for r in records(t, v.height() + 1, t.core.height()) {
        v.apply(&r, late).unwrap();
    }
}

fn sealed(t: &T, seq: u64) -> (Vec<u8>, Vec<u8>) {
    let row = t.core.store().checkpoint(seq).unwrap().unwrap();
    (row.header, row.batch)
}

#[test]
fn a_validator_reproduces_the_sequencer_and_signs() {
    let mut t = T::native();
    busy_lane(&mut t, 22);
    let mut v = follower(Executor::Wasm(common::wasm()));
    catch_up(&mut v, &t);
    assert_eq!(
        (v.height(), v.state_hash()),
        (t.core.height(), t.core.state_hash())
    );
    for seq in [1, 2] {
        let (header, batch) = sealed(&t, seq);
        assert_eq!(
            v.store().checkpoint(seq).unwrap().unwrap().header,
            header,
            "seq {seq}: our own header"
        );
        let sig = v.sign(&header, &batch).unwrap();
        assert!(verify_strict(&v.public_key(), &sha256(&header), &sig));
        assert_eq!(v.store().signed(seq).unwrap(), Some(sha256(&header)));
    }
}

#[test]
fn a_tampered_block_halts_the_validator_and_it_refuses_to_sign() {
    let mut t = T::native();
    busy_lane(&mut t, 12);
    let mut v = follower(Executor::Native);
    let recs = records(&t, 1, 12);
    for r in &recs[..4] {
        v.apply(r, t.now + 3_600_000).unwrap();
    }
    let mut bad = recs[4].clone();
    bad.state_hash_after[0] ^= 1;
    assert!(matches!(
        v.apply(&bad, t.now),
        Err(FollowError::Mismatch(_))
    ));
    assert!(v.halted().is_some());
    // Halted for good: the honest record is refused too, and so is every signature.
    assert!(matches!(
        v.apply(&recs[4], t.now),
        Err(FollowError::Halted(_))
    ));
    let (header, batch) = sealed(&t, 1);
    assert!(matches!(v.sign(&header, &batch), Err(Refusal::Halted(_))));
}

#[test]
fn a_block_off_our_chain_halts_the_validator() {
    let mut t = T::native();
    busy_lane(&mut t, 3);
    let mut v = follower(Executor::Native);
    let recs = records(&t, 1, 3);
    v.apply(&recs[0], t.now + 3_600_000).unwrap();
    assert!(matches!(
        v.apply(&recs[2], t.now + 3_600_000),
        Err(FollowError::Mismatch(_))
    ));
}

#[test]
fn never_two_headers_for_one_seq() {
    let mut t = T::native();
    busy_lane(&mut t, 22);
    let mut v = follower(Executor::Native);
    catch_up(&mut v, &t);
    let (h1, b1) = sealed(&t, 1);
    let sig = v.sign(&h1, &b1).unwrap();
    // Idempotent: the same header again gives the same signature.
    assert_eq!(v.sign(&h1, &b1).unwrap(), sig);
    // Another header for seq 1 is refused, whatever it says.
    let mut other = CheckpointHeaderV1::decode(&h1).unwrap();
    other.escape_total += 1;
    assert_eq!(v.sign(&other.encode(), &b1), Err(Refusal::HeaderMismatch));
    // After seq 2, seq 1 is never signed again.
    let (h2, b2) = sealed(&t, 2);
    v.sign(&h2, &b2).unwrap();
    assert!(matches!(
        v.sign(&h1, &b1),
        Err(Refusal::Equivocation {
            seq: 1,
            last_signed: 2
        })
    ));
    // The rule holds across a restart: it lives in the store.
    assert!(v.store_mut().record_signed(2, &[0xAB; 32]).is_err());
}

#[test]
fn the_batch_must_match_our_blocks() {
    let mut t = T::native();
    busy_lane(&mut t, 10);
    let mut v = follower(Executor::Native);
    catch_up(&mut v, &t);
    let (header, batch) = sealed(&t, 1);
    let mut b = BatchV1::decode(&batch).unwrap();
    b.blocks.pop();
    assert_eq!(
        v.sign(&header, &b.encode().unwrap()),
        Err(Refusal::BatchMismatch)
    );
    assert_eq!(v.sign(&header, &[1, 2, 3]), Err(Refusal::BatchMismatch));
    // A checkpoint the validator has not reached yet.
    let mut ahead = CheckpointHeaderV1::decode(&header).unwrap();
    ahead.seq = 9;
    assert!(matches!(
        v.sign(&ahead.encode(), &batch),
        Err(Refusal::NotCaughtUp { seq: 9, .. })
    ));
}

#[test]
fn live_checks_flag_blocks_and_block_signing_until_cleared() {
    let mut t = T::native();
    busy_lane(&mut t, 10);
    let mut v = follower(Executor::Native);
    let recs = records(&t, 1, 10);
    // Block 3 arrives live, but the validator's clock is 20 s behind: its
    // timestamp is too far ahead, and its oracle entries too far from now.
    let block3 = recs[2].decode_input().unwrap();
    for (i, r) in recs.iter().enumerate() {
        let now = if i == 2 {
            block3.timestamp_ms - 20_000
        } else {
            t.now + LIVE_WINDOW_MS * 1000
        };
        let applied = v.apply(r, now).unwrap();
        assert_eq!(applied.flags.is_empty(), i != 2, "block {}", i + 1);
    }
    let (header, batch) = sealed(&t, 1);
    assert!(
        matches!(v.sign(&header, &batch), Err(Refusal::Suspicious(ref f)) if f.len() == 1 && f[0].0 == 3)
    );
    // An operator clears it; the state was right all along.
    v.store_mut().clear_flags(10).unwrap();
    v.sign(&header, &batch).unwrap();
}

#[test]
fn a_restarted_validator_resumes_and_keeps_its_signatures() {
    let mut t = T::native();
    busy_lane(&mut t, 15);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("validator.sqlite");
    let (state, config_hash) = genesis();
    // As a validator node opens it (F-07): a lazy head, blocks not synced.
    let open = || {
        let mut store = Store::open(&path, &config().lane_id, &config_hash, &state).unwrap();
        store.set_head_every(4);
        store
            .set_block_durability(caravel_runtime::store::Durability::Normal)
            .unwrap();
        Follower::open(
            PerpsApp,
            Executor::Native,
            store,
            ids(),
            SigningKey::from_bytes(&[0x62; 32]),
        )
        .unwrap()
    };
    let mut v = open();
    catch_up(&mut v, &t);
    let (h1, b1) = sealed(&t, 1);
    v.sign(&h1, &b1).unwrap();
    drop(v);
    let mut v = open();
    assert_eq!(v.height(), 15);
    t.order(seeds::A, BTC, caravel_types::tx::Side::Sell, 65_000_000, 1);
    while t.core.height() < 20 {
        t.block();
    }
    catch_up(&mut v, &t);
    assert_eq!(v.state_hash(), t.core.state_hash());
    let (h2, b2) = sealed(&t, 2);
    assert_eq!(v.store().checkpoint(2).unwrap().unwrap().header, h2);
    v.sign(&h2, &b2).unwrap();
    assert!(matches!(
        v.sign(&h1, &b1),
        Err(Refusal::Equivocation { .. })
    ));
}

#[test]
fn accepted_checkpoints_come_from_stellar() {
    let mut t = T::native();
    busy_lane(&mut t, 22);
    let mut v = follower(Executor::Native);
    catch_up(&mut v, &t);
    let (h2, _) = sealed(&t, 2);
    assert!(!v.observe_accepted(2, &[0; 32]).unwrap());
    assert!(v.observe_accepted(2, &sha256(&h2)).unwrap());
    let accepted = v
        .store()
        .last_checkpoint_with(caravel_runtime::store::CheckpointStatus::Accepted)
        .unwrap();
    assert_eq!(accepted, Some(2));
}
