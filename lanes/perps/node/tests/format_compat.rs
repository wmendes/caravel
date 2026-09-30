//! P-04 (DEC-052): the platform's generic formats read the M0 perps bytes.
//!
//! `caravel-core` reads blocks, batches, transactions, receipts and states
//! without knowing the app. These tests check it against the frozen perps
//! codecs (`caravel-types`, lanes/perps/engine, DEC-051) on every frozen
//! vector, the 70 blocks of the P-01 golden trace, and the states in the
//! fixture store:
//! - it decodes what the strict codec decodes, and re-encodes the same bytes;
//! - the fields the platform reads are the same values;
//! - what the strict codec accepts, the generic one accepts too (the strict
//!   one stays the arbiter: the engine and the app's mempool check).

use std::path::PathBuf;

use caravel_core as cc;
use caravel_types as m0;
use proptest::prelude::*;
use serde_json::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn frozen(name: &str) -> Value {
    let path = root().join("lanes/perps/engine/test-vectors").join(name);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// A named vector's bytes.
type Case = (String, Vec<u8>);

/// `(name, bytes)` for every valid vector, then every invalid one.
fn cases(file: &str) -> (Vec<Case>, Vec<Case>) {
    let v = frozen(file);
    let list = |k: &str| {
        v[k].as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| Some((c["name"].as_str()?.to_string(), unhex(c["hex"].as_str()?))))
            .collect()
    };
    (list("vectors"), list("invalid"))
}

fn same_entries(
    strict: &m0::block::BlockInputV1,
    generic: &cc::block::BlockInputV1,
    config_hash: &[u8; 32],
) {
    assert_eq!(strict.entries.len(), generic.entries.len());
    for (s, g) in strict.entries.iter().zip(&generic.entries) {
        match (s, g) {
            (m0::block::Entry::Inbox(a), cc::block::Entry::Inbox(b)) => {
                assert_eq!(a.encode(), b.encode())
            }
            (m0::block::Entry::Oracle(u), cc::block::Entry::Feed(p)) => {
                assert_eq!(&u.encode()[..], &p[..])
            }
            (m0::block::Entry::User(tx), cc::block::Entry::User(env)) => {
                assert_eq!(tx.encode(), env.encode().unwrap());
                assert_eq!(tx.kind() as u8, env.kind);
                assert_eq!(
                    tx.tx_hash_preimage(config_hash),
                    env.tx_hash_preimage(config_hash)
                );
            }
            (s, g) => panic!("entry kinds differ: {s:?} vs {g:?}"),
        }
    }
}

fn check_block(bytes: &[u8]) {
    let strict = m0::block::BlockInputV1::decode(bytes);
    let generic = cc::block::BlockInputV1::decode(bytes);
    if let Ok(strict) = strict {
        let generic = generic.expect("strict-valid block is generic-valid");
        assert_eq!(generic.encode().unwrap(), bytes);
        assert_eq!(
            (
                strict.lane_id,
                strict.height,
                strict.timestamp_ms,
                strict.prev_block_hash,
                strict.checkpoint_end
            ),
            (
                generic.lane_id,
                generic.height,
                generic.timestamp_ms,
                generic.prev_block_hash,
                generic.checkpoint_end
            )
        );
        same_entries(&strict, &generic, &[0x5A; 32]);
    }
}

#[test]
fn copied_modules_are_the_frozen_ones() {
    let frozen = root().join("lanes/perps/engine/crates");
    let core = root().join("platform/crates/caravel-core/src");
    for m in [
        "codec",
        "tags",
        "fixed",
        "inbox",
        "checkpoint",
        "step",
        "preimage",
    ] {
        let a = std::fs::read_to_string(frozen.join("caravel-types/src").join(format!("{m}.rs")))
            .unwrap();
        let b = std::fs::read_to_string(core.join(format!("{m}.rs"))).unwrap();
        assert_eq!(
            a, b,
            "caravel-core/src/{m}.rs differs from the frozen caravel-types copy"
        );
    }
    let tree = std::fs::read_to_string(frozen.join("caravel-merkle/src/tree.rs")).unwrap();
    let copy = std::fs::read_to_string(core.join("merkle/tree.rs")).unwrap();
    assert_eq!(tree.replace("use crate::{", "use super::{"), copy);
    let lib = std::fs::read_to_string(frozen.join("caravel-merkle/src/lib.rs")).unwrap();
    let module = std::fs::read_to_string(core.join("merkle/mod.rs")).unwrap();
    let body = |s: &str| s[s.find("/// Maximum tree depth").unwrap()..].to_string();
    assert_eq!(body(&lib), body(&module));
    let batch = std::fs::read_to_string(frozen.join("caravel-types/src/batch.rs")).unwrap();
    let generic = std::fs::read_to_string(core.join("batch.rs")).unwrap();
    let code = |s: &str| {
        let start = s.find("use alloc::vec::Vec;").unwrap();
        let end = s.find("#[cfg(test)]").unwrap_or(s.len());
        s[start..end].trim().to_string()
    };
    assert_eq!(code(&batch), code(&generic));
}

