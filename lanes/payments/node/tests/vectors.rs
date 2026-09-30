//! Golden vectors for the payments formats (spec §20.4.4):
//! `lanes/payments/test-vectors/payments.json`: the params, the `TRANSFER`
//! body and event, and a short run's states, blocks and receipts. They freeze
//! the formats: a change is a format change (§0.4). `UPDATE_VECTORS=1` writes
//! them; never for an existing vector without a version bump.

mod common;

use std::path::PathBuf;

use caravel_core::receipts::EventV1;
use caravel_core::tx::{SigScheme, StandardBody};
use caravel_harness::{pk, sha256, Lane, USDC};
use caravel_payments::{
    Params, Payments, Transfer, TransferEvent, EVENT_TRANSFER, PERM_TRANSFER, TRANSFER,
};
use common::*;
use serde_json::{json, Value};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn check(name: &str, v: &Value) {
    let text = serde_json::to_string_pretty(v).unwrap() + "\n";
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../test-vectors")
        .join(name);
    if std::env::var_os("UPDATE_VECTORS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{name} (UPDATE_VECTORS=1 to create)"));
    assert!(
        text == want,
        "{name} changed: the payments formats are frozen (spec §20.4.4)"
    );
}

#[test]
fn payments_vectors() {
    let params = Params {
        transfer_fee: FEE,
        min_transfer: 1,
    };
    let tr = Transfer {
        to: pk(B),
        amount: 10 * USDC,
        memo: 0x0102_0304_0506_0708,
    };
    let body = tr.encode();
    assert_eq!(Transfer::decode(&body), Some(tr));
    assert_eq!(body.len(), 56);
    let event = TransferEvent {
        from_idx: 1,
        to_idx: 2,
        amount: 10 * USDC,
        fee: FEE,
        memo: 7,
    };
    let mut fields = Vec::new();
    fields.extend(event.from_idx.to_le_bytes());
    fields.extend(event.to_idx.to_le_bytes());
    fields.extend(event.amount.to_le_bytes());
    fields.extend(event.fee.to_le_bytes());
    fields.extend(event.memo.to_le_bytes());
    assert_eq!(
        TransferEvent::decode(&EventV1 {
            type_id: EVENT_TRANSFER,
            fields: fields.clone()
        }),
        Some(event)
    );

    // A short run: deposits; transfers, a session key and a withdrawal; a
    // session-key transfer, a rejection and the checkpoint.
    let mut l = lane(FEE);
    let genesis_state = l.state_bytes.clone();
    let mut runs = Vec::new();
    let mut record = |l: &mut Lane<Payments>, name: &str, checkpoint: bool| {
        let b = l.build(checkpoint).encode().unwrap();
        l.execute_bytes(&b).unwrap();
        runs.push(json!({
            "name": name,
            "block": hex(&b),
            "receipts": hex(&l.receipts.encode().unwrap()),
            "state": hex(&l.state_bytes),
            "state_hash": hex(&l.state_hash()),
        }));
    };
    l.deposit(A, 100 * USDC);
    l.deposit(B, 50 * USDC);
    record(&mut l, "deposits", false);
    transfer(&mut l, A, A, B, 10 * USDC);
    let expires_at_ms = l.now + 3_600_000;
    l.standard(
        A,
        StandardBody::AddSessionKey {
            session_key: pk(K),
            expires_at_ms,
            permissions: PERM_TRANSFER,
        },
    );
    l.withdraw(B, 5 * USDC);
    record(&mut l, "transfer_session_key_withdraw", false);
    l.tx_as(
        K,
        A,
        SigScheme::RawEd25519,
        TRANSFER,
        Transfer {
            to: pk(B),
            amount: USDC,
            memo: 9,
        }
        .encode(),
    );
    transfer(&mut l, B, B, C, USDC);
    record(
        &mut l,
        "session_transfer_unknown_recipient_checkpoint",
        true,
    );
    assert_eq!(l.balance(T), 2 * FEE);
    check_wasm(&l);

    check(
        "payments.json",
        &json!({
            "format": "Caravel Payments 0.1.0 (template payments, state CVSTPAY1)",
            "spec": "§20.4.4",
            "hash_rule": "state_hash = H(state bytes); each state is step(previous state, block); config_hash = H(config)",
            "params": { "transfer_fee": FEE.to_string(), "min_transfer": "1", "hex": hex(&params.encode()) },
            "transfer_body": {
                "kind": TRANSFER,
                "fields": { "to": hex(&tr.to), "amount": tr.amount.to_string(), "memo": tr.memo.to_string() },
                "hex": hex(&body),
            },
            "transfer_event": {
                "type_id": EVENT_TRANSFER,
                "fields": { "from_idx": 1, "to_idx": 2, "amount": event.amount.to_string(), "fee": FEE.to_string(), "memo": "7" },
                "hex": hex(&fields),
            },
            "invalid_bodies": [
                { "name": "short", "hex": hex(&body[..55]) },
                { "name": "long", "hex": hex(&[body.clone(), vec![0]].concat()) },
            ],
            "run": {
                "config": hex(&l.config.encode().unwrap()),
                "config_hash": hex(&sha256(&l.config.encode().unwrap())),
                "genesis_state": hex(&genesis_state),
                "genesis_state_hash": hex(&sha256(&genesis_state)),
                "blocks": runs,
            },
        }),
    );
    assert!(Transfer::decode(&body[..55]).is_none());
    assert!(Transfer::decode(&[body, vec![0]].concat()).is_none());
}
