//! P-01 (M0.5 platform split): golden snapshots of the node's JSON.
//!
//! - The API builders (`checkpoint_json`, `block_json`, `withdrawal_proofs`,
//!   `escape_proof`) over the fixture store that
//!   `lanes/perps/node/tests/golden.rs` leaves behind, with checkpoints 1 to 5
//!   marked accepted.
//! - `caravel-perps-node genesis` for both perps lane files.
//!
//! The split must give the same JSON (plan P-06). `UPDATE_GOLDEN=1` rewrites
//! the files; never on a split branch.

use std::path::{Path, PathBuf};

use axum::response::{IntoResponse, Response};
use caravel_node::api;
use caravel_node::lane_toml::{self, LaneFile};
use caravel_perps_node::PerpsApp;
use caravel_runtime::checkpoint::sha256;
use caravel_runtime::store::Store;
use caravel_testkit::lane::{config, seeds};
use caravel_types::vectors::pk;
use serde_json::{json, Value};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn updating() -> bool {
    std::env::var_os("UPDATE_GOLDEN").is_some()
}

fn check(name: &str, value: &Value) {
    let text = serde_json::to_string_pretty(value).unwrap() + "\n";
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if updating() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let want =
        std::fs::read_to_string(&path).expect("golden file (UPDATE_GOLDEN=1 on the M0 code)");
    if text != want {
        let line = text
            .lines()
            .zip(want.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(0);
        panic!(
            "{name} changed at line {}: got {:?}, want {:?}",
            line + 1,
            text.lines().nth(line),
            want.lines().nth(line)
        );
    }
}

async fn body(r: api::ApiResult) -> Value {
    let resp: Response = r.unwrap_or_else(|e| e.into_response());
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
        .await
        .unwrap();
    json!({ "status": status, "body": serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null) })
}

