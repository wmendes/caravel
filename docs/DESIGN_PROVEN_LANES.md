# Proven lanes: validity proofs for Caravel lanes

Design note and spike results, 2026-10-07 (M0.12 X-01). This is an internal record, not a claim. The spike is in [`spikes/zk-payments`](../spikes/zk-payments). Your decisions are listed at the end.

## Why

A lane's checkpoint is accepted on Stellar today with a validator signature threshold (2 of 3 on lane #1, whose validators all run on one VM). The settlement contract checks the chain links, the batch hash, the inbox and solvency. It *trusts* the header's `state_hash`, `accounts_root`, `escape_total` and `withdrawals_root`. A quorum that colludes with the sequencer can sign a wrong state. Replay detects it but doesn't prevent it (spec §2.2, §4.3). In the L2 market, a signer committee like this counts as a sidechain, not a rollup.

A validity proof closes that gap. The contract accepts a checkpoint only with a proof that running the lane's engine over the batch (which is on Stellar) takes the last accepted state to this header. Stellar can check that now:
- Protocol 28 is live on testnet and mainnet;
- BN254 pairing checks (CAP-74, Protocol 25), BN254 MSM (CAP-80, Protocol 26) and BLS12-381 (CAP-59) are all available;
- RISC Zero's Groth16 wrap verifies in a Soroban contract.

Prior art: `kalepail/kalien` proves deterministic game tapes with RISC Zero and checks them on Stellar, and NethermindEth's verifier is the one used here. Spec §21 already planned this as T-M2-01. There is no appchain or rollup platform on Stellar that does it.

## What the spike proves

**The statement.** The guest takes the previous header (442 B), that checkpoint's state, and the batch (`BatchV1`, the bytes posted to Stellar). It:
1. checks `sha256(state) == prev.state_hash`;
2. runs `caravel_app_sdk::step::<Payments>` over every block, checking each block's `state_hash_after`;
3. builds the header as `caravel_runtime::checkpoint::assemble` does;
4. commits the journal `H(prev_header) ‖ H(header)`.

The engine's crypto is already behind a `Crypto` trait. The guest supplies the zkVM's SHA-256 and RISC Zero's accelerated curve25519 (`verify_strict`, panicking like the Wasm traps). No engine code changes.

**The contract** (`proven-checkpoint`, about 80 lines):
1. checks `prev_header_hash == last` and `sha256(batch) == batch_hash`, which are today's checks 4 and 5;
2. rebuilds the journal from `last` and `sha256(header)`;
3. calls the verifier with the lane's image ID, in place of check 7, the signatures.

A proof for any other transition or header doesn't verify.

## Results