#[test]
fn platform_vectors_are_the_frozen_files() {
    for f in [
        "inbox_msg",
        "checkpoint_header",
        "step_envelope",
        "merkle",
        "signatures",
        "hashes",
    ] {
        let a = std::fs::read(
            root()
                .join("lanes/perps/engine/test-vectors")
                .join(format!("{f}.json")),
        )
        .unwrap();
        let b = std::fs::read(
            root()
                .join("platform/test-vectors")
                .join(format!("{f}.json")),
        )
        .unwrap();
        assert_eq!(
            a, b,
            "platform/test-vectors/{f}.json differs from the frozen file"
        );
    }
}

#[test]
fn every_frozen_block_vector_reads_generically() {
    for file in ["block_input.json"] {
        let (valid, invalid) = cases(file);
        assert!(!valid.is_empty());
        for (name, bytes) in &valid {
            assert!(m0::block::BlockInputV1::decode(bytes).is_ok(), "{name}");
            check_block(bytes);
        }
        for (_, bytes) in &invalid {
            check_block(bytes);
        }
    }
    let (valid, invalid) = cases("block_record.json");
    for (name, bytes) in valid.iter().chain(&invalid) {
        if m0::block::BlockRecordV1::decode(bytes).is_ok() {
            let g =
                cc::block::BlockRecordV1::decode(bytes).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(g.encode(), *bytes);
        }
    }
    let (valid, invalid) = cases("batch.json");
    for (name, bytes) in valid.iter().chain(&invalid) {
        if let Ok(s) = m0::batch::BatchV1::decode(bytes) {
            let g = cc::batch::BatchV1::decode(bytes).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(g.encode().unwrap(), *bytes);
            assert_eq!(
                (s.lane_id, s.checkpoint_seq, s.blocks.len()),
                (g.lane_id, g.checkpoint_seq, g.blocks.len())
            );
        }
    }
}

#[test]
fn every_frozen_transaction_reads_generically() {
    let (valid, invalid) = cases("lane_tx.json");
    assert_eq!(valid.len(), 6, "one vector per M0 kind");
    for (name, bytes) in valid.iter().chain(&invalid) {
        let Ok(tx) = m0::tx::LaneTxV1::decode(bytes) else {
            continue;
        };
        let env = cc::tx::TxEnvelopeV1::decode(bytes).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(env.encode().unwrap(), *bytes, "{name}");
        assert_eq!(
            (
                tx.lane_id,
                tx.account,
                tx.signer,
                tx.nonce,
                tx.expiry_ms,
                tx.sig_scheme as u8,
                tx.signature
            ),
            (
                env.lane_id,
                env.account,
                env.signer,
                env.nonce,
                env.expiry_ms,
                env.sig_scheme as u8,
                env.signature
            )
        );
        let standard = env.standard_body();
        match tx.body {
            m0::tx::TxBody::Withdraw { amount } => {
                assert_eq!(
                    standard,
                    Some(Ok(cc::tx::StandardBody::Withdraw { amount }))
                )
            }
            m0::tx::TxBody::AddSessionKey {
                session_key,
                expires_at_ms,
                permissions,
            } => assert_eq!(
                standard,
                Some(Ok(cc::tx::StandardBody::AddSessionKey {
                    session_key,
                    expires_at_ms,
                    permissions
                }))
            ),
            m0::tx::TxBody::RevokeSessionKey { session_key } => {
                assert_eq!(
                    standard,
                    Some(Ok(cc::tx::StandardBody::RevokeSessionKey { session_key }))
                )
            }
            _ => assert_eq!(standard, None, "{name}: perps kinds are app kinds"),
        }
    }
}