#[tokio::test]
async fn m0_api_json_is_unchanged() {
    let bytes = config().encode().unwrap();
    let state = caravel_perps::genesis(&bytes, &caravel_perps::native::NativeCrypto).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("lane.sqlite");
    std::fs::copy(
        root().join("lanes/perps/node/tests/fixtures/m0-sequencer.sqlite"),
        &db,
    )
    .unwrap();
    let mut store = Store::open(&db, &config().lane_id, &sha256(&bytes), &state).unwrap();
    for seq in 1..=5u64 {
        store.set_signed(seq, 1, "[]").unwrap();
        store
            .set_accepted(seq, &format!("{:064x}", seq), 1_000 + seq as u32)
            .unwrap();
    }
    let height = store.head().unwrap().0;
    let mut out = json!({ "height": height });
    let mut checkpoints = Vec::new();
    for seq in 0..=8u64 {
        checkpoints.push(body(api::checkpoint_json(&store, seq)).await);
    }
    out["checkpoints"] = Value::Array(checkpoints);
    let mut blocks = Vec::new();
    for h in [0u64, 1, 2, 3, 10, 13, 44, 70, 71] {
        blocks.push(body(api::block_json(&PerpsApp, &store, h)).await);
    }
    out["blocks"] = Value::Array(blocks);
    let mut withdrawals = Vec::new();
    let mut escapes = Vec::new();
    for s in [seeds::A, seeds::B, seeds::C, seeds::D] {
        withdrawals.push(body(api::withdrawal_proofs(&store, &pk(s))).await);
        escapes.push(body(api::escape_proof(&PerpsApp, &store, &pk(s), Some(5))).await);
    }
    escapes.push(body(api::escape_proof(&PerpsApp, &store, &pk(seeds::A), None)).await);
    out["withdrawal_proofs"] = Value::Array(withdrawals.clone());
    out["escape_proofs"] = Value::Array(escapes.clone());
    check("m0-api.json", &out);

    // export-proofs gives every account's exit at once: the same proofs the
    // per-account routes give, each verifying against its header.
    let exits = api::exit_proofs(&PerpsApp, &store, 5).unwrap();
    let header = |seq: u64| {
        caravel_core::checkpoint::CheckpointHeaderV1::decode(
            &store.checkpoint(seq).unwrap().unwrap().header,
        )
        .unwrap()
    };
    let h5 = header(5);
    assert_eq!(exits["seq"], "5");
    assert_eq!(
        exits["header_hash"],
        caravel_runtime::sequencer::hex(&sha256(&h5.encode()))
    );
    let escape = exits["escape"].as_array().unwrap();
    assert_eq!(escape.len() as u32, h5.account_count);
    let mut total = 0i128;
    for e in escape {
        let (index, equity) = (
            e["index"].as_u64().unwrap() as u32,
            e["equity"].as_str().unwrap().parse::<i128>().unwrap(),
        );
        let key = caravel_runtime::views::parse_g(e["account"].as_str().unwrap()).unwrap();
        let leaf = sha256(&caravel_core::preimage::account_leaf_preimage(
            &h5.lane_id,
            5,
            index,
            &key,
            equity,
        ));
        assert!(
            verifies(
                &leaf,
                index,
                h5.account_count,
                &e["proof"],
                &h5.accounts_root
            ),
            "{e}"
        );
        total += equity;
    }
    assert_eq!(total, h5.escape_total);
    for (seed, one) in [seeds::A, seeds::B, seeds::C, seeds::D]
        .iter()
        .zip(&escapes)
    {
        let mine = escape
            .iter()
            .find(|e| e["account"] == caravel_runtime::views::g_address(&pk(*seed)));
        match mine {
            Some(mine) => {
                assert_eq!(one["status"], 200);
                assert_eq!(mine["proof"], one["body"]["proof"]);
                assert_eq!(mine["equity"], one["body"]["equity"]);
            }
            None => assert_eq!(one["status"], 404, "{one}"),
        }
    }
    let all_withdrawals: usize = withdrawals
        .iter()
        .map(|w| w["body"]["withdrawals"].as_array().unwrap().len())
        .sum();
    let listed = exits["withdrawals"].as_array().unwrap();
    assert!(listed.len() >= all_withdrawals && !listed.is_empty());
    for w in listed {
        let seq: u64 = w["seq"].as_str().unwrap().parse().unwrap();
        let h = header(seq);
        let (index, amount) = (
            w["index"].as_u64().unwrap() as u32,
            w["amount"].as_str().unwrap().parse::<i128>().unwrap(),
        );
        let key = caravel_runtime::views::parse_g(w["account"].as_str().unwrap()).unwrap();
        let leaf = sha256(&caravel_core::preimage::withdrawal_leaf_preimage(
            &h.lane_id, seq, index, &key, amount,
        ));
        assert!(
            verifies(
                &leaf,
                index,
                h.withdrawal_count,
                &w["proof"],
                &h.withdrawals_root
            ),
            "{w}"
        );
    }
}

fn verifies(leaf: &[u8; 32], index: u32, n: u32, proof: &Value, root: &[u8; 32]) -> bool {
    let siblings: Vec<[u8; 32]> = proof
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            caravel_runtime::sequencer::unhex(p.as_str().unwrap())
                .unwrap()
                .try_into()
                .unwrap()
        })
        .collect();
    caravel_core::merkle::verify(
        &caravel_core::merkle::NativeSha256,
        leaf,
        index,
        n,
        &siblings,
        root,
    )
}

#[test]
fn m0_genesis_reports_are_unchanged() {
    for (name, file) in [
        (
            "genesis-local.json",
            "lanes/perps/config/lane.caravel-perps.local.toml",
        ),
        (
            "genesis-testnet.json",
            "lanes/perps/config/lane.caravel-perps.testnet.toml",
        ),
    ] {
        let lane = LaneFile::load(&root().join(file)).unwrap();
        let (report, config_bytes, state) = lane_toml::genesis(&PerpsApp, &lane).unwrap();
        check(
            name,
            &json!({
                "report": report,
                "config_hex": caravel_runtime::sequencer::hex(&config_bytes),
                "state_hash": caravel_runtime::sequencer::hex(&sha256(&state)),
            }),
        );
    }
}