| | Result |
|---|---|
| Guest header vs the node's | Byte-identical: the journal equals `H(prev) ‖ H(assemble(...))` on every run |
| Tampered batch (one byte) | The guest refuses it: `ed25519 verification failed` |
| On testnet, a valid checkpoint | Accepted: **32.8M CPU instructions** (8% of 400M), 0.0048 XLM resource fee, [tx 72019ffd…](https://stellar.expert/explorer/testnet/tx/72019ffdc77c705243e44111ab142d31f99efc7d28de82b5fc1696a4d10b980b) |
| Verifier alone | 32.2M instructions, 0.0036 XLM, [tx 7347f127…](https://stellar.expert/explorer/testnet/tx/7347f1270e8f931f329d7a0f2a901e4d951ffd7060eeadf1fb18736571f35595) |
| On testnet, attacks | Tampered batch: `BadBatchHash`. A header whose `state_hash` was changed: the verifier refuses. Replay: `BadPrevHeader`. A flipped journal byte or a wrong image ID: refused. |

**Prover cost.** User cycles in the guest. `exec` counts these without proving.

| Batch | User cycles | Per unit |
|---|---:|---|
| 16 accounts, 120 empty blocks | 5.8M | ~48k per empty block |
| 64 accounts, 20 blocks, 200 transfers | 183.6M | ~0.9M per transfer |
| 16 accounts, 20 blocks, 200 transfers | 181.4M | the same: state size barely matters |
| 128 accounts, 4 full blocks (180 transfers) | 164.1M | ~0.9M per transfer |

**Where the cycles go.** In the 200-transfer batch, ed25519 is 178.6M of 183.7M cycles (97%), about **893k cycles per signature** even with the accelerated curve. SHA-256 is 0.46M over 452 calls.

**Proving time on this machine** (Apple M5 Pro, CPU). RISC Zero 3.0.6 proved on the CPU: all 15 cores busy, and no speedup with the `metal` feature.
- One 1M-cycle segment takes about **55 s**, roughly 19k cycles a second.
- A Groth16 proof of a one-segment batch takes **236 s** end to end, of which about 180 s is recursion and the Groth16 wrap.

## Go / no-go

- **On-chain verification: go.** 32.8M instructions per checkpoint against the 25% (100M) bar. The fee is about 0.005 XLM, and it doesn't grow with the batch.
- **Proving: not on a laptop CPU.**
  - Idle at 500 ms blocks needs ~96k cycles a second; this CPU gives 19k.
  - Each user transaction needs ~0.9M cycles, so this CPU proves about one every 47 s.
  - A CUDA GPU is the real target; kalien proves on rented GPUs. **The next number to get is one GPU's rate**, which costs money, so it waits for your OK.

## Design, if it's a go

1. **Contract v3.** Check 7 becomes "verify the proof of `H(LastCkpt) → H(header)` for the lane's image ID". Every other check stays. The contract then knows `state_hash` and the roots instead of trusting them.
2. **No empty blocks on proven lanes.** Payments has no feeds, so a lane that makes blocks only when there is something to include costs nothing to prove while idle. Latency for real transactions stays at the block time.
3. **A prover node role.** It takes each sealed checkpoint from the sequencer, proves it on a GPU or a proving service, and gives the seal to the relayer.
4. **Latency.** A proof takes minutes, against 7 s from a withdrawal to claimable today. The recommendation is to keep the signed fast path for acceptance, and require the proof before a checkpoint's withdrawals become claimable or before it counts for escapes. Users keep today's speed, and funds move only on proven state.
5. **The lane file:** `[env.X] settlement = "signatures" | "proofs"`. This is where pluggability pays off for lane creators.
6. **DEC-002.** The proof covers the engine's Rust compiled for RISC-V, not the Wasm.
   - The parity gate grows to native = Wasm = guest.
   - The guest needs a cycle cap that mirrors the Wasm budget (DEC-015), so that a block the Wasm accepts can always be proven.
7. **Signature cost** is the lever. Options to measure before choosing:
   - ed25519 batch verification (cheaper, but its semantics must match `verify_strict`);
   - a zkVM with an ed25519 precompile (SP1);
   - passkeys (secp256r1, which RISC Zero accelerates). That fits Stellar smart wallets and kalepail's passkey-kit.

**Honest claims after this:**
- **What we could say:** "validity proven on Stellar".
- **What we still couldn't say:** "trustless". That rests on the zkVM's soundness, the Groth16 setup, an unaudited verifier (NethermindEth says so), the sequencer's liveness (forced inclusion and escape stay), and the testnet admin keys.

## Consensus, parked

With proofs, a lane's consensus no longer protects funds. It only decides liveness and censorship, which already have backstops: forced inclusion through the inbox, and freeze then escape. A BFT committee (Tendermint via Malachite or CometBFT) would need at least 4 validators, peer-to-peer networking and more latency, a burden for the teams Caravel serves.

The cheaper answer to "no single point of failure" is a standby sequencer that follows the leader and takes over when it fails. Sequencer-signed blocks fence the old leader out. Build it when a lane needs it.

## Decisions for you

1. Rent one CUDA GPU for an hour to measure proving speed, the go/no-go number.
2. Proofs only, or the hybrid in point 4.
3. Signature path: ed25519 as today, or measure the options in point 7 first.
4. The trust-model and claims changes (spec §0.4), when a proven lane ships.
5. Share this note with kalepail.