#[test]
fn frozen_receipts_read_generically() {
    let (valid, invalid) = cases("receipts.json");
    for (name, bytes) in valid.iter().chain(&invalid) {
        let Ok(strict) = m0::receipts::Receipts::decode(bytes) else {
            continue;
        };
        check_receipts(name, bytes, &strict);
    }
}

fn check_receipts(name: &str, bytes: &[u8], strict: &m0::receipts::Receipts) {
    let generic =
        cc::receipts::ReceiptsV1::decode(bytes).unwrap_or_else(|e| panic!("{name}: {e:?}"));
    assert_eq!(generic.encode().unwrap(), bytes, "{name}");
    assert_eq!(strict.receipts.len(), generic.receipts.len());
    for (s, g) in strict.receipts.iter().zip(&generic.receipts) {
        assert_eq!(
            (s.entry_index, s.code, s.events.len()),
            (g.entry_index, g.code, g.events.len())
        );
        for (se, ge) in s.events.iter().zip(&g.events) {
            assert_eq!(se.type_id(), ge.type_id);
            let want = match se {
                m0::receipts::Event::Deposit {
                    key,
                    amount,
                    outcome,
                } => Some(cc::receipts::PlatformEvent::Deposit {
                    key: *key,
                    amount: *amount,
                    outcome: match outcome {
                        m0::receipts::DepositOutcome::Credited => {
                            cc::receipts::DepositOutcome::Credited
                        }
                        m0::receipts::DepositOutcome::Created => {
                            cc::receipts::DepositOutcome::Created
                        }
                        m0::receipts::DepositOutcome::Bounced => {
                            cc::receipts::DepositOutcome::Bounced
                        }
                    },
                }),
                m0::receipts::Event::ForcedWithdrawalProcessed { key, amount } => {
                    Some(cc::receipts::PlatformEvent::ForcedWithdrawalProcessed {
                        key: *key,
                        amount: *amount,
                    })
                }
                m0::receipts::Event::Commitment {
                    seq,
                    withdrawals_total,
                    escape_total,
                } => Some(cc::receipts::PlatformEvent::Commitment {
                    seq: *seq,
                    withdrawals_total: *withdrawals_total,
                    escape_total: *escape_total,
                }),
                _ => None,
            };
            assert_eq!(ge.platform(), want.map(Ok), "{name}: event {}", ge.type_id);
        }
    }
}

fn check_state(name: &str, bytes: &[u8]) {
    let s = m0::state::StateV1::decode(bytes).unwrap();
    let f = cc::state::StateFrameV1::read(bytes).unwrap_or_else(|e| panic!("{name}: {e:?}"));
    assert_eq!(&f.magic, m0::state::STATE_MAGIC);
    assert_eq!(
        (
            f.lane_id,
            f.config_hash,
            f.height,
            f.last_block_input_hash,
            f.last_timestamp_ms,
            f.checkpoint_seq
        ),
        (
            s.lane_id,
            s.config_hash,
            s.height,
            s.last_block_input_hash,
            s.last_timestamp_ms,
            s.checkpoint_seq
        )
    );
    assert_eq!(
        (
            f.inbox_through,
            f.inbox_acc,
            f.app_word,
            f.deposits_credited_total,
            f.withdrawals_committed_total
        ),
        (
            s.inbox_through,
            s.inbox_acc,
            s.next_order_id,
            s.deposits_credited_total,
            s.withdrawals_committed_total
        )
    );
    assert_eq!(f.app_flags, u8::from(s.backstop_deficit));
    let c = &s.last_commitment;
    let g = &f.last_commitment;
    assert_eq!(
        (
            c.seq,
            c.last_block_height,
            c.accounts_root,
            c.account_count,
            c.escape_total
        ),
        (
            g.seq,
            g.last_block_height,
            g.accounts_root,
            g.account_count,
            g.escape_total
        )
    );
    assert_eq!(
        (
            c.withdrawals_root,
            c.withdrawal_count,
            c.withdrawals_total,
            c.inbox_through,
            c.inbox_acc
        ),
        (
            g.withdrawals_root,
            g.withdrawal_count,
            g.withdrawals_total,
            g.inbox_through,
            g.inbox_acc
        )
    );
}

