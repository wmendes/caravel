# Platform test vectors

Golden vectors for the formats every lane shares (spec §9, DEC-052), used by
the platform's Rust and TypeScript tests:

| File | Format |
|---|---|
| `inbox_msg.json` | `InboxMsgV1` and the inbox accumulator |
| `checkpoint_header.json` | `CheckpointHeaderV1` |
| `step_envelope.json` | `CVSTEP01` |
| `merkle.json` | Merkle roots and proofs |
| `signatures.json` | ed25519 `verify_strict` cases |
| `hashes.json` | hash preimages (lane id, leaves, signers, rotation, SEP-53) |
| `app_genesis.json` | `AppGenesisV1`, the SDK apps' genesis config (M0.5, spec §20.4.1) |
| `sdk_state.json` | the SDK state layout, with the blocks and receipts between states (§20.4.2) |

The first six are byte-for-byte copies of the M0 files in
`lanes/perps/engine/test-vectors/`, where the frozen perps engine keeps them
(DEC-051). `lanes/perps/node/tests/format_compat.rs` checks that the copies are
identical. App formats (transaction bodies, state, receipts events, oracle
updates, scenarios) stay with each lane. The SDK files are generated and
checked by `platform/crates/caravel-app-sdk/tests/vectors.rs`; they are frozen
(DEC-060).
