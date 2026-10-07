//! Proves one Caravel Payments checkpoint: from the previous checkpoint's
//! header and state, running the engine over every block of the batch gives
//! this header. The journal is `H(prev_header) ‖ H(header)` (64 bytes), which
//! the settlement contract can rebuild from `LastCkpt.header_hash` and the
//! header it is given.
//!
//! Inputs, each as `len u32 LE ‖ bytes` padded to 4: the header ids
//! (network_id ‖ settlement_addr_hash ‖ engine_wasm_hash, 96 B), the previous
//! header (442 B), the previous checkpoint's state, and the batch (`BatchV1`).
//! A std guest: RISC Zero's accelerated curve25519 needs std.
#![no_main]

use caravel_core::batch::BatchV1;
use caravel_core::block::{block_hash_preimage, BlockInputV1};
use caravel_core::checkpoint::CheckpointHeaderV1;
use caravel_core::state::StateFrameV1;
use ed25519_dalek::{Signature, VerifyingKey};
use risc0_zkvm::guest::env;
use risc0_zkvm::sha::{Impl, Sha256};

risc0_zkvm::guest::entry!(main);

fn sha256(data: &[u8]) -> [u8; 32] {
    let d = Impl::hash_bytes(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(d.as_bytes());
    out
}

/// The engine's crypto, as `NativeCrypto` but with the zkVM's SHA-256 and
/// accelerated curve25519: `verify_strict`, panicking like the Wasm traps.
struct ZkCrypto;

impl caravel_app_sdk::Crypto for ZkCrypto {
    fn sha256(&self, data: &[u8]) -> [u8; 32] {
        sha256(data)
    }

    fn ed25519_verify(&self, public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]) {
        let ok = VerifyingKey::from_bytes(public_key).is_ok_and(|k| {
            k.verify_strict(message, &Signature::from_bytes(signature))
                .is_ok()
        });
        assert!(ok, "ed25519 verification failed");
    }
}

fn read_bytes() -> Vec<u8> {
    let mut len = [0u8; 4];
    env::read_slice(&mut len);
    let len = u32::from_le_bytes(len) as usize;
    let mut buf = vec![0u8; (len + 3) & !3];
    env::read_slice(&mut buf);
    buf.truncate(len);
    buf
}

fn main() {
    let ids = read_bytes();
    let prev_header_bytes = read_bytes();
    let mut state = read_bytes();
    let batch_bytes = read_bytes();
    assert_eq!(ids.len(), 96, "ids");

    // 1. The state is the previous checkpoint's.
    let prev = CheckpointHeaderV1::decode(&prev_header_bytes).expect("previous header");
    assert_eq!(sha256(&state), prev.state_hash, "state is not the previous checkpoint's");

    // 2. Every block, as validators re-execute it.
    let batch = BatchV1::decode(&batch_bytes).expect("batch");
    let first = batch.blocks.first().expect("a block");
    let last = batch.blocks.last().expect("a block");
    for record in &batch.blocks {
        let out = caravel_app_sdk::step::<caravel_payments::Payments, _>(
            &state,
            &record.input,
            &ZkCrypto,
        )
        .expect("block executes");
        assert_eq!(sha256(&out.state), record.state_hash_after, "state_hash_after");
        state = out.state;
    }

    // 3. The header, as `caravel_runtime::checkpoint::assemble` builds it.
    let frame = StateFrameV1::read(&state).expect("state frame");
    let c = frame.last_commitment;
    let first_input = BlockInputV1::decode(&first.input).expect("first block");
    let last_input = BlockInputV1::decode(&last.input).expect("last block");
    let state_hash = sha256(&state);
    assert!(
        last_input.checkpoint_end && last_input.height == c.last_block_height,
        "the batch ends at a checkpoint"
    );
    assert!(
        batch.lane_id == frame.lane_id && batch.checkpoint_seq == c.seq,
        "the batch is this lane's checkpoint"
    );
    let mut id = [[0u8; 32]; 3];
    for (k, slot) in id.iter_mut().enumerate() {
        slot.copy_from_slice(&ids[k * 32..(k + 1) * 32]);
    }
    let header = CheckpointHeaderV1 {
        lane_id: frame.lane_id,
        network_id: id[0],
        settlement_addr_hash: id[1],
        engine_wasm_hash: id[2],
        seq: c.seq,
        prev_header_hash: sha256(&prev_header_bytes),
        first_block_height: first_input.height,
        last_block_height: last_input.height,
        last_block_timestamp_ms: last_input.timestamp_ms,
        last_block_hash: sha256(&block_hash_preimage(&sha256(&last.input), &last.state_hash_after)),
        batch_hash: sha256(&batch_bytes),
        state_hash,
        accounts_root: c.accounts_root,
        account_count: c.account_count,
        escape_total: c.escape_total,
        withdrawals_root: c.withdrawals_root,
        withdrawal_count: c.withdrawal_count,
        withdrawals_total: c.withdrawals_total,
        inbox_through: c.inbox_through,
        inbox_acc: c.inbox_acc,
    };

    // 4. The journal: what the contract checks against `LastCkpt`.
    env::commit_slice(&header.prev_header_hash);
    env::commit_slice(&sha256(&header.encode()));
}