#[test]
fn the_state_frame_is_the_m0_state_header() {
    let (valid, _) = cases("state.json");
    for (name, bytes) in &valid {
        check_state(name, bytes);
    }
    // The genesis state of both perps lane files, and every state the golden workload leaves in its store.
    let cfg = caravel_testkit::lane::config().encode().unwrap();
    let genesis = caravel_perps_genesis(&cfg);
    check_state("genesis", &genesis);
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("lane.sqlite");
    std::fs::copy(
        root().join("platform/crates/caravel-runtime/tests/fixtures/m0-sequencer.sqlite"),
        &db,
    )
    .unwrap();
    let config_hash = cc_sha256(&cfg);
    let store = caravel_runtime::store::Store::open(
        &db,
        &caravel_testkit::lane::config().lane_id,
        &config_hash,
        &genesis,
    )
    .unwrap();
    let (height, head) = store.head().unwrap();
    assert_eq!(height, 70);
    check_state("head", &head);
    for seq in 0..=7u64 {
        let (_, snap) = store.snapshot(seq).unwrap().unwrap();
        check_state(&format!("snapshot {seq}"), &snap);
    }
}

fn cc_sha256(b: &[u8]) -> [u8; 32] {
    use cc::merkle::Sha256;
    cc::merkle::NativeSha256.hash(b)
}

fn caravel_perps_genesis(config: &[u8]) -> Vec<u8> {
    caravel_testkit::NativeExecutor
        .genesis(config)
        .expect("genesis")
}

use caravel_testkit::Executor as _;

#[test]
fn the_golden_trace_reads_generically() {
    let trace: Value = serde_json::from_str(
        &std::fs::read_to_string(
            root().join("platform/crates/caravel-runtime/tests/golden/m0-trace.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let blocks = trace["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 70);
    for b in blocks {
        let record = unhex(b["record_hex"].as_str().unwrap());
        let strict = m0::block::BlockRecordV1::decode(&record).unwrap();
        let generic = cc::block::BlockRecordV1::decode(&record).unwrap();
        assert_eq!(generic.encode(), record);
        assert_eq!(strict.state_hash_after, generic.state_hash_after);
        check_block(&strict.input);
        let receipts = unhex(b["receipts_hex"].as_str().unwrap());
        let strict = m0::receipts::Receipts::decode(&receipts).unwrap();
        check_receipts(&format!("block {}", b["height"]), &receipts, &strict);
        if let Some(c) = b.get("checkpoint") {
            let header = unhex(c["header_hex"].as_str().unwrap());
            let s = m0::checkpoint::CheckpointHeaderV1::decode(&header).unwrap();
            let g = cc::checkpoint::CheckpointHeaderV1::decode(&header).unwrap();
            assert_eq!(s.encode(), g.encode());
            assert_eq!(&g.encode()[..], &header[..]);
        }
    }
}

#[test]
fn merkle_matches_the_frozen_crate() {
    let h = caravel_merkle::NativeSha256;
    let g = cc::merkle::NativeSha256;
    for n in [0usize, 1, 2, 3, 5, 8, 33, 1024] {
        let leaves: Vec<[u8; 32]> = (0..n)
            .map(|i| cc_sha256(&(i as u64).to_le_bytes()))
            .collect();
        let a = caravel_merkle::root(&h, &leaves).unwrap();
        let b = cc::merkle::root(&g, &leaves).unwrap();
        assert_eq!(a, b, "root n={n}");
        for i in [0, n / 2, n.saturating_sub(1)] {
            if i < n {
                let pa = caravel_merkle::proof(&h, &leaves, i as u32).unwrap();
                let pb = cc::merkle::proof(&g, &leaves, i as u32).unwrap();
                assert_eq!(pa, pb);
                assert!(cc::merkle::verify(
                    &g, &leaves[i], i as u32, n as u32, &pb, &b
                ));
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Flip bytes in valid blocks: whatever the strict decoder still accepts,
    /// the generic one accepts too and re-encodes byte for byte.
    #[test]
    fn strict_valid_implies_generic_valid(which in 0usize..3, flips in prop::collection::vec((any::<usize>(), any::<u8>()), 1..4)) {
        let (valid, _) = cases("block_input.json");
        let mut bytes = valid[which % valid.len()].1.clone();
        for (at, v) in flips {
            let n = bytes.len();
            bytes[at % n] ^= v;
        }
        check_block(&bytes);
        if let Ok(tx_block) = m0::block::BlockInputV1::decode(&bytes) {
            for e in &tx_block.entries {
                if let m0::block::Entry::User(tx) = e {
                    prop_assert!(cc::tx::TxEnvelopeV1::decode(&tx.encode()).is_ok());
                }
            }
        }
    }
}
