//! Golden vectors for the SDK formats (spec §20.4, DEC-060):
//! `platform/test-vectors/app_genesis.json` and `sdk_state.json`. They freeze
//! the formats: a change here is a format change (§0.4). `UPDATE_VECTORS=1`
//! writes them; never for an existing vector without a version bump.

mod common;

use std::path::PathBuf;

use caravel_app_sdk::testapp::TestApp;
use caravel_app_sdk::{template_id, AccessMode, AppGenesisV1, SdkState};
use caravel_core::tx::StandardBody;
use common::*;
use serde_json::{json, Value};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-vectors")
}

fn check(name: &str, v: &Value) {
    let text = serde_json::to_string_pretty(v).unwrap() + "\n";
    let path = dir().join(name);
    if std::env::var_os("UPDATE_VECTORS").is_some() {
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{name} (UPDATE_VECTORS=1 to create)"));
    assert!(
        text == want,
        "{name} changed: the SDK formats are frozen (spec §20.4)"
    );
}

fn genesis_vector(name: &str, g: &AppGenesisV1) -> Value {
    let bytes = g.encode().unwrap();
    assert_eq!(&AppGenesisV1::decode(&bytes).unwrap(), g);
    json!({
        "name": name,
        "fields": {
            "lane_id": hex(&g.lane_id),
            "template": String::from_utf8(g.template.iter().copied().take_while(|b| *b != 0).collect()).unwrap(),
            "template_version": g.template_version.to_string(),
            "system_keys": g.system_keys.iter().map(|k| hex(k)).collect::<Vec<_>>(),
            "access_mode": (g.access_mode as u8).to_string(),
            "allowlist": g.allowlist.iter().map(|k| hex(k)).collect::<Vec<_>>(),
            "min_deposit": g.min_deposit.to_string(),
            "min_withdrawal": g.min_withdrawal.to_string(),
            "max_accounts": g.max_accounts.to_string(),
            "max_session_keys": g.max_session_keys.to_string(),
            "max_txs_per_account_per_block": g.max_txs_per_account_per_block.to_string(),
            "max_entries_per_block": g.max_entries_per_block.to_string(),
            "max_block_bytes": g.max_block_bytes.to_string(),
            "max_pending_withdrawals": g.max_pending_withdrawals.to_string(),
            "exec_cpu_limit": g.exec_cpu_limit.to_string(),
            "exec_mem_limit": g.exec_mem_limit.to_string(),
            "app_params": hex(&g.app_params),
        },
        "hex": hex(&bytes),
        "hash": hex(&sha256(&bytes)),
    })
}

#[test]
fn app_genesis_vectors() {
    let open = config();
    let mut allowlist = config();
    allowlist.template = template_id("payments");
    allowlist.system_keys = vec![pk(0x21)];
    allowlist.access_mode = AccessMode::Allowlist;
    let mut keys = vec![pk(0xA1), pk(0xB2), pk(0xC3)];
    keys.sort();
    allowlist.allowlist = keys;
    // Payments' 32-byte params (spec §20.4.4): transfer_fee = 0.01 USDC, min_transfer = 1 stroop.
    allowlist.app_params = [100_000i128.to_le_bytes(), 1i128.to_le_bytes()].concat();
    let mut bad_magic = open.encode().unwrap();
    bad_magic[7] = b'2';
    let mut trailing = open.encode().unwrap();
    trailing.push(0);
    check(
        "app_genesis.json",
        &json!({
            "format": "AppGenesisV1",
            "spec": "§20.4.1",
            "hash_rule": "hash = config_hash = H(AppGenesisV1 bytes)",
            "vectors": [genesis_vector("testapp_open", &open), genesis_vector("payments_allowlist", &allowlist)],
            "invalid": [
                { "name": "bad_magic", "hex": hex(&bad_magic), "error": "BadMagic" },
                { "name": "trailing_byte", "hex": hex(&trailing), "error": "TrailingBytes" },
            ],
        }),
    );
    assert!(AppGenesisV1::decode(&bad_magic).is_err() && AppGenesisV1::decode(&trailing).is_err());
}

#[test]
fn sdk_state_vectors() {
    let mut lane = Lane::new(config());
    let genesis_state = lane.state.clone();
    let (da, db) = (
        lane.deposit(0xA1, 100 * USDC),
        lane.deposit(0xB2, 50 * USDC),
    );
    let block1 = lane.block_bytes(vec![da.clone(), db.clone()], false);
    lane.run(vec![da, db], false).unwrap();
    let after1 = lane.state.clone();
    let receipts1 = lane.last.as_ref().unwrap().receipts.clone();
    let n = lane.nonce(0xA1);
    let entries = vec![
        lane.owner(
            0xA1,
            StandardBody::AddSessionKey {
                session_key: pk(0x5E),
                expires_at_ms: lane.now + 3_600_000,
                permissions: 1,
            },
        ),
        lane.count(0xA1, 0xA1, n + 1, 5),
        lane.owner_with_nonce(0xA1, n + 2, StandardBody::Withdraw { amount: 10 * USDC }),
    ];
    let block2 = lane.block_bytes(entries.clone(), true);
    lane.run(entries, true).unwrap();
    let after2 = lane.state.clone();
    let receipts2 = lane.last.as_ref().unwrap().receipts.clone();
    for s in [&genesis_state, &after1, &after2] {
        assert_eq!(
            &SdkState::decode(s, b"CVSTTST1").unwrap().encode().unwrap(),
            s
        );
    }
    let v = |name: &str, state: &[u8], block: Option<&[u8]>, receipts: Option<&[u8]>| {
        json!({
            "name": name,
            "fields": {
                "block": block.map(hex),
                "receipts": receipts.map(hex),
            },
            "hex": hex(state),
            "hash": hex(&sha256(state)),
        })
    };
    check(
        "sdk_state.json",
        &json!({
            "format": "SDK state (the test app, CVSTTST1)",
            "spec": "§20.4.2",
            "hash_rule": "hash = state_hash = H(state bytes); each state after the first is step(previous state, block)",
            "context": { "config_hex": hex(&lane.config_bytes), "config_hash": hex(&lane.config_hash) },
            "vectors": [
                v("genesis", &genesis_state, None, None),
                v("two_deposits", &after1, Some(&block1), Some(&receipts1)),
                v("session_key_count_withdraw_checkpoint", &after2, Some(&block2), Some(&receipts2)),
            ],
        }),
    );
    let _ = TestApp;
}
