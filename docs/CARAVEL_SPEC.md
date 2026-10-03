# Caravel Development Specification

Spec version: 0.1.0 · Written: 2026-09-29 · Status: M0 ready to build · Owner: Wlademyr Mendes

This file is the single source of truth for building Caravel. It is written for AI coding agents (Claude Code) and for humans reviewing their work. If code and this spec disagree, the spec wins until a human changes it. If this spec is wrong or silent, stop and ask (see §0.4).

---

## 0. How to use this document

### 0.1 Reading order for an agent

1. Read §0 (rules), §1 (what we are building), §4 (architecture) and §20 (plan) before writing code.
2. Before working on a task, read every section listed in that task's `Reads:` line.
3. Before touching a binary format, read §8 (determinism) and §9 (encodings). Formats are frozen once their golden vectors exist.

### 0.2 Conventions

- **MUST / MUST NOT / SHOULD / MAY** follow RFC 2119 meaning.
- IDs are stable and referenced from code comments and tests:
  - `REQ-xxx` requirement, `INV-xxx` invariant (must hold after every block or call), `DEC-xxx` decision (§22), `T-xxx` task (§20), `OQ-xxx` open question (§23).
- `[VERIFY]` marks a fact that can drift (versions, network limits, API names). Before relying on it, check the primary source named next to it and, if it changed, update this spec in the same change.
- All multi-byte integers in Caravel binary formats are **little-endian**. `bytes32` is 32 raw bytes. `i128` is 16-byte two's complement little-endian.
- Amounts are integers. There are no floating-point numbers anywhere in consensus code.
- `H(x)` means SHA-256 of the byte string `x`. `||` is byte concatenation. String literals inside `H(...)` are ASCII bytes with no terminator.

### 0.3 Golden rules for agents

1. **Deterministic replay is the product invariant.** Given the genesis config, the engine Wasm and the batches posted to Stellar, anyone MUST be able to recompute every state hash byte for byte. Never trade this away for speed or convenience.
2. **Never invent APIs.** For any crate or npm package, check the docs for the exact pinned version in `versions.json` (docs.rs for Rust, the package README/typedoc for npm) before calling a function. If an API named in this spec does not exist in the pinned version, find the equivalent, note it in §22 as a new DEC, and continue.
3. **Tests first for consensus code.** Every function in `caravel-types`, `caravel-merkle`, `caravel-perps` and both contracts gets unit tests; codecs get golden vectors; the engine gets property tests for the invariants in §11.9.
4. **Do not change a frozen format** (anything in §9 with golden vectors committed) without bumping its magic/version, regenerating vectors and recording a DEC.
5. **Keep claims honest.** Do not write UI copy, docs or comments that claim more than §2 allows.
6. **Pin and record versions** (protocol, crates, npm packages, Wasm hashes) in `versions.json` in every reproducibility-sensitive change.
7. **Small, reviewable commits.** One task (or sub-task) per branch/PR. Update the task's status in §20 in the same PR.
8. **No secrets in the repo.** Keys come from env vars or files listed in `.gitignore`.
9. **Testnet only** until M2 is complete and audited. Refuse to add mainnet network passphrases or mainnet contract IDs to defaults.

### 0.4 When to stop and ask the human

You MAY decide on your own, and record it as a DEC in §22, for implementation-level choices that leave the following unchanged: frozen formats, invariants, the trust model, contract checks and claims. Examples: a library choice, a substitute for an API name that does not exist, internal module layout.

Stop and ask (in the PR description or chat) when:
- a change would touch a frozen format, an `INV-*`, a settlement check in §13, the trust model in §4.3 or the claims in §2;
- a task depends on an `OQ-xxx` in §23 that is still open;
- a test in §19 cannot pass without changing a frozen format or an invariant;
- the pinned toolchain cannot build the contracts under the size limits in §3.3;
- you would need a new external service, paid account or key that is not listed.

---

## 1. Product summary

### 1.1 What Caravel is

Caravel is infrastructure as code for appchains on Stellar. Each appchain is called a **lane**.

- A **lane file** declares a lane and its deployments. `caravel plan` shows every change on Stellar and on the host before it is made, `caravel apply` makes it, and `caravel destroy` winds the lane down to a frozen contract and an exit proof for every account (§20.3).
- A lane runs Soroban contracts with its own settings: block time, fee model, validator set, access rules and execution limits.
- A lane settles to Stellar. User funds, in the lane's settlement token (a Stellar asset or a SEP-41 token, DEC-072), are held by a **settlement contract** on Stellar.
- The lane posts **checkpoints** to that contract. Each checkpoint:
  - carries the lane's full block data;
  - carries a state commitment;
  - is signed by the lane's validators.
- Withdrawals are paid on Stellar against a checkpoint, with a Merkle proof.
- If the lane stops or censors users, anyone can **freeze** the contract. Each user can then reclaim their last checkpointed equity from Stellar alone.

The first lane is **Caravel Perps**, a fully on-chain perpetual futures exchange:
- USDC collateral;
- central limit order book;
- 1-second blocks;
- no gas fees, only trading fees.

### 1.2 Milestone M0 (what this spec makes buildable now)

A public **testnet** demo of Caravel Perps. It has:

- a settlement contract on Stellar testnet: USDC vault, inbox, checkpoints, withdrawals, freeze, escape;
- a perps engine contract. Its exact Wasm is executed off-chain by the lane nodes through `soroban-env-host`, and it is also deployed on testnet;
- one sequencer and three independent validators. The validators re-execute every block and co-sign checkpoints, with a 2-of-3 weighted threshold;
- a relayer that bridges Stellar events and transactions;
- a replay CLI that rebuilds the lane from Stellar data alone and checks every checkpoint;
- a web app: connect a Stellar wallet (Stellar Wallets Kit since M0.5), deposit testnet USDC, trade 3 markets, withdraw, and use the escape hatch.

### 1.3 Non-goals for M0

- Mainnet, audits, real money.
- Validity proofs (ZK), fraud proofs, bonded or slashed validators, and the reward token. These are M1/M2 (§21).
- Running arbitrary third-party Soroban contracts on a lane (M1).
- Decentralized sequencing, and SCP consensus on the lane itself (M3).
- Forking stellar-core (see DEC-001).

---

## 2. Honest claims

This section governs README text, UI copy, pitch material generated from code, and code comments.

### 2.1 What M0 may claim

> Caravel Perps runs as a Soroban lane. Every lane block is executed with the exact perps-engine Wasm through `soroban-env-host`, by the sequencer and independently by three validators. Every checkpoint, including the full block data, is posted to Stellar and accepted only with a 2-of-3 validator signature. Anyone can rebuild the lane from Stellar data alone and check every state hash. USDC stays in a Stellar contract; withdrawals need a Merkle proof against an accepted checkpoint. If the lane stops checkpointing or ignores deposits and forced-withdrawal requests sent through Stellar, anyone can freeze the contract, and users reclaim their last checkpointed equity on Stellar. This is a testnet build: the contract admin can still upgrade it and rotate validators.

### 2.2 What M0 MUST NOT claim

- That Stellar validators execute lane blocks. They verify signatures and store data, and they do not re-run trades. This mirrors SoroDOOM decision D011.
- That the lane is trustless. If 2 of 3 validators collude with the sequencer, they can sign a wrong state. Replay detects this but does not prevent it.
- That lane blocks are Stellar transactions.
- Sub-second or 1-second blocks "on Stellar". The 1s cadence is the lane's, and Stellar mainnet ledgers stay ~5s.
- Full censorship resistance for traders:
  - a sequencer can process forced withdrawals, which only release free collateral, while ignoring a user's closing orders;
  - forced position closes through the inbox are M1 (T-M1-08).
- "Audited", "production", "mainnet", "first ever".

### 2.4 What the platform may claim (M0.5, approved by the human on 2026-09-30)

> Caravel deploys and runs appchains ("lanes") that settle to Stellar, from one lane file, each in the token it chooses: a Stellar asset (through its Stellar Asset Contract) or any SEP-41 token contract whose transfers move exact amounts. `caravel plan` shows every change on Stellar and on the host before it is made; `caravel apply` makes it; `caravel destroy` winds a lane down to a frozen contract and gives every account its exit proof. Each lane's engine is Soroban Wasm, run through `soroban-env-host` by the sequencer and independently by each validator. Every checkpoint, with its block data, is posted to Stellar and accepted only with the validators' threshold signature. Anyone can rebuild a lane from Stellar data alone. The token stays in the lane's settlement contract; if the lane stops checkpointing or ignores deposits and forced withdrawals sent through Stellar, anyone can freeze it, and users take their last checkpointed balance back on Stellar. The token's own rules still apply (an asset issuer's freeze or clawback reaches the contract's balance too). This is testnet software: a lane's admin can still upgrade its contract and rotate its validators, and lane #1's three validators all run on one machine operated by the Caravel team.

It MUST NOT claim, besides §2.2:
- that any token works: a token must move exact amounts on transfer (no transfer fees, no rebasing), and a template may need set decimals (perps: 7);
- one-click or zero-configuration lanes: a lane needs a lane file, Stellar identities and a host;
- hosting, cloud provisioning, or a lane registry or console: they were cut from M0.5 (§20.3);
- any comparison naming the well-known infrastructure-as-code tool: the human's copy rule, which CI enforces.

### 2.3 Known mismatches with the pitch materials (deck, landing page)

Do not implement from the deck. Where they differ, this spec wins:

| Pitch says | M0 does | Why |
|---|---|---|
| Validators BLS12-381-sign checkpoints (CAP-59) | Weighted ed25519 multisig (Axelar gateway pattern) | Simpler and cheaper for 3–20 validators; BLS aggregation is M2 (DEC-004) |
| Block time is "already a network setting" in stellar-core | M0 lanes do not run stellar-core | CAP-70 bounds `ledgerTargetCloseTimeMilliseconds` to [4000, 5000] and close times are whole seconds; CAP-88 (ms close times) is in Final Comment Period with no protocol version (DEC-001) |
| Games lane at 0.5s, Anchor lane at 2s | Only Caravel Perps exists; block time is a node setting (`block_time_ms`, default 1000) | Other lanes are examples, not builds |
| Validity proofs (Groth16/UltraHonk) | Not in M0 | M2 via RISC Zero (kalien pattern) |

---

## 3. Ground truth: Stellar facts this build depends on

All values below were checked on 2026-09-29 unless dated otherwise. Anything marked `[VERIFY]` MUST be rechecked before a deployment.

### 3.1 Network and versions

| Item | Value | Source |
|---|---|---|
| Current protocol | 28 ("Adapter"). Testnet upgraded 2026-08-27; mainnet activated 2026-09-16 (checked 2026-09-29 via stellar-raven). **Conflict, open:** on 2026-09-29 testnet RPC `getNetwork` and `getVersionInfo` report `protocolVersion 29` (stellar-core 29.0.0, RPC 29.0.0 built 2026-09-22), while stellar-raven's docs and news cover only Protocol 28 and crates.io has no `soroban-env-host` 29 (latest 28.0.2). Pins stay at 28. The T-012 witness `step` on testnet (protocol 29) returned output byte-equal to the executor on host 28.0.2 (tx `d0aa5608…a700b0`, 2026-09-29, `docs/RESULTS.md`) | SDF blog "Adapter, Protocol 28 Upgrade Guide", "Introducing Adapter, Protocol 28 on Stellar"; Stellar Weekly Roundup 2026-09-18; testnet RPC |
| Protocol 28 CAPs | CAP-83 (empty tx set value), CAP-85 (externally managed contract executables), CAP-86 (sparse map host functions) | same |
| `soroban-sdk` | 28.0.0 (2026-09-18) | crates.io |
| `soroban-env-host` | 28.0.2 | crates.io |
| `soroban-simulation` | 28.0.2 (M1 only) | crates.io |
| `stellar-xdr` | 28.0.1 is the latest release; Caravel pins **28.0.0**, which `soroban-env-common 28.0.2` and `soroban-sdk 28.0.0` require exactly (DEC-018) | crates.io dependency API; developers.stellar.org "Software Versions" |
| `stellar-strkey` | Caravel pins **0.0.16** (`soroban-sdk 28.0.0` requires it). `soroban-env-host 28.0.2` also pulls 0.0.13, an upstream duplicate (DEC-018) | crates.io dependency API |
| Stellar CLI | 28.1.0 (2026-09-26); builds contracts for `wasm32v1-none` | crates.io, GitHub releases |
| `@stellar/stellar-sdk` | 17.2.0 | npm |
| `@creit.tech/stellar-wallets-kit` | 2.7.0 | npm (replaced `@stellar/freighter-api` 6.0.1 in M0.5, DEC-059) |
| Rust | 1.93.0 (MSRV of stellar-cli 28.1.0; soroban-sdk 28.0.0 needs ≥ 1.91). Contracts target `wasm32v1-none`: checked 2026-09-29 with `stellar contract build --print-commands-only` | crates.io `rust_version`, stellar-cli |
| Testnet passphrase | `Test SDF Network ; September 2015` | Stellar docs |
| Testnet RPC | `https://soroban-testnet.stellar.org` | Stellar docs |
| Testnet USDC issuer | `GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5` | developers.stellar.org "Verify Trustlines" |
| Testnet USDC SAC | `CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA` | same |
| USDC decimals | 7 (1 USDC = 10,000,000 stroops) | SAC standard |
| Testnet USDC faucet | faucet.circle.com (select Stellar testnet) `[VERIFY]` | Circle |

### 3.2 Crypto available to Soroban contracts (soroban-sdk 28, `env.crypto()`)

- `ed25519_verify` traps on an invalid signature. It does not return a bool (see §8.4).
- `sha256`, `keccak256`, `secp256k1_recover`, `secp256r1_verify`.
- `bls12_381()` (CAP-59, protocol 22+) and `bn254()` (CAP-74, protocol 25+; MSM and Fr arithmetic via CAP-80, protocol 26+).
- `env.ledger().network_id()` returns the 32-byte network ID, which is `H(passphrase)`. `env.ledger().timestamp()` returns seconds.
- `ToXdr::to_xdr(&env)` and `FromXdr::from_xdr(&env, &bytes)` exist in `soroban_sdk::xdr`.

### 3.3 Per-transaction Soroban limits (testnet, checked 2026-09-29 with `stellar network settings --network testnet`)

| Limit | Value |
|---|---:|
| CPU instructions | 400,000,000 |
| Memory | 41,943,040 bytes (40 MiB) |
| Footprint entries | 400 |
| Disk reads | 200 entries / 200,000 bytes |
| Writes | 200 entries / 132,096 bytes |
| Transaction size | 132,096 bytes |
| Events + return value | 16,384 bytes |
| Contract-data entry | 65,536 bytes (key ≤ 250 bytes) |
| Contract Wasm | 131,072 bytes |
| Max entry TTL | 3,110,400 ledgers (≈ 180 days at the 5,000 ms target close time) |

These match the protocol-27 values SoroDOOM recorded on 2026-07-28. They are live network settings: recheck before a deployment `[VERIFY]`.

Consequences, used throughout this spec:

- `MAX_BATCH_BYTES = 96_000`, so a checkpoint transaction (header, batch, signatures, envelope) stays under the transaction-size limit with margin. Measured in T-006 with the settlement Wasm: a 96,000-byte batch with 3 signatures costs 7.9M instructions and 1.3 MB of memory, writes 2 entries (1,976 bytes), and its arguments are 96,892 bytes of XDR (`docs/BENCHMARKS.md`).
- `max_block_bytes` (consensus, 12,000 on the testnet lane after the T-005 benchmark, DEC-028) caps one `BlockInputV1`, so every block fits in a batch. The engine enforces it (§11.2), and the sequencer ends a batch before the next block could overflow it (§14.2).
- The engine Wasm MUST be ≤ 131,072 bytes to be deployable, so budget for ≤ 120,000.
- A contract function's return value MUST stay under 16 KiB. Views return small structs, not batches.

### 3.4 Ledger timing (why M0 does not fork stellar-core)

- CAP-70 (Final, protocol 23) added `ConfigSettingSCPTiming.ledgerTargetCloseTimeMilliseconds` with an implementation-level range of **[4000, 5000] ms**. Nomination and ballot timeouts have minimums of 750 ms and 500 ms.
- Ledger `closeTime` is whole seconds, a hard 1-second floor. CAP-88 (millisecond close times, created 2026-08-21) is in Final Comment Period with protocol version TBD.
- A stellar-core-based lane at 1s would require patching core bounds, and anything below 1s requires CAP-88. M0 avoids both (DEC-001).

### 3.5 Message signing (SEP-53, Final, v1.0.0)

- Payload: `"Stellar Signed Message:\n" || message_bytes`. Hash: `SHA256(payload)`. Signature: ed25519 over that 32-byte hash, 64 bytes.
- Freighter exposes `signMessage`, and the Stellar CLI has `stellar message sign|verify`.
- `@stellar/stellar-sdk 17.2.0` has `Keypair.signMessage` and `Keypair.verifyMessage`. Checked on 2026-09-29: a signature from `signMessage` verifies against `SHA256("Stellar Signed Message:\n" || msg)`. Use it in tests.
- Address XDR, checked with `@stellar/stellar-sdk 17.2.0` on 2026-09-29:
  - `Address(G...).toScVal().toXDR()` is 44 bytes: `000000120000000000000000 || raw_ed25519_key`;
  - a contract address is 40 bytes, starting `0000001200000001`.
- Caravel uses SEP-53 so users can sign lane transactions with Freighter (§10.3).

---

## 4. Architecture (M0)

### 4.1 Diagram

```text
                        ┌──────────────────────────── LANE (Caravel Perps, 1s blocks) ─────────────────────────────┐
 Browser (web app)      │                                                                                          │
  ├ Wallet (owner) ─────┼─► Sequencer (caravel-perps-node sequencer)                                               │
  └ session key ────────┼─►  ├ mempool → BlockInputV1 every block_time_ms                                         │
                        │    ├ executes engine Wasm via soroban-env-host  (StateV1 → StateV1)                     │
                        │    ├ persists block + state (SQLite) before broadcasting                                │
                        │    ├ public API + WebSocket                                                             │
                        │    └ assembles CheckpointHeaderV1 + BatchV1, collects signatures                        │
                        │            │ blocks (WS)                     ▲ signatures                               │
                        │            ▼                                 │                                          │
                        │    Validators ×3 (caravel-perps-node validator)                                        │
                        │      re-execute every block with the same Wasm, compare state hashes,                   │
                        │      sign checkpoint header hashes (ed25519), keep their own copy of blocks            │
                        └──────────────┬──────────────────────────────────────────────▲──────────────────────────┘
                                       │ checkpoint tx (header + full batch + sigs)    │ inbox messages, oracle prices
                                       ▼                                               │
                     Relayer (platform/relayer, TypeScript)  ──────────────────────────────┘
                       ├ inbox watcher (Stellar events → sequencer)
                       ├ checkpoint submitter (sequencer → Stellar)
                       └ oracle feeder (Reflector/CEX → signed OracleUpdateV1 → sequencer)
                                       │
                                       ▼
 ┌────────────────────────────── STELLAR (testnet, ~5s ledgers) ─────────────────────────────┐
 │ Settlement contract (platform/contracts/settlement)                                                │
 │   USDC vault · inbox (deposits, forced withdrawals, hash accumulator)                     │
 │   checkpoints (verify header, batch hash, ed25519 weighted threshold)                     │
 │   claim_withdrawal (Merkle proof) · freeze · escape_claim · refund_unprocessed_deposit    │
 │ Engine contract (lanes/perps/engine/contracts/perps-engine) deployed for Wasm-hash identity + witness tx     │
 │ USDC SAC                                                                                  │
 └───────────────────────────────────────────────────────────────────────────────────────────┘
                                       │ RPC (contract data + transactions)
                                       ▼
 Replay verifier (caravel-perps-node replay): rebuilds every state from genesis + batches on Stellar
```

### 4.2 Components

| Component | Path | Language | Role |
|---|---|---|---|
| Types & codecs | `lanes/perps/engine/crates/caravel-types` | Rust `no_std` | All §9 encodings, fixed-point helpers, domain tags |
| Merkle | `lanes/perps/engine/crates/caravel-merkle` | Rust `no_std` | Tree build/verify (§9.9) shared by engine, nodes, settlement contract |
| Perps core | `lanes/perps/engine/crates/caravel-perps` | Rust `no_std` + `alloc` | Deterministic engine logic (§11), generic over a `Crypto` trait |
| Engine contract | `lanes/perps/engine/contracts/perps-engine` | Soroban | Thin wrapper: `genesis`, `step`, `version` (§12) |
| Settlement contract | `platform/contracts/settlement` | Soroban | §13 |
| Lane runtime | `platform/crates/caravel-runtime` | Rust std | soroban-env-host executor, block/batch builders, SQLite store |
| Node | `platform/crates/caravel-node` | Rust std (library) | The `sequencer`, `validator`, `replay`, `check-store`, `genesis` and `witness` commands for any app through `NodeApp` (§14–§16, DEC-053); lane files (DEC-054) |
| Perps node | `lanes/perps/node` | Rust std (bin `caravel-perps-node`) | `PerpsApp` (the perps `LaneApp` and `NodeApp`), its lane file, views and `/v1/markets*` routes, and `tx` |
| Relayer | `platform/relayer` | TypeScript (Node ≥ 22) | §17 |
| Web app | `lanes/perps/web` | TypeScript, React, Vite | §18 |

### 4.3 Trust model (M0)

| Actor | Can | Cannot |
|---|---|---|
| Sequencer | Order, delay or drop lane transactions until the force-inclusion window; propose blocks | Forge user signatures; credit deposits that did not happen (the inbox accumulator is checked on Stellar); get a checkpoint accepted without 2 validator signatures |
| 2 of 3 validators + sequencer (collusion) | Sign a wrong state and steal funds through fake withdrawals | Hide it: the batch is on Stellar and replay exposes the wrong state hash |
| Oracle key | Publish bad prices within the circuit breaker (§11.5), causing unfair liquidations | Move funds directly |
| Relayer | Delay submissions (liveness) | Change checkpoint contents: signatures cover the header, and the header covers the batch hash |
| Settlement admin (testnet only) | Upgrade the contract and rotate validators without delay | Documented as a testnet-only power; removed or timelocked before M2 (T-M2-06) |

### 4.4 Life of the main flows

1. **Deposit:**
   - The user calls `deposit` on the settlement contract, which moves USDC into the vault and appends an `InboxMsgV1` (kind 0).
   - The relayer forwards the message, and the sequencer includes it as an `INBOX` entry.
   - The engine credits collateral and updates `inbox_acc`.
   - At the next checkpoint, the contract checks `inbox_acc` against its own accumulator.
2. **Trade:**
   - The browser signs a `LaneTxV1` (usually with the session key) and posts it to the sequencer.
   - The sequencer includes it in the next block, and the engine matches it.
   - The fill is visible within one block (soft confirmation). It is final on Stellar after the next checkpoint is accepted.
3. **Checkpoint:**
   - Every `checkpoint_every_blocks` blocks (default 10), or earlier if the batch would exceed `MAX_BATCH_BYTES`, the sequencer flags the last block `CHECKPOINT_END`.
   - The engine computes commitments, the sequencer builds header and batch, and validators verify and sign.
   - The relayer submits `submit_checkpoint(header, batch, epoch, signatures)`.
4. **Withdraw:**
   - The user signs `WITHDRAW` with their wallet (SEP-53), and the engine debits collateral into the pending withdrawal list.
   - At the checkpoint, the list becomes `withdrawals_root`.
   - After the checkpoint is accepted, the web app fetches a proof and calls `claim_withdrawal` on Stellar.
5. **Forced withdrawal (anti-censorship):**
   - The user calls `request_forced_withdrawal` on Stellar (inbox kind 1).
   - If the lane does not process it within `force_inclusion_window_secs`, anyone can call `freeze`.
6. **Freeze and escape:**
   - Freeze is allowed if no checkpoint is accepted for `escape_timeout_secs`, or an inbox message is overdue.
   - After freeze, users call `escape_claim` with a Merkle proof of their account leaf from the last accepted checkpoint.
   - Unprocessed deposits are refunded 1:1.

---

## 5. Prior art to reuse

Rules:
- Copy code only from files whose license is MIT or Apache-2.0.
- Keep the upstream copyright header, and add the source to `docs/SOURCES.md`.
- Never copy GPL files.

| Caravel part | Take | From | License / status (2026-09-29) | Notes |
|---|---|---|---|---|
| Off-chain execution of exact Wasm | `Runner` pattern:<br>• `Host::test_host_with_recording_footprint()`<br>• `set_ledger_info`<br>• `register_test_contract_wasm_from_source_account`<br>• budget reset per call (Caravel pins explicit limits, §14.5)<br>• `Host::call` of `step(Bytes, Bytes) -> Bytes` | SoroDOOM `apps/host-runner/src/main.rs` | MIT OR Apache-2.0 (original code); host-runner is original | Pinned to `soroban-env-host =27.0.1`; Caravel pins 28.0.2 (§7). Re-verify API names |
| Parity discipline | Same transition run natively and in Wasm, byte-identical at every step; golden tapes with intermediate hashes | SoroDOOM `docs/DETERMINISM.md`, `docs/BENCHMARKS.md` | same | §8, §19 |
| Tape/replay format style | Fixed-width LE, magic + version, tape hash over all preceding bytes, fail-closed decoding | SoroDOOM `docs/REPLAY-FORMAT.md` (`SDTAPE01`) | same | §9 |
| Registry contract style | Immutable rules binding; manifest; explicit index because Soroban cannot enumerate by prefix; TTL extend toward 120 days when < 30 days | SoroDOOM `contracts/registry` (`soroban-sdk =27.0.3`) | MIT OR Apache-2.0 | §13.6 |
| Sequencer persistence | Persist state + tape in one transaction before acknowledging; restore after restart | SoroDOOM D004 (Durable Object + SQLite) | same | Caravel uses SQLite in a Rust process, not a Durable Object (DEC-010) |
| Agent rules | "Benchmark before splitting the engine"; keep time, randomness and I/O out of state | SoroDOOM `AGENTS.md` | same | Folded into §0.3 and §8 |
| Snapshot simulation | `soroban_simulation::simulate_invoke_host_function_op` over caller-supplied ledger entries | Soroflare (`stellar/soroflare`) | Apache-2.0, archived Feb 2026, old env | Reference only; M1 executor (§21) uses current `soroban-simulation` |
| Validator signatures | Weighted ed25519 signers, epochs, rotation delay, previous-signer retention, `validate_proof` | Axelar `axelar-amplifier-stellar` `contracts/stellar-axelar-gateway/src/auth.rs` | Check LICENSE before copying `[VERIFY]`; maintained | Re-implement from the design if license unclear (§13.4) |
| USDC pool shape | `deposit`, `rollup`, `withdraw`, `collect_fees` collateral pool | `rails-xyz/soroban-rollup-contract` | License not stated: design reference only | Owner-trusted; Certora rules present |
| Exit timing | Declare → observation period → close; newer state wins | Starlight (Go, CAP-21/CAP-40) | Apache-2.0, archived Apr 2024 | Design only |
| Soroban channel pattern | Deposit, off-chain ed25519 cumulative commitments, `close()`, refund waiting period | `stellar-experimental/one-way-channel` (used by `stellar/stellar-mpp-sdk`) | MIT, experimental | Reference for freeze/escape timers |
| Dispute pattern | Deposit, dispute, settle, withdraw in Soroban | `perun-network/perun-soroban-contract` | Apache-2.0, dormant | Reference for M1 challenge windows |
| BLS aggregate verify | G1 pubkeys, G2 sigs, DST `BLSSIG-V01-CS01-with-BLS12381G2_XMD:SHA-256_SSWU_RO_`, pairing check | `stellar/soroban-examples/bls_signature` | Apache-2.0; "not security-audited" | M2 only; needs proof-of-possession |
| ZK verification | RISC Zero Groth16 wrap verified on Soroban; deterministic tapes proven off-chain | `kalepail/kalien`, `NethermindEth/stellar-risc0-verifier`, `NethermindEth/rs-soroban-ultrahonk`, `stellar/soroban-examples/groth16_verifier` | Mixed; check each | M2 only |
| Contract utilities | Pausable, upgradeable, Merkle/crypto utilities, fee abstraction, governance/timelock | OpenZeppelin `stellar-contracts` (`stellar-contract-utils`, `stellar-fee-abstraction`, `stellar-governance`) | MIT; audited releases | M0 MAY use `stellar-contract-utils` for upgradeable/pausable; M2 timelock |
| Data lake | Ledger metadata export (LedgerCloseMeta) to object storage; SEP-54 layout | Galexie, SEP-54 | Apache-2.0 | Long-term access to checkpoint transactions after RPC retention (§16.3) |

What is new (nobody has built it for Stellar):
- the multi-user perps engine;
- the inbox and accumulator;
- per-checkpoint withdrawal and escape roots;
- validator re-execution and co-signing;
- the lane config.

---

## 6. Repository layout

The M0 layout, as first planned. M0.5 moved it to `platform/` and `lanes/perps/` (§20.3, DEC-051); CLAUDE.md has the current layout.

```text
caravel/
├── CLAUDE.md                     # agent entry point (short; points here)
├── docs/
│   ├── CARAVEL_SPEC.md           # this file
│   ├── SOURCES.md                # every external source used, with URL and date
│   └── RUNBOOK.md                # created in T-015: how to run locally and on testnet
├── versions.json                 # pinned versions and artifact hashes (§7)
├── LICENSE-MIT, LICENSE-APACHE
├── Cargo.toml                    # workspace
├── rust-toolchain.toml
├── crates/
│   ├── caravel-types/
│   ├── caravel-merkle/
│   ├── caravel-perps/
│   ├── caravel-lane/
│   ├── caravel-node/
│   └── caravel-testkit/          # test-only: lane simulator, scenarios, `cargo gen-vectors` (DEC-025)
├── contracts/
│   ├── perps-engine/
│   └── settlement/
├── apps/
│   ├── relayer/
│   └── web/
├── config/
│   ├── lane.caravel-perps.testnet.toml   # genesis + node config (§10)
│   └── lane.local.toml
├── test-vectors/                 # golden vectors (hex JSON), shared by Rust and TS tests
├── scripts/
│   ├── build-contracts.sh
│   ├── check-versions.mjs        # CI check of versions.json pins and placeholders (§7)
│   ├── deploy-testnet.sh
│   ├── e2e-local.sh
│   └── e2e-testnet.sh
├── docker/
│   ├── node.Dockerfile
│   └── compose.local.yml         # quickstart + sequencer + 3 validators + relayer
├── site/                         # landing page, deployed to Vercel from this folder only (DEC-019)
└── .github/workflows/ci.yml
```

License for original Caravel code: `MIT OR Apache-2.0`, the same as SoroDOOM (OQ-006 confirms).

---

## 7. Pinned versions (`versions.json`)

Create this file in T-000 and keep it current. CI fails if a `Cargo.toml` or `package.json` version differs from it.

```json
{
  "spec_version": "0.1.0",
  "stellar_protocol": 28,
  "network": "testnet",
  "rust_toolchain": "1.93.0",
  "wasm_target": "wasm32v1-none",
  "stellar_cli": "28.1.0",
  "node": "22",
  "crates": {
    "soroban-sdk": "=28.0.0",
    "soroban-env-host": "=28.0.2",
    "stellar-xdr": "=28.0.0",
    "stellar-strkey": "=0.0.16",
    "ed25519-dalek": "=2.2.0",
    "sha2": "=0.10.9"
  },
  "npm": {
    "@stellar/stellar-sdk": "17.2.0",
    "@creit.tech/stellar-wallets-kit": "2.7.0",
    "lightweight-charts": "5.2.1",
    "@noble/ed25519": "3.2.0",
    "@noble/hashes": "2.4.0"
  },
  "artifacts": {
    "engine_wasm_sha256": "4571cd252d4b78d523d01fc4a31ab763a1aff77aecafbb9d7302cfb879abbf0a",
    "settlement_wasm_sha256": "8a2fafbd1ad48d53333ab79d73afa1c50d5f790f39a97fafb88b5443b383d503",
    "genesis_config_sha256": "f4b9db09137583ba9d66ea0b8a3a2b658a163f7b72993e0f242f04ea3ac93997",
    "genesis_state_sha256": "22702d9f4c45f88306aca02169cf7a86ccaec3b45ed85103c7a9fc4277291e77"
  },
  "testnet": {
    "rpc_url": "https://soroban-testnet.stellar.org",
    "network_passphrase": "Test SDF Network ; September 2015",
    "usdc_sac": "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA",
    "settlement_contract": "FILLED_BY_T-012",
    "engine_contract": "FILLED_BY_T-012"
  }
}
```

`ed25519-dalek` and `sha2` in the native engine build MUST be the exact versions `soroban-env-host 28.0.2` depends on. Read its `Cargo.toml` and use `cargo tree`. This makes native and host signature verification behave identically (§8.4). Resolved in T-000 (2026-09-29): the host requires `^2.0.0` and `^0.10.8`, and the workspace pins `=2.2.0` and `=0.10.9`, so `Cargo.lock` holds exactly one version of each. `scripts/check-versions.mjs` enforces this.

`stellar_cli` pins the CLI that builds the Wasm of record (DEC-020). `node` is the minimum Node major for the relayer and web app.

Values starting with `RESOLVE_IN_` or `FILLED_BY_` are placeholders:
- The CI version check skips them.
- CI fails if any `RESOLVE_IN_` value remains after T-000 is `done`.
- CI fails if a `FILLED_BY_T-xxx` value remains after that task is `done`.

---

## 8. Determinism rules

These apply to `caravel-types`, `caravel-merkle`, `caravel-perps` and `contracts/*`. Violations are bugs, even if tests pass.

### 8.1 Coding rules

- **INV-D1:** Integer arithmetic only.
  - Use `checked_*` for anything reachable from user input, and map overflow to a rejection code.
  - Release profiles set `overflow-checks = true`.
- **INV-D2:** No wall-clock time, randomness, environment variables, filesystem or network access inside the engine. Time enters only as `BlockInputV1.timestamp_ms`.
- **INV-D3:** No `HashMap`/`HashSet` in consensus code. Use sorted `Vec`s or `BTreeMap`. Iteration order MUST be defined (by account index, order ID or price-time).
- **INV-D4:** Serialize fields, never memory layout. No `unsafe` transmutes, and no `usize` in encodings (use `u32`/`u64`).
- **INV-D5:** Every division uses one of the named helpers in `caravel-types::fixed`, which document the rounding direction. There are no bare `/` or `%` on signed values:
  - `div_floor(a, b)`, `div_ceil(a, b)`, `div_trunc(a, b)`;
  - `mul_div_floor(a, b, c)`, `mul_div_ceil(a, b, c)`, `mul_div_trunc(a, b, c)`;
  - intermediates are `i128`, and overflow returns `Err`;
  - `a % tick == 0` checks use `rem_euclid`.
- **INV-D6:** Decoders are strict:
  - exact lengths, and trailing bytes are an error;
  - enums only accept listed values;
  - booleans only accept 0 or 1.
- **INV-D7:** The engine Wasm is built reproducibly: pinned toolchain, `--locked`, and the release profile in §12.3. `sha256` of the built Wasm is recorded in `versions.json` and MUST equal the hash uploaded to Stellar.

### 8.2 Execution paths that must agree

| Path | Where | Used for |
|---|---|---|
| Wasm in `soroban-env-host 28.0.2` | sequencer, validators, replay | **Consensus.** The only path whose output is signed |
| Native Rust (`caravel-perps` compiled for the host) | unit, property and fuzz tests; sequencer pre-validation | Speed during tests; MUST equal the Wasm path |
| Wasm on Stellar testnet (`step` invocation) | witness transaction (T-012) | Evidence that the exact call is valid on Stellar |

Parity gate (T-005): 10,000 randomized blocks plus the golden scenarios in §19.3 MUST produce identical state bytes on the native and Wasm paths.

### 8.3 Fatal vs non-fatal

- **Fatal (block invalid):**
  - bad encoding;
  - entry order violation;
  - an inbox index gap;
  - a failed signature verification;
  - an engine trap or panic.
  - A fatal block MUST NOT be signed by validators. The sequencer MUST NOT produce one, and it discards the block and alerts.
- **Non-fatal (transaction rejected with a reason code):** everything in §11 that depends on user intent or state. Examples: bad nonce, insufficient margin, stale oracle.
- Rejected transactions stay in the block, which keeps the tape exact. Their effects are limited to what §11.3 says (nonce consumption).

### 8.4 Signature verification

- Inside the engine Wasm, signatures are checked with `env.crypto().ed25519_verify`, which **traps** on failure (fatal).
- The sequencer MUST pre-verify every signature natively before accepting a transaction into the mempool.
- The native `Crypto` impl MUST use the same verification function variant as `soroban-env-host 28.0.2`. Checked 2026-09-29 in the pinned source (`src/crypto/mod.rs`, `verify_sig_ed25519_internal`): the host parses the key with `ed25519_dalek::VerifyingKey::from_bytes` (an invalid key is an error, so the call traps) and verifies with `verify_strict`. The native impl MUST do the same, and panic on both an invalid key and an invalid signature.
- `test-vectors/signatures.json` (T-001) has a non-canonical `S + L` signature and a small-order (identity) key. `verify_strict` rejects both, while plain `verify` accepts the small-order case, and so does OpenSSL. Assert that both paths agree on every case (T-003 native, T-005 host).

---

## 9. Binary encodings (frozen once vectors land)

General rules: little-endian integers, `bytes32` raw, no padding, and length prefixes exactly as listed. Every encoded object starts with an 8-byte ASCII magic where listed. Decoders reject unknown magic or version.

### 9.1 Domain tags

Used inside `H(...)`. Defined as constants in `caravel-types::tags`:

| Constant | Bytes |
|---|---|
| `TAG_TX` | `CARAVEL/TX/V1` |
| `TAG_INBOX` | `CARAVEL/INBOX/V1` |
| `TAG_ORACLE` | `CARAVEL/ORACLE/V1` |
| `TAG_ACCT_LEAF` | `CARAVEL/ACCT/V1` |
| `TAG_WDL_LEAF` | `CARAVEL/WDL/V1` |
| `TAG_SIGNERS` | `CARAVEL/SIGNERS/V1` |
| `TAG_ROTATE` | `CARAVEL/ROTATE/V1` |
| `TAG_LANE_ID` | `CARAVEL/LANE/V1` |

`lane_id = H(TAG_LANE_ID || utf8(lane_name))`. M0 lane name: `caravel-perps-testnet-0`.

### 9.2 `LaneTxV1` (user transaction)

| Offset | Size | Field | Notes |
|---:|---:|---|---|
| 0 | 1 | version | `1` |
| 1 | 32 | lane_id | |
| 33 | 32 | account | owner's ed25519 public key (the raw key of their `G...` address) |
| 65 | 32 | signer | `account`, or a registered session key |
| 97 | 8 | nonce | u64; MUST equal `account.next_nonce` |
| 105 | 8 | expiry_ms | u64; block `timestamp_ms` MUST be ≤ this |
| 113 | 1 | kind | §9.3 |
| 114 | 1 | sig_scheme | `0` RAW_ED25519, `1` SEP53 |
| 115 | 2 | body_len | u16; MUST equal the kind's fixed body size |
| 117 | N | body | §9.3 |
| 117+N | 64 | signature | |

- `tx_hash = H(TAG_TX || config_hash || bytes[0 .. 117+N])`, i.e. everything except the signature.
  - `config_hash` is `H(GenesisConfigV1 bytes)` of the lane. Clients get it from `GET /v1/status`.
  - This binds signatures to one lane deployment even if two deployments share a name.
  - Each deployment MUST still use a unique lane name.
- `sig_scheme = 0`: `ed25519_verify(signer, tx_hash, signature)`.
- `sig_scheme = 1`:
  - message = UTF-8 `"Caravel lane tx " || lowercase_hex(tx_hash)`, 80 bytes;
  - `sep53_hash = H("Stellar Signed Message:\n" || message)`;
  - check `ed25519_verify(signer, sep53_hash, signature)`.
  - Scheme 1 is only valid when `signer == account`.

### 9.3 Transaction kinds and bodies

| kind | Name | Body (size) | Allowed signer |
|---:|---|---|---|
| 1 | PLACE_ORDER | `market_id u16 · side u8 (0 buy, 1 sell) · tif u8 (0 GTC, 1 IOC, 2 POST_ONLY) · reduce_only u8 · price i64 · lots i64 · client_order_id u64` (29) | owner, or session key with `PERM_TRADE` |
| 2 | CANCEL_ORDER | `market_id u16 · order_id u64` (10) | owner, or session key with `PERM_CANCEL` |
| 3 | CANCEL_ALL | `market_id u16` (`0xFFFF` = all markets) (2) | owner, or session key with `PERM_CANCEL` |
| 4 | WITHDRAW | `amount i128` (16) | owner only |
| 5 | ADD_SESSION_KEY | `session_key bytes32 · expires_at_ms u64 · permissions u8` (41) | owner only |
| 6 | REVOKE_SESSION_KEY | `session_key bytes32` (32) | owner only |

Permissions bits:
- `PERM_TRADE = 0x01`;
- `PERM_CANCEL = 0x02`;
- other bits MUST be 0;
- session keys can never withdraw or manage keys.

### 9.4 `InboxMsgV1` (Stellar → lane; built by the settlement contract)

| Offset | Size | Field |
|---:|---:|---|
| 0 | 1 | kind (`0` DEPOSIT, `1` FORCED_WITHDRAWAL) |
| 1 | 8 | index (u64, 0-based, sequential) |
| 9 | 32 | lane_account |
| 41 | 16 | amount (i128) |
| 57 | 8 | enqueued_at (u64, Stellar ledger timestamp, seconds) |

Total 65 bytes.

- Accumulator: `acc_0 = [0u8; 32]`, and `acc_{n+1} = H(TAG_INBOX || acc_n || msg_n)`.
- `inbox_through = n` means messages `0..n-1` are processed and `inbox_acc = acc_n`.

### 9.5 `OracleUpdateV1`

| Offset | Size | Field |
|---:|---:|---|
| 0 | 2 | market_id |
| 2 | 8 | price (i64, USDC stroops per lot; §11.1) |
| 10 | 8 | publish_time_ms (u64) |
| 18 | 32 | oracle_key |
| 50 | 64 | signature over `H(TAG_ORACLE || lane_id || bytes[0..18])` |

Total 114 bytes.

### 9.6 `BlockInputV1` (what the engine executes) and `BlockRecordV1`

Header (93 bytes):

| Offset | Size | Field |
|---:|---:|---|
| 0 | 8 | magic `CVBLKIN1` |
| 8 | 32 | lane_id |
| 40 | 8 | height (u64, first block = 1) |
| 48 | 8 | timestamp_ms (u64, ≥ previous block's) |
| 56 | 32 | prev_block_hash (zero for height 1) |
| 88 | 1 | flags (bit 0 = `CHECKPOINT_END`; other bits 0) |
| 89 | 4 | entry_count (u32, total of all entry types, ≤ `max_entries_per_block`) |

Then `entry_count` entries, each `entry_type u8 · len u32 · payload[len]`:

| entry_type | Payload | Order rule |
|---:|---|---|
| 1 INBOX | `InboxMsgV1` (65) | all INBOX entries first, indexes strictly sequential from `state.inbox_through` |
| 2 ORACLE | `OracleUpdateV1` (114) | after INBOX, before USER; **at most one per market per block** |
| 3 USER | `LaneTxV1` | last, in sequencer arrival order |

Block-level limits (all fatal if violated):
- `BlockInputV1` total length ≤ `max_block_bytes`;
- `entry_count ≤ max_entries_per_block` (counts INBOX + ORACLE + USER);
- each entry's `len` equals its payload's exact encoded length.

`BlockRecordV1 = BlockInputV1 bytes || state_hash_after (32)`, where `state_hash_after = H(StateV1 bytes after executing the block)`.

- `input_hash = H(BlockInputV1 bytes)`.
- `block_hash = H(input_hash || state_hash_after)`.
- The next block's `prev_block_hash` is this `block_hash`, and the engine checks it (§11.2).

### 9.7 `BatchV1` (the data-availability payload posted on Stellar)

| Offset | Size | Field |
|---:|---:|---|
| 0 | 8 | magic `CVBATCH1` |
| 8 | 32 | lane_id |
| 40 | 8 | checkpoint_seq (u64) |
| 48 | 4 | block_count (u32) |
| 52 | … | block_count × (`len u32 · BlockRecordV1 bytes`) |

Rules:
- Blocks are consecutive heights.
- The first block's `prev_block_hash` equals the previous checkpoint's `last_block_hash` (zero for seq 1).
- Only the last block has `CHECKPOINT_END` set.
- Total size ≤ `MAX_BATCH_BYTES` (96,000).
- `batch_hash = H(BatchV1 bytes)`.

### 9.8 `CheckpointHeaderV1` (442 bytes)

| Offset | Size | Field | Notes |
|---:|---:|---|---|
| 0 | 8 | magic | `CVCKPT01` |
| 8 | 2 | version | `1` |
| 10 | 32 | lane_id | |
| 42 | 32 | network_id | `H(network passphrase)`; contract compares to `env.ledger().network_id()` |
| 74 | 32 | settlement_addr_hash | `H(ScVal XDR of the settlement contract Address)`; contract compares to `H(env.current_contract_address().to_xdr(&env))` |
| 106 | 32 | engine_wasm_hash | MUST equal the contract config |
| 138 | 8 | seq | first checkpoint = 1 |
| 146 | 32 | prev_header_hash | zero for seq 1 |
| 178 | 8 | first_block_height | |
| 186 | 8 | last_block_height | |
| 194 | 8 | last_block_timestamp_ms | |
| 202 | 32 | last_block_hash | `block_hash` (§9.6) of the last block in the batch |
| 234 | 32 | batch_hash | |
| 266 | 32 | state_hash | after the last block |
| 298 | 32 | accounts_root | §11.8 |
| 330 | 4 | account_count | |
| 334 | 16 | escape_total | i128 |
| 350 | 32 | withdrawals_root | §11.8 |
| 382 | 4 | withdrawal_count | |
| 386 | 16 | withdrawals_total | i128 |
| 402 | 8 | inbox_through | |
| 410 | 32 | inbox_acc | |

`header_hash = H(header bytes)`. Validators sign `header_hash`, the raw 32 bytes, with ed25519.

### 9.9 Merkle trees (`caravel-merkle`)

- Input: an ordered list of `n` leaf hashes (32 bytes each; leaf hashing is done by the caller, §11.8).
- If `n == 0`, `root = [0u8; 32]`.
- Otherwise:
  - pad to `P = next_power_of_two(n)` with `ZERO = [0u8; 32]` leaves (not hashed);
  - `node = H(0x01 || left || right)`;
  - leaf **preimages** start with byte `0x00` and node preimages with `0x01` (see §11.8), which separates leaves from nodes.
- Proof: `siblings[depth]` from the leaf level upward, where `depth = log2(P)` (0 when `n == 1`).
- Verification rules:
  - `index < n`;
  - `siblings.len() == ceil_log2(n)`;
  - at level `k`, if bit `k` of `index` is 0 the current node is left, else right.
- Max depth: 20.
- `caravel-merkle` is `no_std` with no `alloc` in the verify path. Hashing goes through a `trait Sha256 { fn hash(&self, data: &[u8]) -> [u8; 32]; }`:
  - native impl: `sha2`;
  - Soroban impl: `Bytes::from_slice` + `env.crypto().sha256`.

### 9.10 `StateV1` (canonical engine state)

Layout in order:

```text
magic "CVSTATE1" (8)
lane_id (32)
config_hash (32)                 # H(GenesisConfigV1 bytes)
height u64                       # last executed block height (0 at genesis)
last_block_input_hash (32)       # H(BlockInputV1) of the last executed block; zero at genesis
last_timestamp_ms u64
checkpoint_seq u64               # completed checkpoints
inbox_through u64
inbox_acc (32)
next_order_id u64                # starts at 1
deposits_credited_total i128
withdrawals_committed_total i128
flags u8                         # bit0 BACKSTOP_DEFICIT (informational, §11.7); other bits 0
config GenesisConfigV1 (embedded, §10.2 encoding)
account_count u32
accounts[account_count] AccountV1    # index = position; 0 = backstop, 1 = treasury
markets[config.market_count] MarketStateV1
pending_count u32
pending[pending_count] (key bytes32 · amount i128)
last_commitment CommitmentV1
```

`AccountV1`:

```text
key (32) · flags u8 (bit0 SYSTEM) · next_nonce u64 · collateral i128
open_order_count u16 · session_key_count u8
session_keys[session_key_count] (key 32 · expires_at_ms u64 · permissions u8)   # sorted by key
positions[config.market_count] (lots i64 · cost_basis i128)                     # by market position
txs_this_block u16                                                               # reset each block
```

`MarketStateV1`:

```text
oracle_price i64 · oracle_time_ms u64 · last_funding_time_ms u64
cumulative_funding_per_lot i128 · open_interest_lots i64
bid_count u32 · bids[bid_count] OrderV1     # best first: price desc, order_id asc
ask_count u32 · asks[ask_count] OrderV1     # best first: price asc,  order_id asc
```

- `OrderV1`: `order_id u64 · account_index u32 · price i64 · lots_remaining i64 · client_order_id u64`.
- `CommitmentV1`:

  ```text
  seq u64 · last_block_height u64
  accounts_root (32) · account_count u32 · escape_total i128
  withdrawals_root (32) · withdrawal_count u32 · withdrawals_total i128
  inbox_through u64 · inbox_acc (32)
  ```
- `state_hash = H(StateV1 bytes)`.
- The account lookup by key is rebuilt on decode (sort `(key, index)`). It is not stored.

---

## 10. Lane configuration

### 10.1 Two kinds of settings

- **Consensus settings** live in `GenesisConfigV1`. They are hashed into `config_hash`, embedded in state and cannot change in M0: markets, fees, caps, oracle keys, access list, system account keys.
- **Node settings** do not affect state hashes: `block_time_ms`, `checkpoint_every_blocks`, URLs, key file paths, and ports.

### 10.2 `GenesisConfigV1` encoding

```text
magic "CVGENES1" (8) · lane_id (32)
backstop_key (32) · treasury_key (32)
access_mode u8 (0 OPEN, 1 ALLOWLIST) · allowlist_count u16 · allowlist[] (32 each, sorted)
oracle_key_count u8 · oracle_keys[] (32 each, sorted)
oracle_max_staleness_ms u64 · oracle_max_future_ms u64
oracle_circuit_breaker_bps u16 · oracle_breaker_bps_per_sec u16
funding_interval_ms u64 · funding_damping u16 · funding_max_rate_ppm u32
insurance_fee_share_bps u16
min_deposit i128 · min_withdrawal i128
max_accounts u32 · max_orders_per_side u32 · max_open_orders_per_account u16
max_session_keys u8 · max_txs_per_account_per_block u16 · max_entries_per_block u32
max_block_bytes u32 · max_pending_withdrawals u32
exec_cpu_limit u64 · exec_mem_limit u64          # host budget per step call (consensus, §14.5)
market_count u16 · markets[] MarketParamsV1
```

`MarketParamsV1`:

```text
market_id u16 · symbol [16] (ASCII, zero-padded)
tick i64 · imf_bps u16 · mmf_bps u16 · taker_fee_bps u16 · maker_fee_bps u16
liq_fee_bps u16 · band_bps u16 · max_position_lots i64 · max_oi_lots i64
impact_lots i64                                              # depth used for the funding premium (§11.6)
display_lot_base_units i64 · display_base_decimals u8        # UI only, not used in math
```

Genesis validation. `genesis()` returns `Fatal(BAD_CONFIG)` unless every rule holds:
- `backstop_key != treasury_key`.
- `allowlist`, `oracle_keys` and market IDs are strictly ascending (so there are no duplicates). `oracle_key_count ≥ 1`.
- `funding_interval_ms ≥ 60_000`, `funding_damping ≥ 1`, `funding_max_rate_ppm ≤ 10_000`.
- `insurance_fee_share_bps ≤ 10_000`, `oracle_circuit_breaker_bps ≥ 1`, `oracle_max_staleness_ms ≥ 1_000`.
- `min_deposit ≥ 1`, `min_withdrawal ≥ 1`.
- All `max_*` fields are > 0.
- `max_accounts ≥ 3` (two system accounts plus at least one user).
- `max_block_bytes ≤ 48_000` and `max_block_bytes + 1_024 ≤ MAX_BATCH_BYTES`.
- `exec_cpu_limit ≤ 400_000_000`, `exec_mem_limit ≤ 41_943_040`.
- `1 ≤ market_count ≤ 4`.
- Per market:
  - `tick > 0`;
  - `0 < mmf_bps < imf_bps ≤ 10_000`;
  - fee bps values ≤ 1_000; `liq_fee_bps ≤ mmf_bps`;
  - `0 < band_bps ≤ 5_000`;
  - `0 < max_position_lots ≤ max_oi_lots`;
  - `impact_lots > 0`;
  - `symbol` is ASCII.

### 10.3 M0 lane file (`lanes/perps/config/lane.caravel-perps.testnet.toml`)

`caravel-perps-node genesis` converts this to `GenesisConfigV1` and prints `config_hash`, `genesis_state_hash` and the state size.

This is the M0 layout. Since M0.5 the file uses the platform layout (DEC-054): the same values, with `[app]` added, the perps settings under `[perps.*]`, and `max_orders_per_side` and `max_open_orders_per_account` under `[perps.limits]`. Both layouts give the same bytes.

```toml
[lane]
name = "caravel-perps-testnet-0"          # lane_id = H("CARAVEL/LANE/V1" || name)

[node]                                     # not consensus
block_time_ms = 1000                       # allowed 200..=5000
checkpoint_every_blocks = 60                # one a minute on testnet (DEC-044)
max_batch_bytes = 96000

[accounts]
backstop_key = "G..."                      # system account 0 (insurance fund + liquidation backstop)
treasury_key = "G..."                      # system account 1 (fees)

[access]
mode = "open"                              # "open" | "allowlist"
allowlist = []

[oracle]
keys = ["G..."]
max_staleness_ms = 30000
max_future_ms = 5000
circuit_breaker_bps = 1000                 # 10% base move allowed per update ...
breaker_bps_per_sec = 10                   # ... plus 0.1% per second since the last accepted update

[funding]
interval_ms = 3600000
damping = 8
max_rate_ppm = 500                         # 0.05% per interval

[fees]
insurance_share_bps = 3000                 # 30% of every fee to backstop, rest to treasury

[limits]
min_deposit = 10000000                     # 1 USDC (the settlement contract also enforces its own min, §13.1)
min_withdrawal = 10000000
max_accounts = 256                         # caps from the T-005 benchmark (DEC-028)
max_orders_per_side = 128
max_open_orders_per_account = 32
max_session_keys = 4
max_txs_per_account_per_block = 50
max_entries_per_block = 256                # all entry types together
max_block_bytes = 12000                    # ~56 PLACE_ORDER txs; see §3.3
max_pending_withdrawals = 512
exec_cpu_limit = 200000000                 # T-005: worst block at these caps is 95.8M host insns
exec_mem_limit = 41943040

[[markets]]
market_id = 1
symbol = "BTC-PERP"
display_lot_base_units = 10000             # 1 lot = 0.0001 BTC (base decimals 8)
display_base_decimals = 8
tick = 1000                                # 0.0001 USDC per lot = $1 per BTC
imf_bps = 1000                             # 10x max
mmf_bps = 500
taker_fee_bps = 5
maker_fee_bps = 0
liq_fee_bps = 100
band_bps = 500
max_position_lots = 20000                  # 2 BTC
max_oi_lots = 200000                       # 20 BTC
impact_lots = 1000                         # 0.1 BTC of depth for the funding premium

[[markets]]
market_id = 2
symbol = "ETH-PERP"
display_lot_base_units = 100000            # 1 lot = 0.001 ETH (base decimals 8)
display_base_decimals = 8
tick = 1000                                # $0.10 per ETH
imf_bps = 1000
mmf_bps = 500
taker_fee_bps = 5
maker_fee_bps = 0
liq_fee_bps = 100
band_bps = 500
max_position_lots = 50000
max_oi_lots = 500000
impact_lots = 2000                         # 2 ETH

[[markets]]
market_id = 3
symbol = "XLM-PERP"
display_lot_base_units = 100000000         # 1 lot = 10 XLM (base decimals 7)
display_base_decimals = 7
tick = 1000                                # $0.00001 per XLM
imf_bps = 2000                             # 5x max
mmf_bps = 1000
taker_fee_bps = 5
maker_fee_bps = 0
liq_fee_bps = 100
band_bps = 500
max_position_lots = 100000
max_oi_lots = 1000000
impact_lots = 1000                         # 10,000 XLM
```

What this shows about "configurable lanes" (for the demo and docs):
- block time is a node setting;
- there is no gas, only trading fees in USDC;
- the validator set and threshold live in the settlement contract (§13);
- access can be open or an allowlist;
- execution caps are per lane.

---

## 11. Perps engine (`lanes/perps/engine/crates/caravel-perps`)

Pure, deterministic, `no_std + alloc`. Generic over:

```rust
pub trait Crypto {
    fn sha256(&self, data: &[u8]) -> [u8; 32];
    /// MUST trap/panic on invalid signature, same as soroban `ed25519_verify`.
    fn ed25519_verify(&self, public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]);
}
```

Public API (the engine contract and the native tests call these):

```rust
pub fn genesis(config_bytes: &[u8], c: &impl Crypto) -> Result<Vec<u8>, Fatal>;   // StateV1 bytes
pub fn step(state_bytes: &[u8], block_bytes: &[u8], c: &impl Crypto)
    -> Result<StepOutput, Fatal>;                                                  // new state + receipts
pub struct StepOutput { pub state: Vec<u8>, pub receipts: Vec<u8> }                  // receipts: §11.10
pub struct Fatal { pub code: u16, pub entry_index: u32 }                             // 0xFFFF_FFFF = block-level
```

Fatal codes (`caravel-types::fatal`):

| Code | Name |
|---:|---|
| 1 | BAD_STATE_ENCODING |
| 2 | BAD_BLOCK_ENCODING |
| 3 | BAD_CONFIG |
| 4 | WRONG_LANE_BLOCK |
| 5 | BAD_HEIGHT |
| 6 | BAD_PREV_HASH |
| 7 | TIME_REGRESSION |
| 8 | BLOCK_TOO_LARGE |
| 9 | TOO_MANY_ENTRIES |
| 10 | ENTRY_ORDER |
| 11 | INBOX_GAP |
| 12 | BAD_ENTRY_ENCODING |
| 13 | UNKNOWN_ORACLE_KEY |
| 14 | DUPLICATE_ORACLE_MARKET |
| 15 | PENDING_QUEUE_OVERFLOW |
| 16 | ARITHMETIC_OVERFLOW (an invariant-level overflow, not user input) |
| 17 | BAD_SIGNATURE (native path only; in Wasm the host traps) |

How fatals surface:
- In the engine contract, a fatal becomes `panic_with_error!` with the code. The entry index is lost.
- An invalid signature traps inside `ed25519_verify` with no code.
- To find the offending entry, the sequencer re-runs the block natively with a diagnostic `Crypto` impl that returns `Fatal { code: 17, entry_index }` instead of panicking (§14.1).

### 11.1 Units

- **Collateral and fees:** USDC stroops, `i128` (1 USDC = 10^7).
- **Size:** `lots`, `i64`. Positions are signed: long > 0, short < 0.
- **Price:** USDC stroops **per lot**, `i64 > 0`, and a multiple of the market's `tick`.
  - The oracle feeder converts USD/base prices to this unit (§17.3). All math is integer with no scale factors.
- **Notional:** `notional(lots, price) = |lots| × price` as `i128`.
- **Bps:** 1/10,000. **Ppm:** 1/1,000,000.

### 11.2 Block execution order (`step`)

1. Decode state and block (fatal on error).
2. Check:
   - `block.lane_id == state.lane_id`;
   - `block.height == state.height + 1`;
   - `block.prev_block_hash == expected_prev` (see below);
   - `block.timestamp_ms ≥ state.last_timestamp_ms`;
   - byte size, entry count, entry order and one-oracle-per-market rules (§9.6).
   Any failure is fatal.

   `expected_prev` is `[0; 32]` when `state.height == 0`. Otherwise it is `H(state.last_block_input_hash || H(state_bytes))`. Here `state_bytes` is the input state, so `H(state_bytes)` is the previous block's `state_hash_after`.
3. Reset `txs_this_block` for all accounts.
4. **Funding** (§11.6) for each market whose interval boundary was crossed.
5. **INBOX** entries in order (§11.4).
6. **ORACLE** entries in order (§11.5).
7. **USER** entries in order (§11.3).
8. **Liquidations** (§11.7).
9. If `flags & CHECKPOINT_END`, compute the **commitment** (§11.8).
10. Set `height = block.height`, `last_timestamp_ms = block.timestamp_ms` and `last_block_input_hash = H(BlockInputV1 bytes)`.
11. Encode and return state and receipts.

This makes the engine enforce the full block chain on its own:
- `block_hash = H(input_hash || state_hash_after)` (§9.6);
- the state stores only the input hash, so there is no cycle.

### 11.3 User transactions (`USER` entries)

For each `LaneTxV1`:

1. Decode. A decode failure is **fatal**; the sequencer MUST never include one.
2. `lane_id` mismatch → reject `WRONG_LANE`.
3. Account lookup by `account` key; missing → reject `UNKNOWN_ACCOUNT`.
4. Signer rules:
   - If `signer == account`, the key is valid.
   - Else `signer` must be in the account's session keys, with `expires_at_ms > block.timestamp_ms` and permissions allowing `kind` (§9.3). Otherwise reject `UNAUTHORIZED_SIGNER`.
   - `sig_scheme = 1` with `signer != account` → reject `UNAUTHORIZED_SIGNER`.
5. Verify the signature (§9.2). Failure is **fatal**; the sequencer pre-verifies (§8.4).
6. `block.timestamp_ms > expiry_ms` → reject `EXPIRED`; the nonce is not consumed.
7. `nonce != account.next_nonce` → reject `BAD_NONCE`; the nonce is not consumed.
8. `txs_this_block ≥ max_txs_per_account_per_block` → reject `RATE_LIMITED`; the nonce is not consumed.
9. **Consume the nonce:** `next_nonce += 1` and `txs_this_block += 1`. From here on, a rejection still consumes the nonce.
10. Dispatch by kind:
    - PLACE_ORDER (§11.3.1);
    - CANCEL_ORDER: the order must exist on that market and belong to the account, else `ORDER_NOT_FOUND`;
    - CANCEL_ALL;
    - WITHDRAW (§11.3.2);
    - ADD_SESSION_KEY, REVOKE_SESSION_KEY (§11.3.3).

Reason codes are `u16` constants in `caravel-types::codes`. Keep this exact numbering:

| Code | Name |
|---:|---|
| 0 | OK |
| 1 | WRONG_LANE |
| 2 | UNKNOWN_ACCOUNT |
| 3 | UNAUTHORIZED_SIGNER |
| 4 | EXPIRED |
| 5 | BAD_NONCE |
| 6 | RATE_LIMITED |
| 10 | UNKNOWN_MARKET |
| 11 | BAD_PRICE |
| 12 | BAD_SIZE |
| 13 | ORACLE_STALE |
| 14 | OUTSIDE_PRICE_BAND |
| 15 | TOO_MANY_OPEN_ORDERS |
| 16 | REDUCE_ONLY_VIOLATION |
| 17 | POST_ONLY_WOULD_CROSS |
| 18 | INSUFFICIENT_MARGIN |
| 19 | POSITION_LIMIT |
| 20 | OPEN_INTEREST_LIMIT |
| 21 | BOOK_FULL |
| 22 | (reserved) |
| 30 | ORDER_NOT_FOUND |
| 40 | BELOW_MIN_WITHDRAWAL |
| 41 | INSUFFICIENT_FREE_COLLATERAL |
| 42 | WITHDRAWAL_QUEUE_FULL |
| 43 | INSUFFICIENT_LANE_LIQUIDITY |
| 50 | TOO_MANY_SESSION_KEYS |
| 51 | BAD_SESSION_KEY |

#### 11.3.1 PLACE_ORDER

Validation, in order. Each failure is a rejection with the code shown:

1. The market exists → `UNKNOWN_MARKET`.
2. `price > 0 && price % tick == 0` → `BAD_PRICE`. `0 < lots ≤ max_position_lots` → `BAD_SIZE`.
3. Reduce-only rules:
   - `reduce_only == 1` requires `tif == IOC`, the side opposite the current position, and `lots ≤ |position|` → `REDUCE_ONLY_VIOLATION`.
   - Reduce-only orders skip checks 4, 8 and 9.
4. Staleness: `oracle_age ≤ oracle_max_staleness_ms` and `oracle_price > 0` → `ORACLE_STALE`.
   - `oracle_age = block.timestamp_ms.saturating_sub(oracle_time_ms)` everywhere in the engine. Oracle times may be slightly ahead of block time (§11.5).
5. Price band: `|price − mark| × 10_000 ≤ band_bps × mark` → `OUTSIDE_PRICE_BAND`. Here `mark = oracle_price`.
6. `tif == POST_ONLY` and the order would cross the best opposite price → `POST_ONLY_WOULD_CROSS`.
7. `tif != IOC` and `open_order_count ≥ max_open_orders_per_account` → `TOO_MANY_OPEN_ORDERS`.
   - `tif != IOC` and that side of the book is at `max_orders_per_side` → `BOOK_FULL`.
8. Position and OI, worst case including resting orders. With `s` = current position, `B`/`A` = the account's resting bid/ask lots on this market, and the new order added to its side:
   - `max(|s + B'|, |s − A'|) ≤ max_position_lots` → `POSITION_LIMIT`, where `B' = B + lots` for a buy (`A' = A`), or `A' = A + lots` for a sell;
   - `open_interest_lots + lots ≤ max_oi_lots` → `OPEN_INTEREST_LIMIT`. This is conservative for the new order. Fills of orders that already rest can still push OI past the cap, by at most the resting volume. That is accepted in M0.
9. Margin, worst case (§11.3.4): `free_collateral_after_worst_case ≥ 0` → `INSUFFICIENT_MARGIN`.

Matching (price-time priority; execution at the maker's price):

```text
remaining = lots
while remaining > 0 and best opposite order crosses (buy: ask.price ≤ price; sell: bid.price ≥ price):
    maker = best opposite order
    if maker.account_index == taker index:              # self-trade prevention: cancel resting
        remove maker; maker account open_order_count -= 1; emit ORDER_CANCELED(STP); continue
    q = min(remaining, maker.lots_remaining)
    fill(taker, side, q, maker.price); fill(maker_account, opposite, q, maker.price)   # §11.3.5
    charge_fee(taker, q, maker.price, taker_fee_bps); charge_fee(maker, q, maker.price, maker_fee_bps)
    maker.lots_remaining -= q; if 0: remove maker, maker open_order_count -= 1
    remaining -= q; emit FILL
if remaining > 0:
    GTC or POST_ONLY → rest order (order_id = next_order_id++, append at price level, keep sort), open_order_count += 1, emit ORDER_RESTED
    IOC → emit ORDER_CANCELED(IOC_REMAINDER)
```

#### 11.3.2 WITHDRAW

Checks, in order:
1. `amount ≥ min_withdrawal` → `BELOW_MIN_WITHDRAWAL`.
2. `pending_count < max_pending_withdrawals` → `WITHDRAWAL_QUEUE_FULL`.
3. The oracle is fresh for every market where the account has a position → `ORACLE_STALE`.
4. `amount ≤ collateral`: only realized cash can leave, not unrealized PnL.
5. `amount ≤ free_collateral(account)`, using IM including open orders (§11.3.4). Checks 4 and 5 both → `INSUFFICIENT_FREE_COLLATERAL`.
6. Lane liquidity: `Σ pending.amount + amount ≤ deposits_credited_total − withdrawals_committed_total` → `INSUFFICIENT_LANE_LIQUIDITY`. This is INV-P7, and it guarantees §13.3 check 8 can never fail for an honest engine.

Effect: `collateral −= amount`; push `(account.key, amount)` to `pending`.

#### 11.3.3 Session keys

- ADD: `session_key != account` and not already present → `BAD_SESSION_KEY`.
- ADD: `expires_at_ms > block.timestamp_ms`, and at most `block.timestamp_ms + 7 days` → `BAD_SESSION_KEY`.
- ADD: `permissions & !0x03 == 0` → `BAD_SESSION_KEY`.
- ADD: `count < max_session_keys` → `TOO_MANY_SESSION_KEYS`.
- ADD effect: insert, keeping the list sorted by key.
- REVOKE: the key must be present → `BAD_SESSION_KEY`. Effect: remove it.

#### 11.3.4 Margin formulas

For account `a`, market `m`:
- `s` = signed lots;
- `C` = cost basis (signed, `Σ Δlots × price`);
- `M` = `oracle_price`;
- `B` = resting bid lots;
- `A` = resting ask lots.

```text
upnl_m        = s × M − C
equity E      = collateral + Σ_m upnl_m
exposure_m    = max(|s + B|, |s − A|)
IM_m          = mul_div_ceil(exposure_m × M, imf_bps, 10_000)
MM_m          = mul_div_ceil(|s| × M, mmf_bps, 10_000)
free_collat   = E − Σ_m IM_m
```

Worst-case check for a new order of `lots` at limit `p` on side `side`:
1. Add `lots` to `B` (buy) or `A` (sell) for `exposure_m`.
2. Subtract `fee_max = mul_div_ceil(lots × p, taker_fee_bps, 10_000)`.
3. Subtract `max(0, lots × (p − M))` for a buy, or `max(0, lots × (M − p))` for a sell.
4. Require the resulting `free_collat ≥ 0`.

#### 11.3.5 `fill(account, Δ, p)` (Δ signed: +buy, −sell)

```text
if s == 0 or sign(Δ) == sign(s):
    C += Δ × p ; s += Δ
else:
    q      = sign(Δ) × min(|Δ|, |s|)                     # closing part
    C_part = mul_div_trunc(C, |q|, |s|)                  # proportional cost basis removed
    realized = −(q × p) − C_part
    collateral += realized ; C −= C_part ; s += q
    rest = Δ − q                                         # flip remainder
    if rest != 0: C += rest × p ; s += rest
update market open_interest_lots incrementally: OI += max(s_new,0) − max(s_old,0)
```

Property (proved in §11.9): for any account, `collateral − C` changes by exactly `−Δ × p`, whatever the rounding of `C_part`. This is why totals are conserved.

`charge_fee(account, q, p, bps)`:
- `fee = mul_div_ceil(q × p, bps, 10_000)`;
- `account.collateral −= fee`;
- `ins = mul_div_floor(fee, insurance_fee_share_bps, 10_000)`;
- `backstop.collateral += ins`;
- `treasury.collateral += fee − ins`.

### 11.4 INBOX entries

For each message:
1. It MUST have `index == state.inbox_through`; otherwise **fatal**.
2. Update `inbox_acc = H(TAG_INBOX || inbox_acc || msg)` and `inbox_through += 1`.

Then by kind:

- **DEPOSIT (0):**
  - Always first: `deposits_credited_total += amount`.
  - If the account exists: `collateral += amount`.
  - Else, if the access rules allow `lane_account` (OPEN, or in the allowlist) and a slot is available: create the account in that slot, then credit it.
    - A slot is a new index if `account_count < max_accounts`.
    - Otherwise it is the lowest-index **empty** non-system account. Empty means `collateral == 0`, no positions, no open orders, and no pending withdrawal for its key. The slot is overwritten in place, so indices do not shift.
    - New accounts start with `next_nonce = block.timestamp_ms`. This way, transactions signed for an evicted earlier holder of the same key can never replay.
  - Else **bounce**: push `(lane_account, amount)` to `pending`, which refunds it via the withdrawal root.
  - If `pending` is full when a push is needed, the block is **fatal** (`PENDING_QUEUE_OVERFLOW`). The sequencer prevents this (§14.2).
- **FORCED_WITHDRAWAL (1):**
  - If the account does not exist, this is a no-op (still processed).
  - Otherwise:
    - cancel all of the account's open orders;
    - `amt = min(amount, collateral, max(0, free_collateral), liquidity_left)`, where `liquidity_left = deposits_credited_total − withdrawals_committed_total − Σ pending.amount`;
    - if `amt ≥ 1`, push `(key, amt)` to `pending` and `collateral −= amt`. Pending full → **fatal**, as above.
  - Positions are not force-closed in M0 (forced close is T-M1-08).
  - Emit `FORCED_WITHDRAWAL_PROCESSED` with `amt`.

### 11.5 ORACLE entries

- `oracle_key` must be in `config.oracle_keys`; otherwise **fatal** (`UNKNOWN_ORACLE_KEY`).
- There is at most one ORACLE entry per market per block; a second one is **fatal** (`DUPLICATE_ORACLE_MARKET`).
- The signature is verified (§9.5); failure is **fatal**.
- Non-fatal rejections (emit `ORACLE_REJECTED`, state unchanged):
  - unknown market;
  - `price ≤ 0`;
  - `publish_time_ms ≤ market.oracle_time_ms`;
  - `publish_time_ms > block.timestamp_ms + oracle_max_future_ms`;
  - `market.oracle_price > 0` and `|price − old| × 10_000 > allowed_bps × old`, where `allowed_bps = oracle_circuit_breaker_bps + oracle_breaker_bps_per_sec × div_floor(publish_time_ms − market.oracle_time_ms, 1000)`.
- The widening bound lets the price recover after an outage. One update per block limits how fast a key can walk the price. The breaker is protection against fat fingers, not against a malicious oracle key (§4.3).
- Accepted: set `oracle_price` and `oracle_time_ms`.

### 11.6 Funding

At block start, for each market with `oracle_price > 0`, `oracle_age ≤ oracle_max_staleness_ms`, and `block.timestamp_ms ≥ last_funding_time_ms + funding_interval_ms`:

```text
impact_bid = price of the bid level at which cumulative bid depth first reaches impact_lots (none if depth < impact_lots)
impact_ask = same on the ask side
if both exist:
    mid = div_floor(impact_bid + impact_ask, 2)
    premium_ppm = mul_div_floor(mid − M, 1_000_000, M)
else premium_ppm = 0                                      # thin books pay no funding
rate_ppm = clamp(div_trunc(premium_ppm, funding_damping), −max_rate_ppm, +max_rate_ppm)
fpl      = mul_div_floor(rate_ppm, M, 1_000_000)          # stroops per lot; >0 means longs pay
for each account with s ≠ 0 (index order): collateral −= s × fpl
cumulative_funding_per_lot += fpl
last_funding_time_ms = div_floor(block.timestamp_ms, funding_interval_ms) × funding_interval_ms
emit FUNDING(market, rate_ppm, fpl)
```

`Σ s = 0` per market, so the payments sum to exactly zero (INV-P2). No rounding dust exists.

At genesis, `last_funding_time_ms` = 0. The first funding happens after the first oracle price.

### 11.7 Liquidations (end of every block)

For each non-system account, in index order, that has any position:
- Skip the account if any market it has a position in has a stale oracle.
- If `E < Σ MM`:
  1. Cancel all its open orders.
  2. Compute the fee from the **pre-liquidation** positions: `fee_due = Σ_m mul_div_ceil(notional(s_m, M_m), liq_fee_bps_m, 10_000)`.
  3. For each market with `s ≠ 0`: `fill(account, −s, M)` and `fill(backstop, +s, M)`. This transfers the position to the backstop at mark and keeps `Σ s = 0`.
  4. Collect the fee:
     - `fee = min(fee_due, max(0, collateral))`, where `collateral` is the value after step 3;
     - move `fee` from the account to backstop collateral.
  5. If `collateral < 0`: `backstop.collateral += collateral` (a negative number) and `collateral = 0`. The loss is absorbed by the backstop.
  6. Emit `LIQUIDATION`.

After the loop:
- Set `flags.BACKSTOP_DEFICIT` if backstop equity `E_backstop < 0`; clear it otherwise.
- Emit `BACKSTOP_DEFICIT` whenever the flag changes.
- The flag does not restrict trading in M0.
- Solvency toward Stellar is still guaranteed by the lane-liquidity rule (INV-P7). A deficit means the last withdrawers may be limited by liquidity; auto-deleveraging is T-M2-04.
- The operator unwinds the backstop's positions by trading with `backstop_key` (§14.6). This is ordinary trading, so it can never deadlock.

### 11.8 Commitments (only on `CHECKPOINT_END`)

Let `seq = checkpoint_seq + 1`.

```text
for j in 0..account_count:
    escape_equity_j = max(0, E_j)    # E with current oracle prices, stale or not
    leaf_j = H(0x00 || TAG_ACCT_LEAF || lane_id || seq u64 || j u32 || key_j || escape_equity_j i128)
accounts_root = merkle_root(leaf_0..)            escape_total = Σ escape_equity_j

for i in 0..pending_count:
    leaf_i = H(0x00 || TAG_WDL_LEAF || lane_id || seq u64 || i u32 || key_i || amount_i i128)
withdrawals_root = merkle_root(leaf_0..)         withdrawals_total = Σ amount_i

last_commitment = { seq, last_block_height = block.height, accounts_root, account_count, escape_total,
                    withdrawals_root, withdrawal_count = pending_count, withdrawals_total,
                    inbox_through, inbox_acc }
withdrawals_committed_total += withdrawals_total ; pending = [] ; checkpoint_seq = seq
```

The engine does not know about Stellar. The node builds `CheckpointHeaderV1` from `last_commitment`, the batch and the settlement config (§14.4).

### 11.9 Invariants (property-tested after every block)

- **INV-P1 (conservation):**
  `Σ_accounts (collateral − Σ_m C_m) + Σ pending.amount == deposits_credited_total − withdrawals_committed_total`.
- **INV-P2 (zero sum):** for every market, `Σ_accounts s_m == 0`, and `open_interest_lots == Σ max(s_m, 0)`.
- **INV-P3 (book):**
  - bids are sorted by price desc then order_id asc;
  - asks by price asc then order_id asc;
  - no crossed book at rest (`best_bid < best_ask`);
  - every order has `lots_remaining > 0`;
  - `open_order_count` equals the number of the account's orders on the books.
- **INV-P4 (inbox):** `inbox_acc` equals the fold of `TAG_INBOX` over all processed messages.
- **INV-P5 (determinism):** native `step` output (state bytes **and** receipts bytes) == Wasm `step` output (T-005).
- **INV-P6 (no float, no time):** enforced by lint. `clippy::float_arithmetic` is denied in consensus crates.
- **INV-P7 (lane liquidity):** `Σ pending.amount ≤ deposits_credited_total − withdrawals_committed_total`. Every checkpoint therefore passes §13.3 check 8, because that check's right-hand side equals the same quantity.
- **INV-P8 (unique keys):** account keys are unique, and so are session keys within an account.

Proof sketch for INV-P1:
- `fill` changes `collateral − C` by `−Δp`. Buyer and seller have opposite `Δ`, so per fill the sum changes by 0.
- Fees, liquidation fees and funding only move collateral between accounts, and funding sums to zero by INV-P2.
- Deposits add to both sides of the equation.
- Withdrawals move collateral into `pending`, and at commitment `pending` moves into `withdrawals_committed_total`.

### 11.10 Receipts (informational, not committed)

Layout:
- `receipts` = magic `CVRCPT01` (8) · `count u32` · `count` × Receipt.
- Receipt = `entry_index u32 · status u8 (0 ok, 1 rejected) · code u16 · event_count u16 · events[]`.
- Event = `type u8 · len u16 · fields`, with fields fixed-width LE in the order listed below.

Pseudo-entries for block-level events:
- `entry_index = 0xFFFF_FFF0` is block start: funding, emitted before INBOX entries.
- `entry_index = 0xFFFF_FFF1` is block end: liquidations, backstop flag, commitment.
- Pseudo-entries appear only when they carry at least one event, and in that position.

| type | Name | Fields (widths) |
|---:|---|---|
| 1 | FILL | `market u16 · maker_order_id u64 · maker_idx u32 · taker_idx u32 · price i64 · lots i64 · taker_side u8` |
| 2 | ORDER_RESTED | `market u16 · order_id u64 · account_idx u32 · side u8 · price i64 · lots i64` |
| 3 | ORDER_CANCELED | `market u16 · order_id u64 · reason u8 (0 user, 1 STP, 2 IOC_REMAINDER, 3 LIQUIDATION, 4 FORCED_WITHDRAWAL)` |
| 4 | FUNDING | `market u16 · rate_ppm i32 · fpl i64` |
| 5 | LIQUIDATION | `account_idx u32 · fee i128 · deficit i128` |
| 6 | DEPOSIT | `key 32 · amount i128 · outcome u8 (0 credited, 1 created, 2 bounced)` |
| 7 | FORCED_WITHDRAWAL_PROCESSED | `key 32 · amount i128` |
| 8 | ORACLE | `market u16 · price i64 · accepted u8` |
| 9 | BACKSTOP_DEFICIT | `active u8 · backstop_equity i128` |
| 10 | COMMITMENT | `seq u64 · withdrawals_total i128 · escape_total i128` |

Receipts are part of the parity gate (INV-P5) but are not hashed into checkpoints. Changing their format needs a bump to `CVRCPT0x` but not a state-format bump.

---

## 12. Engine contract (`lanes/perps/engine/contracts/perps-engine`)

### 12.1 Interface

```rust
#[contract] pub struct PerpsEngine;
#[contractimpl] impl PerpsEngine {
    /// Rules identity; returns H(TAG "CARAVEL/ENGINE/V1" || spec_version) as BytesN<32>.
    pub fn version(env: Env) -> BytesN<32>;
    pub fn genesis(env: Env, config: Bytes) -> Bytes;                 // StateV1
    /// Returns "CVSTEP01" (8) · state_len u32 · state · receipts_len u32 · receipts
    pub fn step(env: Env, state: Bytes, block: Bytes) -> Bytes;
}
```

The contract has no storage. It copies `Bytes` into linear memory, calls `caravel_perps::step` with a `SorobanCrypto` impl (`env.crypto()`), and copies the output back. On a `Fatal`, it calls `panic_with_error!` with contract error code = fatal code (the `EngineError` enum, 1–17, is in the contract spec). An invalid signature traps inside `ed25519_verify` and is a host error, never a contract error code. The engine allocates through the `soroban-sdk` `alloc` feature (a bump allocator that never frees), so every allocation in a `step` call counts toward `exec_mem_limit` (DEC-026).

### 12.2 Size and cost budget

- Wasm ≤ 120,000 bytes (hard limit 131,072). Achieve this with `lto`, `codegen-units = 1`, `panic = "abort"`, `strip`, and no `format!`/`core::fmt` in hot paths. The size-optimized `opt-level = "z"` fits easily (63,090 bytes, T-004) but makes the interpreted engine 2.7–4.6× more expensive, so the profile uses `opt-level = 2` (DEC-027): **84,764 bytes**, sha256 `4571cd25…bf0a` (T-005), identical from a clean clone in another directory.
  - If the Wasm is over budget, first remove formatting and generics bloat. Only then consider splitting (SoroDOOM rule: benchmark before splitting).
- CPU per block in the host at full caps SHOULD stay ≤ 100M instructions. Measure it in T-005.
  - If over, reduce caps in `lane.toml` before optimizing algorithms.
  - Measured 2026-09-29 (`docs/BENCHMARKS.md`). At the original caps (1,024 accounts, 3 × 2 × 256 orders, 24,000-byte blocks) even an empty block cost 607M, over the 400M limit. After `opt-level = 2` (DEC-027) and computing resting lots once per block, the worst block at those caps was 270M. The testnet lane now uses 256 accounts, 128 orders per side and 12,000-byte blocks (DEC-028): the worst block measured is **95.8M** (IOC takers sweeping the book), and memory stays under 5 MB.

### 12.3 Release profile

```toml
[profile.release]
opt-level = 2                # not "z": see DEC-027
overflow-checks = true
debug = 0
strip = "symbols"
debug-assertions = false
panic = "abort"
codegen-units = 1
lto = true
```

Build with `scripts/build-contracts.sh`, i.e. `stellar contract build --locked --out-dir target/contracts` with the pinned CLI (DEC-020). Record `sha256sum` of the x86_64 Linux build (CI) in `versions.json` (DEC-033).

Checked 2026-09-29: plain `cargo build --target wasm32v1-none` is **not** an option for contracts. The `soroban-sdk 28.0.0` build script exits with an error unless `SOROBAN_SDK_BUILD_SYSTEM_SUPPORTS_SPEC_SHAKING_V2` is set, which `stellar-cli ≥ 25.2.0` does. The CLI runs `cargo rustc --locked --crate-type=cdylib --target=wasm32v1-none --release` with `--remap-path-prefix` for the registry. Crates that do not depend on `soroban-sdk` (`caravel-types`, `caravel-merkle` without `soroban`, `caravel-perps`) still build with plain cargo, and CI checks that they do.

---

## 13. Settlement contract (`platform/contracts/settlement`)

Soroban contract, `soroban-sdk =28.0.0`. One instance per lane.

### 13.1 Types

```rust
#[contracttype] pub struct WeightedSigner { pub key: BytesN<32>, pub weight: u32 }
#[contracttype] pub struct WeightedSigners { pub signers: Vec<WeightedSigner>, pub threshold: u32 }   // keys strictly ascending
#[contracttype] pub struct Sig { pub signer_index: u32, pub signature: BytesN<64> }                   // indexes strictly ascending
#[contracttype] pub struct Params {
    pub force_inclusion_window_secs: u64,   // M0 testnet default 3600; demo value 600
    pub escape_timeout_secs: u64,           // M0 testnet default 21600; demo value 1200
    pub min_rotation_delay_secs: u64,       // 3600
    pub signer_retention_epochs: u32,       // 2
    pub min_deposit: i128,                  // same as genesis config
}
#[contracttype] pub struct Config {
    pub admin: Address, pub usdc: Address, pub lane_id: BytesN<32>,
    pub engine_wasm_hash: BytesN<32>, pub genesis_state_hash: BytesN<32>, pub config_hash: BytesN<32>,
    pub params: Params,
}
#[contracttype] pub struct LastCheckpoint {
    pub seq: u64, pub header_hash: BytesN<32>, pub last_block_height: u64, pub last_block_hash: BytesN<32>,
    pub last_block_timestamp_ms: u64, pub state_hash: BytesN<32>,
    pub accounts_root: BytesN<32>, pub account_count: u32, pub escape_total: i128,
    pub inbox_through: u64, pub accepted_at: u64,          // ledger timestamp (secs)
}
#[contracttype] pub struct CheckpointRecord {
    pub header_hash: BytesN<32>, pub withdrawals_root: BytesN<32>, pub withdrawal_count: u32,
    pub withdrawals_total: i128, pub claimed_total: i128, pub stellar_ledger: u32,
}
#[contracttype] pub struct InboxMsg {
    pub kind: u32, pub from: Address, pub lane_account: BytesN<32>, pub amount: i128,
    pub enqueued_at: u64, pub acc_after: BytesN<32>, pub cum_deposits_after: i128, pub refunded: bool,
}
#[contracttype] pub struct FrozenInfo { pub at: u64, pub payout_num: i128, pub payout_den: i128 }
#[contracttype] pub enum DataKey {
    Config, Epoch, MinValidEpoch, LastCkpt, InboxCount, Frozen, LastRotationAt,
    TotalWithdrawalsCommitted, TotalWithdrawalsClaimed,
    Signers(u64), SignersEpoch(BytesN<32>), Inbox(u64), Ckpt(u64),
    Claimed(u64, u32), EscapeClaimed(BytesN<32>),
}
```

Storage classes:
- **Instance:** `Config`, `Epoch`, `MinValidEpoch`, `LastCkpt`, `InboxCount`, `Frozen`, `LastRotationAt`, and the two totals.
- **Persistent:** everything else.
- **TTL:** extend instance storage on every call. Extend a persistent entry when it is read or written and its TTL is below 30 days, targeting 120 days (≈ 17,280 ledgers per day). Both are clamped to `env.storage().max_ttl()`; testnet's maximum is 3,110,400 ledgers (§3.3), above the 2,073,600-ledger target.

### 13.2 Functions

| Function | Auth | Effect |
|---|---|---|
| `__constructor(admin, usdc, lane_id, engine_wasm_hash, genesis_state_hash, config_hash, signers: WeightedSigners, params)` | — | Validate signers (§13.4). `Epoch = 1`, `MinValidEpoch = 1`, `LastCkpt.seq = 0` with `state_hash = genesis_state_hash`, zero roots, `accepted_at = now` |
| `deposit(from, amount, lane_account) -> u64` | `from.require_auth()` | not frozen; `amount ≥ min_deposit`; USDC `transfer(from, this, amount)`; append inbox kind 0; emit `inbox` event; return index |
| `request_forced_withdrawal(owner, lane_account, amount) -> u64` | `owner.require_auth()` | not frozen; `owner.to_xdr() == ACCOUNT_XDR_PREFIX || lane_account` (§13.5); `amount > 0`; append inbox kind 1 |
| `submit_checkpoint(header: Bytes, batch: Bytes, epoch: u64, sigs: Vec<Sig>)` | none (signatures are the auth) | §13.3 |
| `claim_withdrawal(recipient, lane_account, seq, index, amount, proof: Vec<BytesN<32>>)` | none | §13.5; allowed when frozen |
| `rotate_signers(new: WeightedSigners, epoch: u64, sigs: Vec<Sig>)` | current signers | §13.4 |
| `admin_rotate_signers(new: WeightedSigners)` | `admin.require_auth()` | **testnet only**; emergency rotation: same effect without the delay or signatures (including the reuse rule and `LastRotationAt = now`, DEC-031), **and** sets `MinValidEpoch = Epoch` (old sets are invalid immediately); emits `admin_rotate` |
| `freeze()` | none | §13.6 |
| `escape_claim(recipient, lane_account, index: u32, equity: i128, proof)` | none | §13.6 |
| `refund_unprocessed_deposit(index: u64)` | none | §13.6 |
| `upgrade(new_wasm_hash)` | admin | **testnet only**; emits `upgrade` |
| views | — | `config()`, `last_checkpoint()`, `checkpoint(seq)`, `inbox(index)`, `inbox_count()`, `signers(epoch)`, `epoch()`, `min_valid_epoch()`, `frozen()`, `frozen_info()`, `is_claimed(seq, index)`, `escape_claimed(lane_account)`, `totals()` |

Events are defined with `#[contractevent(topics = [...])]` structs and published with `env.events().publish_event(&ev)`. In soroban-sdk 28.0.0, `env.events().publish` is `#[deprecated]` (checked 2026-09-29 in `src/events.rs`):

| Event | Topics | Data |
|---|---|---|
| `Inbox` | `("inbox", index)` | kind, lane_account, amount, enqueued_at, acc_after, from |
| `Checkpoint` | `("ckpt", seq)` | header_hash, last_block_height, withdrawals_total |
| `Claimed` | `("claim", seq, index)` | recipient, amount |
| `Frozen` | `("frozen",)` | payout_num, payout_den |
| `EscapeClaimed` | `("escape", lane_account)` | amount |
| `SignersRotated` | `("rotate", epoch)` | signers_hash |
| `AdminRotate` | `("admin_rotate", epoch)` | signers_hash |
| `Upgrade` | `("upgrade",)` | new_wasm_hash |
| `Refund` | `("refund", index)` | from, amount (DEC-030) |

Inbox append (shared by `deposit` and `request_forced_withdrawal`):
1. `index = InboxCount`.
2. Build `InboxMsgV1` bytes (§9.4) with `enqueued_at = env.ledger().timestamp()`.
3. `acc_after = H(TAG_INBOX || acc_prev || msg)`, where `acc_prev` is `Inbox(index-1).acc_after`, or zero for index 0.
4. `cum_deposits_after = cum_prev + (kind == 0 ? amount : 0)`.
5. Store `Inbox(index)` and increment `InboxCount`.

### 13.3 `submit_checkpoint`: checks in order

Reject (panic with a `contracterror` code) on the first failure:

1. Not frozen.
2. `header.len() == 442`, magic `CVCKPT01`, version 1.
3. Identity fields:
   - `lane_id == config.lane_id`;
   - `network_id == env.ledger().network_id()`;
   - `settlement_addr_hash == H(env.current_contract_address().to_xdr(&env))`;
   - `engine_wasm_hash == config.engine_wasm_hash`.
4. Chain fields:
   - `seq == LastCkpt.seq + 1`;
   - `prev_header_hash == LastCkpt.header_hash` (zero if seq 1);
   - `first_block_height == LastCkpt.last_block_height + 1`;
   - `last_block_height ≥ first_block_height`;
   - `last_block_timestamp_ms ≥ LastCkpt.last_block_timestamp_ms`;
   - `last_block_timestamp_ms ≤ (ledger_timestamp + 60) × 1000`.
5. `H(batch) == header.batch_hash`, and `batch.len() ≤ 96_000`.
   - The contract does **not** parse the batch; validators and replayers do.
   - Binding: the batch's first block `prev_block_hash` is checked by replay against `LastCkpt.last_block_hash`, and `last_block_hash` in the header is signed.
6. Inbox:
   - `LastCkpt.inbox_through ≤ header.inbox_through ≤ InboxCount`;
   - `header.inbox_acc == (inbox_through == 0 ? zero : Inbox(inbox_through − 1).acc_after)`.
7. Signatures:
   - `MinValidEpoch ≤ epoch ≤ Epoch` and `Epoch − epoch ≤ signer_retention_epochs`, and `Signers(epoch)` exists;
   - `sigs` indexes strictly ascending and `< signers.len()`;
   - for each `sig`: `ed25519_verify(signers[i].key, header_hash, sig)` (traps on a bad signature), summing weight;
   - `Σ weight ≥ threshold`.
8. Solvency of new withdrawals:
   - `outstanding = TotalWithdrawalsCommitted − TotalWithdrawalsClaimed`;
   - `unprocessed_deposits = cum(InboxCount) − cum(header.inbox_through)`;
   - require `withdrawals_total ≤ usdc.balance(this) − outstanding − unprocessed_deposits`.
9. Effects:
   - store `Ckpt(seq)`;
   - update `LastCkpt`, with `accepted_at = now`;
   - `TotalWithdrawalsCommitted += withdrawals_total`;
   - emit `Checkpoint`.

### 13.4 Signer sets (Axelar-style)

- Valid set:
  - `1 ≤ n ≤ 32`;
  - keys strictly ascending;
  - every weight > 0;
  - `0 < threshold ≤ Σ weights`.
- `signers_hash = H(TAG_SIGNERS || n u32 || Σ(key || weight u32) || threshold u32)`.
- `rotate_signers(new, epoch, sigs)`:
  - `epoch == Epoch`, the current set only;
  - `now − LastRotationAt ≥ min_rotation_delay_secs`;
  - `msg = H(TAG_ROTATE || lane_id || network_id || settlement_addr_hash || (Epoch+1) u64 || signers_hash(new))`;
  - signatures must meet the current set's threshold;
  - reject if `SignersEpoch(signers_hash(new))` already exists (no re-use of a past set);
  - effect: `Epoch += 1`, `Signers(Epoch) = new`, `SignersEpoch(hash) = Epoch`, `LastRotationAt = now`.
- M0 default set: 3 validators, weight 1 each, threshold 2.

### 13.5 Withdrawal claims

- `ACCOUNT_XDR_PREFIX = 00 00 00 12 00 00 00 00 00 00 00 00` (12 bytes). This is the XDR of `ScVal::Address(ScAddress::Account(PublicKey::Ed25519(key)))` without the key. Assert this in a unit test against `recipient.to_xdr(&env)`.
- `claim_withdrawal(recipient, lane_account, seq, index, amount, proof)`:
  1. `recipient.to_xdr(&env) == ACCOUNT_XDR_PREFIX || lane_account`. Only the owner's `G...` address can receive.
  2. `Ckpt(seq)` exists; `index < withdrawal_count`; `!Claimed(seq, index)`.
  3. `leaf = H(0x00 || TAG_WDL_LEAF || lane_id || seq || index || lane_account || amount)`; verify the proof against `withdrawals_root` (§9.9).
  4. Effects:
     - set `Claimed(seq, index)`;
     - `Ckpt(seq).claimed_total += amount`;
     - `TotalWithdrawalsClaimed += amount`;
     - USDC `transfer(this, recipient, amount)`;
     - emit `Claimed`.
- Anyone may submit a claim for anyone, since funds only go to the owner. The recipient needs a USDC trustline. The web app checks this first.

### 13.6 Freeze and escape

- `freeze()` is allowed if not frozen and either:
  - **(A)** `now − LastCkpt.accepted_at > escape_timeout_secs`; or
  - **(B)** `InboxCount > LastCkpt.inbox_through` and `now − Inbox(LastCkpt.inbox_through).enqueued_at > force_inclusion_window_secs`.
- On freeze:
  - `available = usdc.balance(this) − outstanding − unprocessed_deposits`, computed as in §13.3 check 8 but using `LastCkpt.inbox_through`;
  - `payout_den = LastCkpt.escape_total`;
  - `payout_num = min(available, payout_den)`;
  - store `Frozen`; emit `Frozen`.
- After freeze:
  - `deposit`, `request_forced_withdrawal` and `submit_checkpoint` fail;
  - `claim_withdrawal` still works.
- `escape_claim(recipient, lane_account, index, equity, proof)`:
  1. Frozen; the recipient check is the same as §13.5.
  2. `!EscapeClaimed(lane_account)`.
  3. `leaf = H(0x00 || TAG_ACCT_LEAF || lane_id || LastCkpt.seq || index || lane_account || equity)`, verified against `LastCkpt.accounts_root` with `account_count`.
  4. Pay `mul_div_floor(equity, payout_num, payout_den)` (0 if `payout_den == 0`); mark claimed.
- `refund_unprocessed_deposit(index)`:
  - frozen; `index ≥ LastCkpt.inbox_through`; kind 0; not refunded;
  - transfer `amount` to `from`; mark refunded.
- There is no unfreeze. For demos, deploy a fresh instance.

### 13.7 Required tests (soroban-sdk testutils)

- Setup: register USDC with `env.register_stellar_asset_contract_v2(issuer)` (exists in soroban-sdk 28.0.0, checked 2026-09-29), `mock_all_auths` for happy paths, plus explicit auth tests.
- Happy path: deposit → checkpoint seq 1 (with a withdrawal leaf) → claim.
- Rejects for **every** check in §13.3, one test per numbered check.
- Double-claim rejected, wrong recipient rejected, proof of wrong length rejected.
- Signatures: bad signature traps; duplicate signer index rejected; below threshold rejected; old epoch within retention accepted, beyond retention rejected.
- Rotation: delay enforced; wrong epoch rejected.
- Freeze paths:
  - (A) via `env.ledger().with_mut(|l| l.timestamp += ...)`;
  - (B) with an overdue inbox message;
  - escape pro-rata with `payout_num < payout_den`;
  - refund of an unprocessed deposit;
  - post-freeze calls rejected.
- Golden vector: a header built by the Rust node code (T-001 vectors) verifies in the contract.
- Budget: `submit_checkpoint` with a 96,000-byte batch and 3 signatures stays under limits. Measure with `env.cost_estimate().resources()` on the contract registered from its Wasm (for a natively registered contract the VM costs are missing), under `budget().reset_limits` set to the §3.3 limits. Result in §3.3.

---

## 14. Sequencer (`caravel-perps-node sequencer`)

### 14.1 Responsibilities

- Accept `LaneTxV1` over HTTP. Pre-validate:
  - decode;
  - lane_id;
  - native signature verify;
  - `expiry > now`;
  - `nonce ≥ next_nonce` against a mempool view.
  Keep them in a FIFO mempool.
- Accept inbox messages and oracle updates from the relayer (internal API, bearer token).
- Every `block_time_ms`, build a `BlockInputV1`:
  - timestamp = wall clock ms, clamped to ≥ previous;
  - all newly known inbox messages in index order, up to the queue rule in §14.2;
  - the latest oracle update per market, only if its `publish_time_ms` is newer than state;
  - then user transactions, FIFO, while the block stays within both budgets:
    - **entries:** INBOX + ORACLE + USER ≤ `max_entries_per_block`. Reserve room for inbox and oracle entries first; USER gets the rest;
    - **bytes:** `BlockInputV1` ≤ `min(max_block_bytes, space left in the current batch)` (§14.2).
    Skip transactions whose nonce is not next (keep them for later blocks) and drop expired ones.
  - Put at most one ORACLE entry per market in a block.
  - Empty blocks are produced too (the clock keeps moving).
- Execute the block via the executor (§14.5), which yields the new state and receipts.
- Persist `BlockRecordV1`, state bytes and receipts in **one SQLite transaction**, then broadcast them over WebSocket. Never broadcast before persisting.
- On a `Fatal`, or a host error or budget exhaustion, from the Wasm executor:
  1. Re-run the block with the native engine and the diagnostic `Crypto` (§11) to get `Fatal { code, entry_index }`.
  2. Discard the block.
  3. Quarantine the offending entry: remove the user tx from the mempool, or mark the oracle update bad. For budget exhaustion, halve the USER entries.
  4. Log with the entry bytes.
  5. Rebuild the block for the same height immediately.
  - The engine should never be fatal on sequencer-validated input, so any fatal is a bug to fix.

### 14.2 Checkpoint policy

Batch accounting:
- `batch_used` = 52 bytes (batch header) + Σ over blocks already in the current batch of (4 + len(BlockInputV1) + 32).
- Before building a block, `space_left = max_batch_bytes − batch_used − 36`.
- The block is built with a byte budget of `min(max_block_bytes, space_left)`.

Set `CHECKPOINT_END` on the block being built when any of these holds:
- **(a)** `blocks_since_last_checkpoint + 1 == checkpoint_every_blocks`;
- **(b)** after adding this block, `max_batch_bytes − batch_used_after − 36 < max_block_bytes`. The next block might not fit, so end the batch now. This guarantees no batch exceeds `max_batch_bytes`, because every block is ≤ `max_block_bytes`;
- **(c)** pending-queue headroom:
  - `pending_at_block_start + inbox_entries_this_block + withdraw_txs_this_block ≤ max_pending_withdrawals` MUST hold for every block, since each of those can push one pending entry;
  - the sequencer includes fewer INBOX entries or WITHDRAW txs to keep it;
  - it sets END whenever `pending_at_block_start ≥ max_pending_withdrawals / 2`, so the queue drains at the commitment.

### 14.3 Checkpoint assembly

After a `CHECKPOINT_END` block:
1. Build `BatchV1` from the blocks since the last checkpoint.
2. Build `CheckpointHeaderV1`:
   - `last_commitment` fields come from state;
   - `network_id`, `settlement_addr_hash` and `engine_wasm_hash` come from node config;
   - `prev_header_hash` is the previous header's hash.
3. Pre-check what the contract will check:
   - `inbox_acc` matches the accumulator the relayer reported from Stellar for that index;
   - the solvency condition (INV-P7 makes it hold, but assert it);
   - header length and identity fields.
   Never ask validators to sign a header that would be rejected on Stellar, because validators never sign a second header for the same seq.
4. POST `{header, batch}` to every validator's `/v1/sign`. Collect signatures until the threshold weight is reached, with a 10s timeout and retries.
5. Append `{seq, header, batch, sigs, epoch}` to a **FIFO submission queue** in SQLite and expose the head to the relayer (§14.4). The relayer submits strictly in seq order.

The lane keeps producing blocks and sealing checkpoints while earlier ones wait. Back-pressure:
- if the queue holds ≥ 3 sealed checkpoints, the sequencer builds blocks with a byte budget of `max_block_bytes / 4` until the queue is < 2.
- Stellar can take roughly one 96 KB checkpoint per ledger (~5 s) per relayer account, so sustained lane data must stay below that rate.

### 14.4 Public and internal API

All JSON uses:
- lowercase hex for bytes;
- decimal strings for i128/u64 amounts;
- `G...` strkeys for account keys.

| Method & path | Purpose |
|---|---|
| `POST /v1/tx` | Body: raw `LaneTxV1` (`application/octet-stream`) or `{ "tx": "<hex>" }`. 202 `{tx_hash, status:"queued"}` or 400 `{error, code}` |
| `GET /v1/status` | lane_id, height, last checkpoint (sequenced, signed, accepted on Stellar), block_time_ms, validators |
| `GET /v1/accounts/{G...}` | collateral, equity, free collateral, positions (with upnl, liq price), open orders, next_nonce, session keys |
| `GET /v1/markets` | params + oracle price + funding |
| `GET /v1/markets/{id}/book?depth=50` | aggregated levels |
| `GET /v1/markets/{id}/trades?limit=100` | recent fills (from receipts) |
| `GET /v1/blocks/{height}` | record hex + decoded entries + receipts |
| `GET /v1/checkpoints/{seq}` | header (hex + decoded), batch hash, signatures, Stellar tx hash, status |
| `GET /v1/proofs/withdrawals?account=G...` | all unclaimed withdrawal leaves for the account: `{seq, index, amount, proof[]}` |
| `GET /v1/proofs/escape?account=G...` | leaf from the **last accepted** checkpoint: `{seq, index, equity, proof[]}` |
| `WS /v1/stream` | subscribe `{"blocks":true,"markets":[1,2,3],"account":"G..."}` → messages `block`, `fill`, `book`, `account`, `checkpoint` |
| `POST /internal/inbox` | relayer → sequencer: `{index, msg_hex, acc_after_hex}`; sequencer checks the acc chain matches its own fold, and on mismatch halts inbox inclusion and alerts |
| `POST /internal/oracle` | relayer → sequencer: `OracleUpdateV1` hex (pre-verified) |
| `GET /internal/checkpoints/pending` | relayer pulls `{seq, header, batch, epoch, sigs}` |
| `POST /internal/checkpoints/{seq}/accepted` | relayer reports `{stellar_tx_hash, ledger}` |

The sequencer must keep state history to serve proofs and replays: all blocks, plus a state snapshot at every checkpoint. It MAY prune other snapshots.

### 14.5 Executor (`platform/crates/caravel-runtime::executor`)

This is SoroDOOM's runner pattern, pinned to `soroban-env-host =28.0.2` (verify each call against docs.rs for that version):

```rust
let host = Host::test_host_with_recording_footprint();
host.set_ledger_info(LedgerInfo { protocol_version: 28, sequence_number: 1, timestamp: 0,
    network_id: LANE_EXEC_NETWORK_ID, base_reserve: 5_000_000, min_persistent_entry_ttl: 4_096,
    min_temp_entry_ttl: 16, max_entry_ttl: 6_312_000 })?;          // values do not affect engine output
host.as_budget().reset_unlimited()?;
let contract = host.register_test_contract_wasm_from_source_account(&wasm, generate_account_id(&host), [9; 32])?;
// per call:
host.as_budget().reset_limits(cfg.exec_cpu_limit, cfg.exec_mem_limit)?;   // consensus budget; also zeroes the counters
let out: BytesObject = host.call(contract, Symbol::try_from_small_str("step")?, host.vec_new_from_slice(&[state.into(), block.into()])?)?.try_into()?;
```

Rules:
- The engine MUST NOT read ledger info (timestamp, sequence), so these values cannot affect output. Add a test that runs the same block under two different `LedgerInfo` values and asserts identical output. (Done in T-005: identical output **and** identical metering.)
- Each call runs in a fresh host (DEC-029): register the Wasm with an unlimited budget, create the argument objects, then `reset_limits` and call. The budget covers exactly the `step` call, including Wasm parsing and instantiation, because the module cache stays off. A contract error maps to the fatal code; `ScErrorType::Crypto`/`InvalidInput` (the `ed25519_verify` trap) maps to `BAD_SIGNATURE`; `Budget`/`ExceededLimit` is budget exhaustion. All three are fatal.
- The per-call budget is **consensus**:
  - set CPU and memory limits to `config.exec_cpu_limit` and `config.exec_mem_limit` from `GenesisConfigV1` with `Budget::reset_limits(cpu, mem)`. Checked 2026-09-29 in `soroban-env-host 28.0.2` (`src/budget/util.rs`): it sets both limits and zeroes the consumed counters, and it is behind the `testutils` feature (DEC-029);
  - never use `reset_unlimited` or an unpinned default for `step` calls;
  - metering is deterministic, so every node exhausts the budget on the same block.
- Load the Wasm from a file and check `sha256 == engine_wasm_hash` at startup. Refuse to start on mismatch.
- Expose per-call `cpu_insns` and `mem_bytes` from the budget as metrics, labeled "host metering". Never label them network fees (SoroDOOM FEES doc).
- A native executor (`--executor native`) exists only for debugging. It MUST refuse to run when `--role validator` or `--role sequencer --production` is set.

### 14.6 Operator tasks

- `caravel-perps-node op backstop-unwind --market 1 --max-lots N`: signs orders with `backstop_key` to close backstop positions. Manual in M0.
- `caravel-perps-node op treasury-withdraw --amount X`: WITHDRAW from the treasury account.

---

## 15. Validator (`caravel-perps-node validator`)

- Config:
  - sequencer URL;
  - own ed25519 key (`S...` strkey file);
  - engine Wasm path + hash;
  - settlement contract ID, network passphrase, genesis config path;
  - its own SQLite path.
- Startup:
  - build genesis state from the config file and check `H == genesis_state_hash`;
  - catch up by fetching `/v1/blocks/{h}` from its last height.
- Follow: for each new `BlockRecordV1`:
  1. Check `prev_block_hash`.
  2. Execute `BlockInputV1` with its own executor.
  3. Check `H(new_state) == record.state_hash_after`.
  4. Persist.
  - On mismatch, stop following, raise an alert, and refuse all signing until an operator intervenes.
- Live policy checks (non-deterministic). They apply only to blocks received live, within 10s of production, and are skipped during catch-up:
  - block `timestamp_ms` is not more than 5s ahead of the validator's clock;
  - every ORACLE entry's `publish_time_ms` is within 60s of the validator's clock.
  - A failed live check marks the block `suspicious` and alerts. The validator still follows the chain, since state is deterministic, but refuses to sign the checkpoint containing that block until an operator clears it.
- The validator computes every checkpoint header itself from its own block store and state: the batch, the `last_commitment`, and `prev_header_hash` = its own computed header for `seq − 1`.
- `POST /v1/sign {header, batch}` → sign only if all of these hold:
  - the batch decodes and its blocks equal the validator's stored records byte for byte;
  - the header equals the header the validator computed for that `seq`, byte for byte;
  - `seq > last_signed_seq` (gaps are allowed, e.g. after a restart), or `seq == last_signed_seq` with an identical header hash (idempotent re-sign).
  Returns `{signer_key, signature}`.
- **Never sign two different headers for the same `seq`.** Persist every signed `(seq, header_hash)` before returning. This is what makes M1 equivocation slashing safe for honest validators.
- Stores a state snapshot at every checkpoint it computes, keeping at least the last 3 plus the one last accepted on Stellar (polled from `last_checkpoint()`).
- Serves from its own store, as redundant DA and an **independent proof source for the escape hatch**:
  - `GET /v1/blocks/{height}`, `GET /v1/checkpoints/{seq}`;
  - `GET /v1/proofs/withdrawals?account=G...`, `GET /v1/proofs/escape?account=G...` (same JSON as the sequencer).
- Validator URLs are listed in the web app config and on `/about`.

---

## 16. Replay verifier (`caravel-perps-node replay`)

### 16.1 Inputs

- `--rpc`, `--network-passphrase`, `--settlement <C...>`, `--genesis-config <file>`, `--engine-wasm <file>`.
- Optional `--from-archive <galexie bucket or local dir>` for checkpoints older than RPC retention.

### 16.2 Algorithm

1. Read `config()` from the contract. Check:
   - `H(GenesisConfigV1 bytes) == config_hash`, where the bytes are derived from the lane TOML by the same code as `caravel-perps-node genesis`;
   - `H(engine wasm) == engine_wasm_hash`;
   - `H(genesis(config)) == genesis_state_hash`.
2. List accepted checkpoints via `Checkpoint` events (`getEvents`) and `checkpoint(seq)` views.
3. For each seq, fetch the `submit_checkpoint` transaction (RPC `getTransaction` by hash, from event metadata) and extract the `header` and `batch` arguments from the envelope XDR.
4. Check `H(header) == Ckpt(seq).header_hash` and `H(batch) == header.batch_hash`.
5. Execute every block through the executor. Check:
   - each `state_hash_after`;
   - the header's `last_block_hash`, `state_hash`, all commitment fields and `inbox_acc`.
6. Output a report: `OK seq=1..N final_state_hash=...`, or the first mismatch with its location.
7. Optional proofs:
   - `--prove-escape G...` prints the escape proof JSON for that account from the last accepted checkpoint;
   - `--prove-withdrawals G...` prints unclaimed withdrawal proofs.
   Users can recover funds with nothing but Stellar RPC and this CLI.

### 16.3 Data retention

Stellar RPC keeps transactions only for a limited window: on testnet, 120,959 ledgers or 7.0 days (`getEvents` `oldestLedger` to `latestLedger`, checked 2026-09-29). For older data:
- replay from a Galexie/SEP-54 ledger-metadata archive; or
- replay from any validator's `/v1/blocks` store, cross-checking against on-chain header hashes.

Document this in RUNBOOK.

---

## 17. Relayer (`platform/relayer`, TypeScript)

Node ≥ 22 with `@stellar/stellar-sdk 17.2.0`. Contract calls go through `rpc.Server` with explicit `ScVal`s, not generated clients (DEC-040). One process runs the inbox and checkpoint loops, plus one loop per feed module the config names (M0.5, DEC-053). Each loop has its own key where signing is needed.

### 17.1 Inbox watcher

1. Poll `rpc.getEvents` every 2s for the settlement contract, topic `inbox`, from the last cursor, which is persisted in a local JSON/SQLite file.
2. For each event, read `inbox(index)` via simulation (read-only), then rebuild `InboxMsgV1` bytes.
3. POST to the sequencer `/internal/inbox`.
4. Idempotent by index.

### 17.2 Checkpoint submitter

1. Poll `/internal/checkpoints/pending`.
2. Before submitting, read `last_checkpoint()`:
   - if `seq` is already accepted, report accepted and continue. This covers "Stellar accepted but we crashed before recording" (SoroDOOM remaining-work item 1).
3. Build `submit_checkpoint(header, batch, epoch, sigs)`, then `prepareTransaction` (simulate), sign with the relayer key (an XLM-funded testnet account), send, and poll to success.
4. Report to `/internal/checkpoints/{seq}/accepted`.
5. Record `minResourceFee`, the fee charged and the tx size per checkpoint in metrics (feeds §19.6 cost reporting).

### 17.3 Oracle feeder

The perps lane's feed module, `lanes/perps/relayer-feeds` (DEC-053). The relayer config lists it under `feeds`, and it reads the oracle key from `CARAVEL_ORACLE_SECRET`. It posts to the perps node's feed route, `/internal/oracle`.

- Sources, in priority, configured per market (DEC-058):
  1. Coinbase's public WebSocket, `wss://ws-feed.exchange.coinbase.com`, on the `ticker` and `heartbeat` channels. A quote is fresh for 5 s from its trade, or from a heartbeat that names that trade as the product's last (for up to 120 s after the trade).
  2. Coinbase's public spot API.
  3. Reflector SEP-40 feeds on testnet: `lastprice(asset)`. The "External CEXs & DEXs" testnet feed is `CCYOZJCOPG34LLQQ7N24YXBM7LL62R7ONMZ3G6WZAAYPB5OYKOMJRN63`, 14 decimals, 300 s resolution (developers.stellar.org "Oracle Providers", 2026-09-08, and on-chain `decimals()`/`resolution()`, 2026-09-29; DEC-041).
- Every block (`intervalMs` 1,000 on testnet) per market:
  1. Fetch the USD price.
  2. Convert to stroops per lot:
     `price_per_lot = round_half_even(usd_price × 10^7 × display_lot_base_units / 10^display_base_decimals)`;
  3. Snap it to a multiple of `tick`.
  4. Sign `OracleUpdateV1` with the oracle key.
  5. POST to the sequencer.
- Skip a publish if the price moved less than 1 tick and less than 10s have passed.

---

## 18. Web app (`lanes/perps/web`)

### 18.1 Stack and brand

- React + Vite + TypeScript, with `@stellar/stellar-sdk` 17.2.0, `@creit.tech/stellar-wallets-kit` 2.7.0 (DEC-059; `@stellar/freighter-api` 6.0.1 in M0), `@noble/ed25519` 3.2.0 and `@noble/hashes` 2.4.0. Use `lightweight-charts` 5.2.1 for the price chart (versions pinned in `versions.json` and `lanes/perps/web/package.json`).
- Brand: the Caravel design system.
  - Font: Schibsted Grotesk 400/700.
  - Dark theme ("Stage") by default. Tokens:
    - `ground #081E22`, `ground-raised #102C31`, `line #4B7178`;
    - `ink #F3EEE4`, `ink-muted #A8BAB8`;
    - `lane #FF8C7C` (lane things only: blocks, soft confirmations), `lane-soft #3E1A18`;
    - `harbor #26958F` (Stellar things only: checkpoints, settlement, claims), `harbor-soft #0F3B3F`.
  - Never add a third accent. Text on `lane` or `harbor` fills uses `#081E22`.
- Copy rules:
  - The product is "Caravel", the chain is a "lane", Stellar is "Stellar" (never "L1").
  - No exclamation marks or hype.
  - Claims follow §2.

### 18.2 Pages

| Route | Content |
|---|---|
| `/trade/:market` | Oracle price chart, order book (depth 20), order form (limit / IOC / post-only, reduce-only for IOC), positions, open orders, recent fills. Status bar: lane height and "soft" badge in `lane`; last accepted checkpoint and "settled on Stellar" badge in `harbor` |
| `/portfolio` | USDC wallet balance (Stellar), lane collateral/equity/free collateral, Deposit, Withdraw, Claims (withdrawals ready to claim on Stellar), Forced withdrawal (advanced) |
| `/explorer` | Lane blocks, checkpoints with header fields, validator signatures, links to the Stellar testnet explorer for each checkpoint transaction |
| `/escape` | Shown only when `frozen()` is true. Explains the state. Gets the escape proof from, in order: the sequencer, each configured validator, or a proof JSON the user uploads (from `caravel-perps-node replay --prove-escape`). Calls `escape_claim`. Also handles refunds for unprocessed deposits |
| `/about` | Honest claims (§2.1) and what is not claimed (§2.2) |

### 18.3 Flows

All wallet calls go through Stellar Wallets Kit 2.7.0 (DEC-059), loaded on first use: `StellarWalletsKit.init({modules: defaultModules(), network, theme})`, `authModal()`, `getAddress()`, `getNetwork()`, `signTransaction(xdr, {networkPassphrase, address})` → `{signedTxXdr}` and `signMessage(message, {networkPassphrase, address})` → `{signedMessage}` (checked in its typings, 2026-09-30).
- **Connect:** the kit's wallet picker (`authModal`). Refuse a wallet whose `getNetwork()` is another network; a wallet that can't report its network is trusted with the passphrase sent on every signature. Check the USDC trustline and link to the Circle testnet faucet.
- **Deposit:** contract client `deposit({from, amount, lane_account: raw key of from})`, then `signTransaction` in the wallet, then send. Wait for the lane to credit it (poll `/v1/accounts`).
- **Enable fast trading:**
  1. Generate an ed25519 session key (`@noble/ed25519`) and store it in IndexedDB. This is acceptable on testnet; label it "trading key on this device".
  2. Sign `ADD_SESSION_KEY` (`PERM_TRADE|PERM_CANCEL`, 24h expiry) with the wallet's `signMessage` (scheme 1, message format §9.2).
  3. Check the signature before using it: decode it (base64 or hex, 64 bytes) and verify it against the SEP-53 hash with the account's key. A wallet that fails, or can't sign messages (Albedo, Rabet and Ledger in the kit), is told that trading needs a SEP-53 wallet; deposits, claims and escape still work with it.
  4. POST it.
- **Order:** build `PLACE_ORDER` with the next nonce (from `/v1/accounts`, then tracked locally), sign with the session key (scheme 0), POST, and show the receipt from the WS stream.
- **Withdraw:**
  1. Sign `WITHDRAW` with the wallet (scheme 1, checked as above).
  2. After the next accepted checkpoint, fetch `/v1/proofs/withdrawals`.
  3. Call `claim_withdrawal` through the wallet.
- **Nonce handling:** on `BAD_NONCE`, refetch the account and retry once.

### 18.4 TS codec

`lanes/perps/web/src/codec/` implements `LaneTxV1` encode, `tx_hash`, SEP-53 message building and Merkle proof verification (for display). Tests run against `test-vectors/*.json`, the same files the Rust tests use.

---

## 19. Testing strategy

### 19.1 Gates (CI, every PR)

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings      # plus deny(clippy::float_arithmetic) in consensus crates
./scripts/build-contracts.sh                                # builds both Wasm, checks size limits and recorded hashes (before the tests)
cargo test --workspace --locked                             # includes the parity gate on scenarios and 1,000 random blocks
cargo test --locked -p caravel-perps-node --test parity -- --ignored   # the 10,000-block parity gate (INV-P5)
npm --prefix platform/relayer ci && npm --prefix platform/relayer test
npm --prefix lanes/perps/web ci && npm --prefix lanes/perps/web test && npm --prefix lanes/perps/web run build
```

### 19.2 Golden vectors (`test-vectors/`)

- One JSON file per format: `{ "name", "fields": {...}, "hex": "...", "hash": "..." }`.
- Generated by a Rust binary and committed: `cargo gen-vectors`, an alias for `cargo run -p caravel-testkit --bin gen-vectors` (DEC-021, DEC-025). It writes the codec vectors (`caravel-types`), `merkle.json` (`caravel-merkle`) and `scenarios.json` (engine scenarios).
- Each file is `{ format, spec, hash_rule, context, vectors: [{ name, fields, hex, hash }], invalid: [{ name, hex, error }] }`. Integers are decimal strings and bytes are lowercase hex. `hash_rule` says which hash `hash` is (for example `tx_hash` for `LaneTxV1`).
- A unit test regenerates every file in memory and fails if a committed file is stale.
- Tests in Rust, the settlement contract and TS all load them.
- Changing a vector requires a DEC and a version bump (§0.3 rule 4).

### 19.3 Engine scenarios (native and Wasm, identical output required)

Name each as a test and keep its final `state_hash` in `test-vectors/scenarios.json`. Item 4 is three scenarios, so there are 20 names. A scenario that needs two lane configs (1, 15, 16) records one final hash per lane in `fields.state_hashes`; `hash` is the last one:

1. `deposit_create_account`: two deposits to new keys; a bounce when `max_accounts` is reached; a bounce in allowlist mode.
2. `limit_rest_and_cancel`: GTC rests, cancel by id, cancel-all.
3. `cross_and_fill_partial`: taker partially fills 3 makers at 2 price levels, and the rest rests.
4. `ioc_remainder_canceled`, `post_only_rejected_when_crossing`, `self_trade_prevention`.
5. `margin_reject_worst_case`: an order passes at mark but fails at the limit due to the adverse limit term.
6. `flip_position`: long 10 → sell 25 → short 15; check realized PnL and cost basis.
7. `funding_zero_sum`: 3 accounts, funding interval crossed twice; Σ payments = 0.
8. `liquidation_to_backstop`: price drop → liquidation → backstop takes the position, fee paid, deficit absorbed.
9. `backstop_deficit_flag`: a deficit sets `BACKSTOP_DEFICIT` and emits the event; trading continues; the operator unwind clears it.
10. `withdraw_then_checkpoint`: pending → commitment → leaves verify with `caravel-merkle`.
11. `forced_withdrawal_cancels_orders`.
12. `session_key_permissions`: trade ok, withdraw rejected, expired key rejected, revoke works.
13. `nonce_rules`: expired and bad nonces are not consumed; margin reject consumes the nonce.
14. `oracle_rules`: stale, future, circuit breaker, unknown key (fatal).
15. `fatal_cases`: bad encoding, inbox gap, invalid signature, block over `max_block_bytes`, two ORACLE entries for one market, unknown oracle key → `Fatal` with the right code and entry index.
16. `withdraw_liquidity_and_cash`:
    - withdrawing unrealized PnL is rejected (`amount ≤ collateral`);
    - after a backstop deficit, a withdrawal above lane liquidity is rejected with `INSUFFICIENT_LANE_LIQUIDITY`;
    - INV-P7 holds.
17. `account_slot_reuse`: fill `max_accounts`, empty one account, deposit from a new key, which reuses the slot at the same index with `next_nonce = timestamp_ms`; an old tx signed for the evicted key is rejected `BAD_NONCE`.
18. `oracle_breaker_widens`: after a 1-hour gap, a 30% move is accepted; a 30% move 1 s after the last update is rejected.

### 19.4 Property tests (`proptest`)

- Random sequences of up to 200 blocks with random deposits, orders, cancels, oracle moves (±3% per block), withdrawals and funding crossings.
- After every block, assert INV-P1…P4, INV-P7 and INV-P8.
- Every 20 blocks, compare native and Wasm state bytes (INV-P5).
- A fixed 400-block run (`long_run_crosses_funding_and_liquidates`) must reach fills, liquidations, non-zero funding and commitments, so the random test cannot pass vacuously.

### 19.5 End-to-end (T-011, T-012)

Local compose runs:
- quickstart with local RPC: `stellar container start local --limits testnet` (CLI 28.1.0; checked 2026-09-29: protocol 28, 1 s ledgers, testnet limits; DEC-043);
- a local USDC SAC;
- the sequencer, 3 validators and the relayer.

The e2e script:
1. Deposit 1,000 USDC to accounts A and B.
2. A rests a bid and B market-sells into it. Check the fill via API.
3. Wait for checkpoint seq ≥ 1 accepted on-chain.
4. A withdraws 100 and claims on Stellar. Check the USDC balance delta.
5. Stop the sequencer, wait past `escape_timeout_secs` (30 s on the local instance, DEC-043), `freeze`, and have A and B `escape_claim`. Check Σ payouts ≤ vault balance and each payout = equity × ratio.
6. Run `caravel-perps-node replay` from Stellar data. It MUST report OK.

### 19.6 Measurements to publish (T-014)

- Blocks/s and user tx/s sustained.
- Host `cpu_insns` per block at p50/p99.
- Checkpoint tx size, `minResourceFee` and fee charged, per checkpoint and per 1,000 lane transactions.
- End-to-end latency: order POST → fill in WS (soft), and fill → checkpoint accepted (hard).

Label each number with its date and conditions (SoroDOOM style).

---

## 20. Build plan

Status values: `todo`, `doing`, `review`, `done`. Agents update the Status cell in the same PR as the work.

### 20.1 Milestone M0: testnet demo

| ID | Task | Depends | Reads | Status |
|---|---|---|---|---|
| T-000 | Bootstrap repo, workspace, toolchain, `versions.json`, CI, CLAUDE.md, SOURCES.md | — | §0, §6, §7, §19.1 | done |
| T-001 | `caravel-types`: all §9 codecs, tags, reason and fatal codes, fixed-point helpers; golden vector generator | T-000 | §8, §9, §10.2, §11 (codes), §11.10 | done |
| T-002 | `caravel-merkle`: build + verify + proof generation; native and Soroban hashers | T-000 | §9.9 | done |
| T-003 | `caravel-perps`: genesis + step (§11 complete) + scenarios 1–18 + property tests (native) | T-001, T-002 | §8, §9.10, §10, §11 | done |
| T-004 | `lanes/perps/engine/contracts/perps-engine`: wrapper, size budget, reproducible build, hash in versions.json | T-003 | §12 | done |
| T-005 | `caravel-lane::executor`: soroban-env-host runner; parity gate (10k blocks + scenarios); cpu/mem benchmark at full caps | T-004 | §8.2, §12.2, §14.5 | done |
| T-006 | `platform/contracts/settlement`: all of §13 + tests in §13.7 | T-001, T-002 | §9, §11.8, §13 | done |
| T-007 | `caravel-node sequencer`: mempool, block loop, SQLite store, API/WS, checkpoint policy + assembly | T-005 | §14 | done |
| T-008 | `caravel-node validator`: follow, re-execute, sign, never-equivocate store | T-005 | §15 | done |
| T-009 | `platform/relayer`: inbox watcher, checkpoint submitter, oracle feeder | T-006, T-007 | §17 | done |
| T-010 | `caravel-node replay` | T-005, T-006 | §16 | done |
| T-011 | Local compose + `scripts/e2e-local.sh` (§19.5 steps 1–6) | T-007…T-010 | §19.5 | done |
| T-012 | Testnet deploy script; deploy engine + settlement; one **witness** `step` transaction on testnet with a small state, byte-equal to the executor output | T-011 | §3, §12, §13 | review |
| T-013 | `lanes/perps/web` (§18) against local, then testnet | T-007, T-009 | §18 | review |
| T-014 | Measurements (§19.6) + `docs/RESULTS.md` with dated numbers | T-012 | §19.6 | review |
| T-015 | `docs/RUNBOOK.md`: run locally, run a validator, deploy, rotate keys, freeze drill, replay | T-012 | all | review |
| T-016 | Security pass: walk §24 checklist, fix or file each item | T-012 | §24 | review |

Acceptance criteria:

- **T-000:**
  - `cargo test` and the npm tests run green on an empty skeleton;
  - CI runs §19.1;
  - `versions.json` matches §7.
- **T-001:**
  - round-trip tests for every type;
  - strict decoders reject trailing bytes, bad enums and wrong lengths;
  - vectors committed;
  - no `std` in the crate.
- **T-002:**
  - vectors for n = 0, 1, 2, 3, 5, 8, 1024;
  - proofs verify, and tampered siblings, wrong index or wrong length fail;
  - the Soroban hasher test passes in a contract test.
- **T-003:**
  - scenarios 1–18 pass;
  - proptest 256 cases × 200 blocks pass INV-P1…P4, P7, P8;
  - `genesis` output hash printed and recorded.
- **T-004:**
  - Wasm ≤ 120,000 bytes;
  - `sha256` recorded;
  - `stellar contract build` reproduces the same hash on a clean checkout.
- **T-005:**
  - INV-P5 holds on 10,000 random blocks and all scenarios;
  - a `LedgerInfo`-independence test passes;
  - benchmark table committed (full caps: cpu ≤ 100M per block, or caps reduced with a DEC).
- **T-006:**
  - every check in §13.3 has a failing-case test;
  - happy paths pass;
  - budget measured for a 96,000-byte batch.
- **T-007:**
  - 1s blocks for 1 hour locally with a synthetic load of 50 tx/s;
  - restart recovers from SQLite with identical state hash;
  - checkpoint every 10 blocks.
- **T-008:**
  - detects a tampered block (test injects a wrong `state_hash_after`) and refuses to sign;
  - never signs two headers for one seq (test).
- **T-009:**
  - idempotent under restart at every step;
  - reconciles already-accepted checkpoints.
- **T-010:** replays the local e2e run from Stellar data only.
- **T-011:** the script passes from a clean clone with one command.
- **T-012:** contract IDs in `versions.json`; witness tx hash in `docs/RESULTS.md`.
- **T-013:** all §18.3 flows work against testnet with Freighter.
- **T-014:** numbers dated and reproducible by a script.
- **T-015:** a new person can run a validator from the doc alone.
- **T-016:** checklist items are all resolved or ticketed.

### 20.2 Suggested order for a 4-week sprint (adjust to the team)

- **Week 1:** T-000, T-001, T-002, T-003 (core logic first; everything else depends on it).
- **Week 2:** T-004, T-005, T-006.
- **Week 3:** T-007, T-008, T-009, T-010, T-011.
- **Week 4:** T-012, T-013, T-014, T-015, T-016.

If time is short, cut in this order:
1. forced withdrawal UI;
2. session keys (sign every order with Freighter);
3. the XLM market;
4. the escape UI (keep the contract function and the e2e test).

Never cut replay, validator re-execution or the escape contract path: they are the claims.

### 20.3 Milestone M0.5: the platform split, then declarative lanes

**Decided with the human on 2026-09-30.** Caravel is the platform that lets any team launch a lane settling to Stellar, in the token the lane chooses (DEC-072). Caravel Perps is the first lane built with it, not the product. M0 built the two as one. M0.5 splits them in one repository:
- `platform/` holds the app-agnostic runtime, nodes, settlement, relayer core and the deploy tool;
- `lanes/perps/` holds Caravel Perps;
- `lanes/payments/` holds a small second template.

**Changed with the human later on 2026-09-30.** Caravel becomes an infrastructure-as-code tool for Stellar lanes:
- one lane file declares the lane, plus one `[env.<name>]` table per deployment;
- `caravel plan` shows what would change on Stellar and on the host, `caravel apply` makes it so, and `caravel destroy` winds the lane down safely: drain, export every exit proof, freeze;
- the lane file and the chain are the only record, and there is no state file;
- hosts are `local` or `ssh` (a host the team already has); the tool provisions no cloud machines;
- lane #1 (the live testnet lane) is imported, and its engine Wasm stays byte for byte.

This drops the lane registry contract, the console and its web packages, hosted trials with their host manager, and the Pyth Pro in-engine feed (parked). The plan of record is `~/.claude/plans/ok-but-now-i-lucky-hoare.md`. Phase gates are P1 (P-07), P2 (P-10), P3 (P-14) and P4 (P-18).

**Needs the human first (§0.4):**
- the new consensus formats of Phase 2 (`AppGenesisV1`, the SDK state layout, the Payments rules), approved 2026-09-30;
- the new settlement build of record for new lanes, approved 2026-09-30;
- each release to the live VM;
- the §2 claims and the landing copy.

| ID | Task | Depends | Status |
|---|---|---|---|
| P-01 | Baseline: tag `perps-m0`; golden sequencer trace, fixture store, API and genesis snapshots | T-016 | review |
| P-02 | Frozen perps engine workspace `lanes/perps/engine/` (byte-identical Wasm, tree-hash check) | P-01 | review |
| P-03 | Moves into `platform/` and `lanes/perps/` (runtime, node, settlement, configs, web, relayer) | P-02 | review |
| P-04 | `caravel-core`: generic codecs over the same bytes (opaque bodies, feeds, receipts, state frame) + compatibility tests | P-03 | review |
| P-05 | Runtime on `LaneApp` + `PerpsApp`: golden trace byte for byte, parity gate, fixture store opens | P-04 | review |
| P-06 | Node on `NodeApp`, lane-file split, `caravel-perps-node`, relayer feed module: API snapshots identical, dependency guard | P-05 | review |
| P-07 | Live VM upgrade to `caravel-perps-node` (check-store, shadow validator, replay) — **Gate P1** | P-06 | review |
| P-07a | Perps oracle: Coinbase's public WebSocket ticker as the first source, once per 1 s block (DEC-058) | P-07 | review |
| P-07b | Stellar Wallets Kit replaces Freighter in the perps web app, with a local SEP-53 check (DEC-059) | P-07a | review |
| P-08 | `caravel-app-sdk` + `testapp`, conformance with perps' standard kinds | P-07 | review |
| P-08b | Pyth Pro feed verified in-engine in the SDK — **parked** with the concept change | P-08 | todo |
| P-09 | `caravel-harness`; settlement tests move off perps; new settlement build of record | P-08 | review |
| P-10 | Payments template: engine, vectors, scenarios, INV-PAY1, parity, node, e2e (DEC-064) — **Gate P2** | P-09 | review |
| P-11 | Node groundwork, one release for the VM: `[env]` tables set aside by the lane-file parser, identity fields in `/v1/status`, `export-proofs`, release with `COMMIT`, full `SHA256SUMS` and vendored relayer dependencies (DEC-065) | P-10 | review |
| P-12 | `caravel-deploy`: the `[env]` schema, secret refusal, derived settlement address, and the pure plan engine with golden plans (DEC-066) | P-11 | review |
| P-13 | `apply` on Stellar and the `local` provider: chain reader, generated node configs, the `caravel` dispatcher (DEC-067) | P-12 | review |
| P-14 | `status` and `destroy`; `e2e-local.sh` driven by the tool for both templates (DEC-068) — **Gate P3** | P-13 | review |
| P-15 | The `ssh` provider: prerequisites check, IAP transport, systemd, Caddy, template extras (DEC-069) | P-14 | review |
| P-16 | Lane #1 under the tool: its `[env.testnet]` in its lane file, a plan with no Stellar changes, the host configs normalized by `apply` (DEC-070) | P-15 | review |
| P-17 | A payments lane on testnet from its lane file, through the whole lifecycle; RESULTS (DEC-071) | P-16 | review |
| P-18 | README, §2.4 claims (proposed), security pass over the tool; the landing copy after the human approves §2.4 — **Gate P4** | P-17 | review |
| P-19 | A configurable settlement token: Stellar assets, SEP-41 contracts, Circle's USDC; template decimals (DEC-072) | P-18 | review |
| P-20 | Landing page for declarative lanes, from the approved §2.4 (a preview first; deployed only after the human's OK) | P-19 | review |
| P-21 | Open source: README, CONTRIBUTING, licenses and an open-source landing page | P-20 | review |
| P-22 | A shorter landing page, from the approved design canvas | P-21 | review |

### 20.4 Phase 2 consensus formats (approved by the human 2026-09-30, DEC-060)

The human approved this section as proposed on 2026-09-30. These formats freeze like §9 once their vectors land: a change needs a version bump, regenerated vectors and a DEC. They reuse the M0 conventions: little-endian, no padding, an 8-byte magic, the §9.2 transaction envelope, the §9.4 inbox, the §9.10 `CommitmentV1`, the §11.10 receipts container and the §11.8 commitment rules. The perps engine stays on its own frozen formats (DEC-051).

#### 20.4.1 `AppGenesisV1` (genesis config for SDK apps)

```text
magic "CVAPPGN1" (8) · lane_id (32)
template [16] (ASCII, zero-padded, e.g. "payments") · template_version u16
system_key_count u8 (1..=4) · system_keys[] (32 each)   # accounts 0..n-1, flag SYSTEM; key 0 is the treasury (fees)
access_mode u8 (0 OPEN, 1 ALLOWLIST) · allowlist_count u16 · allowlist[] (32 each, sorted)
min_deposit i128 · min_withdrawal i128
max_accounts u32 · max_session_keys u8 · max_txs_per_account_per_block u16
max_entries_per_block u32 · max_block_bytes u32 · max_pending_withdrawals u32
exec_cpu_limit u64 · exec_mem_limit u64
app_params_len u32 · app_params[app_params_len]          # the app's own; the app decodes it strictly
```

- `config_hash = H(AppGenesisV1 bytes)`, as for `GenesisConfigV1`. The lane file's generic sections (DEC-054) map one to one onto the generic fields, and the app section onto `app_params`.
- `genesis()` returns `Fatal(BAD_CONFIG)` unless:
  - `template` is the engine's own and `template_version` is one it runs;
  - the system keys are distinct and none is in the allowlist;
  - the allowlist is strictly ascending;
  - `min_deposit ≥ 1`, `min_withdrawal ≥ 1`, and every `max_*` is > 0;
  - `max_accounts ≥ system_key_count + 1`;
  - the block and exec limits are within §10.2's bounds;
  - `app_params` decodes, to exactly `app_params_len` bytes.
- There are no feeds in V1. An app with feeds puts its feed configuration in `app_params`; P-08b proposes the Pyth Pro one.

#### 20.4.2 SDK state layout

```text
StateFrameV1 prefix (209, DEC-052)          # magic = the app's, e.g. "CVSTPAY1"; app_word and app_flags are the app's
config AppGenesisV1 (embedded)
account_count u32 · accounts[] AppAccountV1  # index = position; 0..system_key_count-1 are system accounts
app_globals_len u32 · app_globals[]          # the app's
pending_count u32 · pending[] (key 32 · amount i128)
last_commitment CommitmentV1 (160)
```

`AppAccountV1`:

```text
key (32) · flags u8 (bit0 SYSTEM) · next_nonce u64 · balance i128
session_key_count u8 · session_keys[] (key 32 · expires_at_ms u64 · permissions u8)   # sorted by key
txs_this_block u16
ext_len u16 · ext[]                          # the app's per-account data
```

#### 20.4.3 The SDK's standard pipeline

`step` runs the perps order (§11.2), with the app's hooks in place of funding, oracle and liquidations:
1. Decode and block checks.
2. Reset `txs_this_block`.
3. App `begin_block`.
4. INBOX entries.
5. FEED entries: fatal for an app without feeds.
6. USER entries.
7. App `end_block`.
8. The commitment on `CHECKPOINT_END`.
9. Update the frame.

- **INBOX (§11.4).** A deposit credits `balance`, and creates or reuses an empty slot like perps: new accounts start at `next_nonce = block.timestamp_ms`. Otherwise it bounces. A forced withdrawal pushes `min(amount, balance, app free balance, liquidity_left)`.
- **USER (§11.3), steps 1 to 9 unchanged.** Then:
  - WITHDRAW (kind 4): §11.3.2, with `balance` and the app's free balance in place of collateral and margin, and no oracle check;
  - session keys (kinds 5 and 6): §11.3.3, except that the allowed permission bits are the app's mask instead of `0x03`;
  - any other kind: the app.
- **Codes.** The platform's are M0's codes 0 to 6 and 40 to 51 (`INSUFFICIENT_FREE_COLLATERAL` reads as insufficient free balance). Apps use 10 to 39 and 60 and up. Fatal codes 1 to 31 are the platform's.
- **Receipts.** `CVRCPT01`. The platform events are 6 DEPOSIT, 7 FORCED_WITHDRAWAL_PROCESSED and 10 COMMITMENT; the app's events use types 16 and up.
- **Commitment (§11.8).** `escape_equity_j` is the app's escape equity for account `j`.
- **Invariants for every SDK app.** SDK-INV1, conservation: `Σ app-held value + Σ pending.amount == deposits_credited_total − withdrawals_committed_total`. INV-P4, INV-P5, INV-P7 and INV-P8 hold unchanged. Each app states what its held value is.

#### 20.4.4 Payments (template `payments` 0.1.0, state magic `CVSTPAY1`)

- **`app_params`** (32 bytes): `transfer_fee i128` (≥ 0, flat per transfer, paid to the treasury; 0 means free) · `min_transfer i128` (≥ 1).
- **Kind 16, TRANSFER.**
  - Body: `to 32 · amount i128 · memo u64` (56).
  - Signer: the owner, or a session key with `PERM_TRANSFER = 0x01` (the app's only permission bit).
  - Checks, in order, each a rejection that still consumes the nonce:
    1. `amount ≥ min_transfer` → 12 `BELOW_MIN_TRANSFER`;
    2. `to != account` → 13 `SELF_TRANSFER`;
    3. `to` is an existing account → 11 `UNKNOWN_RECIPIENT` (a recipient must have deposited once, so transfers can't fill account slots);
    4. `balance ≥ amount + transfer_fee` → 10 `INSUFFICIENT_BALANCE` (a sum that overflows i128 is this rejection too, not a fatal).
  - Effect: `balance −= amount + fee`; `to.balance += amount`; `treasury.balance += fee`.
  - Event 16 TRANSFER: `from_idx u32 · to_idx u32 · amount i128 · fee i128 · memo u64`.
- **Free balance and escape equity** are both the balance. There is no `ext`, no `app_globals`, and `app_word` and `app_flags` are 0.
- **Feeds:** none. A FEED entry is fatal.
- **INV-PAY1:** `Σ balances + Σ pending.amount == deposits_credited_total − withdrawals_committed_total`.
- **Engine and version:** the `payments-engine` contract, `version()` = `H("CARAVEL/ENGINE/V1" ‖ "payments/0.1.0")`, a 64 KB Wasm budget, and its hash in `versions.json` `lanes.payments`.
- **Lane file** (DEC-054): `[app] template = "payments"`, and `[payments] treasury_key` (G..., system account 0), `transfer_fee`, `min_transfer`. The node is `caravel-payments-node` (DEC-064).

### 20.5 Milestone M0.6: a real CLI and the lane file language

**Decided with the human on 2026-10-02.** The deploy tool works, but it is a fixed-shape deployer:
- one settlement contract, one sequencer, N validators and one relayer, all on one host;
- no variables, references, env inheritance, outputs or extra resources;
- a quickstart that is a set of build commands and a shell loop.

M0.6 makes Caravel a real infrastructure-as-code CLI. The human chose:
- **Language:** keep TOML, and add an expression layer: vars, `${…}` references, env `extends`, `for_each`, outputs and local modules.
- **Priorities:** composition first, then chain resources, then topology. Host-provider plugins come later.
- **CLI scope:** a single `caravel` CLI for operators and for users' flows (fund, deposit, tx, withdraw, claim, escape) with built-in waits.
- **Install:** from source for now (`scripts/install.sh`). There are no public releases yet.

**Rules that hold for every task:**
- Lane #1 keeps `plan` = "No changes." and its genesis hashes.
- Genesis sections stay literal, because they are consensus config.
- There is still no state file.
- The frozen engine is untouched.
- `platform/` never depends on `lanes/`.

The plan of record is `~/.claude/plans/understand-this-project-and-zesty-zephyr.md`. Phase gates are G1 (C-05), G2 (C-10), G3 (C-14), G4 (C-17), G5 (C-21) and G6 (C-25). **Gate G1 passed on 2026-10-02**, with #49–#54 merged.

**Needs the human first (§0.4):**
- signing users' lane transactions through the Stellar CLI keystore (SEP-53);
- how a declared contract's constructor arguments are checked after deploy;
- the trust boundary between hosts (`/v1/sign`, `/internal/*`);
- README and landing copy;
- each release to the live VM.

| ID | Task | Depends | Status |
|---|---|---|---|
| C-00 | Baseline pins: every file rendered for lane #1, its Stellar-side values, every lane file's genesis hashes; docs fixes | P-22 | done |
| C-01 | Plugin protocol in the template binaries: `plugin info`, `plugin example`, `plugin body`; `init` scaffolds (DEC-073) | C-00 | done |
| C-02 | `caravel-deploy` without `NodeApp`: a `Template` trait (in-process or plugin), pure `addresses()`/`desired()`, a state root (DEC-074) | C-01 | done |
| C-03 | `caravel-cli`, the one CLI: lane-file and env discovery, `--json`, exit codes; plan, apply, status, destroy, validate, env, output, version, doctor (DEC-075) | C-02 | done |
| C-04 | Install from source: `install.sh`, the release next to the binary, a web dir per template, a binary-platform guard (DEC-076) | C-03 | done |
| C-05 | `init` and `keys`; local applies create the identities they name (DEC-077) — **Gate G1** | C-04 | done |
| C-06 | `caravel-lanefile`: loader with spans and diagnostics, `include`, `extends`, reserved keys; genesis refuses `${` (DEC-078) | C-05 | review |
| C-07 | The expression evaluator: grammar, types, functions, no time or randomness (DEC-079) | C-06 | review |
| C-08 | Vars, locals, `for_each`, per-env `[env.<name>.node]` | C-07 | todo |
| C-09 | The manifest on resolved values; `caravel render`; the e2e without heredoc or `sed` | C-08 | todo |
| C-10 | Attributes and outputs; `caravel output`; references in relayer feeds — **Gate G2** | C-09 | todo |
| C-11 | Lifecycle: `stop`, `start`, `restart`, `logs`, `replay` from the lane file, `wait`, `api` | C-10 | todo |
| C-12 | Users' Stellar flows: `account create/fund`, `balance`, `deposit` (waits for the credit) | C-11 | todo |
| C-13 | Lane transactions: `tx`, `withdraw`, `claim`, `force-withdraw`, `escape` | C-12 | todo |
| C-14 | The e2e on the CLI only; README, landing quickstart, RUNBOOK, `docs/LANE_FILE.md` — **Gate G3** | C-13 | todo |
| C-15 | The resource graph, with identical plans (goldens byte for byte) | C-14 | todo |
| C-16 | Addresses in plans; `plan --json`; `graph`; `depends_on`, `--target`, `--replace` | C-15 | todo |
| C-17 | Saved plans: `plan --out`, `apply <planfile>` refused when anything moved — **Gate G4** | C-16 | todo |
| C-18 | Accounts: funding, trustlines, balances topped up | C-17 | todo |
| C-19 | Tokens: issued assets and their contracts; a declared token as the settlement token | C-18 | todo |
| C-20 | Contracts: any Wasm, constructor arguments, derived addresses, `prevent_destroy` | C-19 | todo |
| C-21 | Local modules with inputs and outputs — **Gate G5** | C-20 | todo |
| C-22 | Several hosts per deployment and node placement | C-21 | todo |
| C-23 | Networking across hosts (private addresses) | C-22 | todo |
| C-24 | Lane namespaces: several lanes on one host | C-23 | todo |
| C-25 | The web app as a resource, configured from outputs — **Gate G6** | C-24 | todo |

---

## 21. Later milestones (not for M0 agents to start without a human go-ahead)

### M1: Lane framework

- **T-M1-01:** Lane config modules, as a `lane.toml` schema that separates engine modules from node settings. Add a per-lane gas mode (`none` / USDC fee per tx / lane token) for generic lanes.
- **T-M1-02:** Generic Soroban lanes. Replace the stateless `step` with ledger-entry execution:
  - run `soroban_simulation`/`e2e_invoke` over a key-value store;
  - commit state as a sparse Merkle tree over ledger entries;
  - reuse the Soroflare snapshot idea with current crates.
- **T-M1-03:** Browser replay: build `caravel-perps` for `wasm32-unknown-unknown` with a JS `Crypto` shim, parity-tested against the host path (SoroDOOM's browser/host parity).
- **T-M1-04:** Validator bonding in USDC in the settlement contract, plus `slash_equivocation(header_a, sig_a, header_b, sig_b, signer_index)`. Two valid signatures by one key on different headers with the same `(lane_id, seq)` slash that key's bond.
- **T-M1-05:** Fee share to validators: a `validator_share_bps` of the treasury, claimable per epoch.
- **T-M1-06:** Settlement factory: one contract deploys settlement instances per lane (uses CAP-85 externally managed executables where it helps fleet upgrades `[VERIFY]`).
- **T-M1-07:** Challenge window before withdrawals become claimable. This is an optimistic mode: validators or watchers can post a conflicting signed header or a replay mismatch to halt claims.
- **T-M1-08:** Forced close through the inbox (kind 2): it closes all of an account's positions against the backstop at mark, with `liq_fee_bps`. This closes the gap in §2.2 where a sequencer censors closing orders.
- **T-M1-09:** Account admission for open lanes: a creation fee, or a minimum deposit per new account, to stop slot squatting (OQ-008).

### M2: Trust minimization and mainnet readiness

- **T-M2-01:** Validity proofs:
  - a RISC Zero guest runs `caravel-perps::step` over a batch;
  - it proves `(prev_state_hash, batch_hash) → (state_hash, commitments)`;
  - a Groth16 wrap is verified on Soroban with `NethermindEth/stellar-risc0-verifier`;
  - pattern: `kalepail/kalien`.
  - Benchmark proving time per checkpoint first.
- **T-M2-02:** BLS12-381 aggregate signatures for validator sets > 20, with proof-of-possession at registration (`soroban-examples/bls_signature` as a starting point).
- **T-M2-03:** Reward token: vesting, slashable USDC-backed shares for validators, per the tokenomics notes (fee split, insurance fund, forced inclusion). Needs a separate economic spec.
- **T-M2-04:** Auto-deleveraging when the backstop is insolvent; better liquidation (partial, auction).
- **T-M2-05:** External DA option for batches above 96 KB, plus compression. Keep the on-chain hash.
- **T-M2-06:** Remove the testnet admin powers (or put them behind an OpenZeppelin `stellar-governance` timelock). Audit. Mainnet.

### M3: stellar-core-based lanes (optional)

For lanes that need classic Stellar operations or SCP among many validators:
- fork stellar-core with a lane-specific `ledgerTargetCloseTimeMilliseconds` bound;
- adopt CAP-88 when it has a protocol version;
- checkpoint the same way.

---

## 22. Decisions log

| ID | Decision | Why | Revisit when |
|---|---|---|---|
| DEC-001 | M0 lanes are a sequencer + re-executing validators, not a stellar-core fork | CAP-70 limits close time to [4000, 5000] ms; whole-second close times; CAP-88 not scheduled; fastest path to a working demo | M3 |
| DEC-002 | Execute the exact engine Wasm through `soroban-env-host` for consensus | Strongest honest "Soroban lane" claim; SoroDOOM D005 precedent; enables witness tx | never for M0 |
| DEC-003 | Stateless `step(state, block)` with bounded canonical state | Proven by SoroDOOM; simplest deterministic design; state is the replay object | State > ~1 MB or generic contracts (M1-02) |
| DEC-004 | Weighted ed25519 multisig instead of BLS | Native host fn, Stellar keys, Axelar precedent, cheap for ≤ 20 signers | > 20 validators (M2-02) |
| DEC-005 | Calldata DA: full batch as a `submit_checkpoint` argument, only its hash stored | Anyone can rebuild from Stellar transactions; no extra storage rent; SoroDOOM's "replay from Stellar" property | Batches > 96 KB (M2-05) |
| DEC-006 | Inbox with a hash accumulator for Stellar → lane messages | Sequencer cannot fake or reorder deposits; validators need no Stellar access to check them; enables force-inclusion timeouts | — |
| DEC-007 | Withdrawals via a per-checkpoint Merkle root; escape via an account-equity root, paid pro-rata | Standard, cheap to verify (SHA-256), safe under insolvency | M1-07 challenge window |
| DEC-008 | Custom fixed-width LE encodings, no serde in consensus | Same bytes in Rust no_std, Soroban and TS; SoroDOOM style | — |
| DEC-009 | Perps accounting: signed lots + cost basis; integer funding per lot; backstop account liquidations | Exact conservation (INV-P1/P2) without rounding dust | M2-04 |
| DEC-010 | Nodes in Rust (SQLite), relayer and web in TypeScript; no Cloudflare dependency | Validators must be independent processes; stellar-sdk is the best-documented tx path; SoroDOOM's Durable Object pattern is optional hosting | Hosting review |
| DEC-011 | USDC only, 7 decimals; no gas on the lane; per-account per-block tx cap | Matches the pitch ("fees in USDC", zero gas on trades); deterministic spam limit | M1-01 |
| DEC-012 | Testnet only; admin upgrade and rotation allowed but flagged | Speed of iteration | M2-06 |
| DEC-013 | Withdrawals only from realized collateral, capped by lane liquidity (INV-P7) | Keeps every signed checkpoint acceptable on Stellar; avoids permanent halts after bad debt | M2-04 (ADL) |
| DEC-014 | No lane-wide reduce-only mode; `BACKSTOP_DEFICIT` is informational and the operator unwinds | A reduce-only mode could deadlock (no resting liquidity) | M2-04 |
| DEC-015 | Host CPU/memory budget per `step` is a consensus value in `GenesisConfigV1` | Budget exhaustion is fatal, so all nodes need identical limits | — |
| DEC-016 | Empty account slots are reused in place; new accounts start at `next_nonce = block.timestamp_ms` | Bounded state without index shifts; blocks replay of old signatures | M1-02 (unbounded state) |
| DEC-017 | Validators may skip seqs but never sign two headers for one seq; they compute every header themselves | Liveness after restarts without weakening equivocation safety | — |
| DEC-018 | Pin `stellar-xdr =28.0.0` and `stellar-strkey =0.0.16` instead of 28.0.1 and 0.0.18; `ed25519-dalek =2.2.0`, `sha2 =0.10.9`. Allow exactly one upstream duplicate, `stellar-strkey` 0.0.13 (from `soroban-env-host` and `stellar-xdr`) | `soroban-env-common 28.0.2` and `soroban-sdk 28.0.0` require `stellar-xdr =28.0.0`, so 28.0.1 cannot resolve. `soroban-sdk` pins strkey 0.0.16 while the host's `^0.0.13` means exactly 0.0.13. One dalek and sha2 version keeps native and host verification identical (§8.4) | soroban-sdk/env-host 29 |
| DEC-019 | The landing page lives in `site/`, with its own `vercel.json` and Vercel link, and is deployed only from there | Deploying from the repo root would upload the whole Rust/TS repo to Vercel | Web app hosting (T-013) |
| DEC-020 | The Wasm of record is the output of `scripts/build-contracts.sh`, i.e. `stellar contract build --locked` with the CLI version pinned in `versions.json` (`stellar_cli`). Its sha256 is the one recorded and uploaded | The CLI optimizes by default and embeds its version in contract metadata, so plain `cargo build` gives a different hash. One build path keeps INV-D7 checkable | CLI major upgrade |
| DEC-021 | Golden vectors come from `cargo gen-vectors` (the `gen-vectors` feature of `caravel-types`), and each vector file holds a list of vectors plus invalid cases | A binary cannot use dev-dependencies, and the `no_std` library must not depend on a hasher or signer. A feature keeps `sha2` and `ed25519-dalek` out of the library build. A staleness test makes a silent format change impossible | — |
| DEC-022 | `caravel-types` checks encoding only and never hashes. Block decode errors are split into header errors (the engine reports `BAD_BLOCK_ENCODING`) and entry errors with the entry index (`BAD_ENTRY_ENCODING`). Entry order, one-oracle-per-market and the size and count caps are left to the engine, which reports them with their own codes (§11.2). Receipt decoding requires `status` to match `code` and rejects unknown codes | Keeps the fatal codes in §11 distinct and attributable. Preimage builders (tx hash, SEP-53, inbox acc, leaves, signers, rotation) let the engine, nodes and the contract share exact bytes with their own SHA-256 | — |
| DEC-023 | `Crypto` gains a defaulted `check_ed25519(..) -> Result<(), InvalidSignature>` that the engine calls. The default traps through `ed25519_verify`, as the host does; `DiagnosticCrypto` overrides it so a bad signature becomes `Fatal { BAD_SIGNATURE, entry_index }` | §14.1 needs the entry index of a bad signature; the trait in §11 had no fallible path | — |
| DEC-024 | **Accepted at Gate 1 (2026-09-29).** Where the spec is silent (each choice changes state or receipt bytes, so scenario hashes depend on it): (a) an IOC remainder never gets an order id, so its `ORDER_CANCELED` has `order_id 0`; (b) `CANCEL_ALL` with a market id that is neither `0xFFFF` nor configured is rejected `UNKNOWN_MARKET` (nonce consumed); (c) `FORCED_WITHDRAWAL` for a missing account emits no event, otherwise the event carries the amount actually queued (0 if none); (d) `LIQUIDATION.deficit` is the non-negative amount the backstop absorbed; (e) arithmetic overflow in `PLACE_ORDER` checks 8–9 or `WITHDRAW` check 5 rejects with that check's code, while overflow in a state transition is fatal `ARITHMETIC_OVERFLOW`; (f) duplicate account keys in a decoded state are `BAD_STATE_ENCODING`; (g) more than 2^20 accounts or pending withdrawals at a commitment is `ARITHMETIC_OVERFLOW` (§10.2 does not cap `max_accounts` at the Merkle depth) | Deterministic answers where §11 does not say; none touches a frozen format, an invariant, a settlement check or a claim | — |
| DEC-025 | New test-only crate `lanes/perps/engine/crates/caravel-testkit`: an `Executor` trait (native now, Wasm from T-005), a `Lane` simulator that signs real blocks and checks INV-P1…P4, P7, P8 after each one, the §19.3 scenarios, and the `gen-vectors` binary | Scenarios and vectors need the engine, which depends on `caravel-types`, so the generator cannot live there; one harness runs every scenario on both execution paths | — |
| DEC-026 | The engine contract uses `soroban-sdk`'s `alloc` feature (bump allocator) and copies `Bytes` in and out with `to_alloc_vec` / `from_slice`. `scripts/build-contracts.sh` fails when a built hash differs from the one in `versions.json` | `caravel-perps` is `no_std + alloc` by design (§4.2); the host charges linear memory to the per-call budget, which T-005 measures. The hash check makes INV-D7 continuous: an engine change must update the recorded hash in the same PR | T-005 benchmark shows memory pressure |
| DEC-027 | **Accepted at Gate 2 (2026-09-29).** Contracts build with `opt-level = 2` instead of `"z"` | The engine runs interpreted inside `soroban-env-host`. `"z"` avoids inlining, and at full caps that made decoding 4.6× and margin scans 2.7× more expensive (T-005 profile). `2` matches `3` on cost and is smaller: 84,764 bytes, within the 120,000 budget. The T-004 `"z"` build also hashed differently on Linux and macOS; that is host-dependent symbol order, not the opt level (DEC-033) | Wasm size approaches 120 KB |
| DEC-028 | **Accepted at Gate 2 (2026-09-29).** Testnet lane caps: `max_accounts 256`, `max_orders_per_side 128`, `max_block_bytes 12,000`, `exec_cpu_limit 200,000,000` (from 1,024 / 256 / 24,000 / 400M). Genesis hashes updated | §12.2: at 1,024 / 256 / 24,000 the worst block measured 270M even after optimization. 256 / 128 / 12,000 is the largest tested set with every block shape ≤ 100M (worst 95.8M). The CPU limit leaves 2× headroom over the worst block. About 56 orders per block still covers the 50 tx/s load test | Engine gets cheaper, or M1 state layout (T-M1-02) |
| DEC-029 | The executor enables `soroban-env-host`'s `testutils` feature and runs every call in a fresh `Host::test_host_with_recording_footprint()` with the fixed lane `LedgerInfo` (`network_id = H("CARAVEL/LANE-EXEC/V1")`) | `testutils` holds the test host, contract registration and `Budget::reset_limits`, and adds only `arbitrary` and recording mode. A fresh host per call keeps memory bounded and metering independent of history, and parity plus identical metering under two ledger infos are tested. The ungated `Budget::try_from_configs` + `invoke_function` path is left for M1 | M1 executor (T-M1-02) |
| DEC-030 | `refund_unprocessed_deposit` emits a `Refund` event, topics `("refund", index)`, data `from, amount` | §13.2 lists no refund event, but after a freeze indexers and the web app have to show which deposits went back, just as `Claimed` and `EscapeClaimed` show payouts | — |
| DEC-031 | Settlement edge rules: (a) `admin_rotate_signers` applies the same install as `rotate_signers`: a set used before is refused (`SignersReused`) and `LastRotationAt = now`; (b) `escape_claim` marks the account claimed and emits `EscapeClaimed` even when its share is 0 (`payout_den == 0`, or `payout_num ≤ 0` because the vault is fully owed to committed withdrawals) and transfers only a positive amount; (c) `MAX_BATCH_BYTES` comes from `caravel-types`, so the contract and the lane codec share one constant | (a) "same effect" in §13.2, and a reused set could replay old rotation signatures; (b) a zero share is a valid outcome and must not be claimable again; (c) one source for a consensus limit | — |
| DEC-032 | Settlement contract error codes: 1–2 constructor and signer sets, 10–13 deposits and forced withdrawals, 20–39 one code per `submit_checkpoint` check in §13.3 order, 40–41 rotation, 50–54 claims, 60–63 freeze, escape and refund (`platform/contracts/settlement/src/types.rs`, also in the contract spec). An invalid signature traps in `ed25519_verify` and has no code | §13.3 asks for a `contracterror` per rejection without numbering them; the relayer and the web app need stable codes to explain a failure | New checks get new codes; codes are never reused |
| DEC-033 | **Accepted at Gate 2 (2026-09-29).** The Wasm of record is built on an **x86_64 Linux** host: the CI `rust` job (toolchain from `rust-toolchain.toml`, the pinned CLI release binary with its GitHub digest). CI uploads the files as the `contracts-wasm` artifact, and deploys (T-012) use those files. `build-contracts.sh` fails on a hash mismatch on x86_64 Linux and only warns on other hosts | On other hosts the same source builds to different bytes of the same size. The settlement Wasm is `8a2fafbd…` on Linux and `8280828f…` on macOS; the T-004 engine at `"z"` was `95814212…` vs `04c731d0…`; the engine at `2` happens to match (`4571cd25…`). Cause: cargo 1.93 mixes every dependency's metadata hash into a crate's `-C metadata`, and proc-macro and build-script dependencies are host units whose hash includes the `host:` line of `rustc -vV` (`compute_metadata`, `hash_rustc_version` in cargo's `compilation_files.rs`). So every mangled symbol hash differs between hosts (checked on unstripped builds: same names in the same order, different hashes), and rustc can order some items by symbol name. The `type`, `func` and `code` sections differ; data, metadata and contract spec do not. Both settlement builds pass the same 73 tests with identical metering. INV-D7 holds with the host as part of the pinned toolchain | A Docker builder for local runs (with T-011), or cargo no longer hashing the host into target units |
| DEC-034 | Sequencer layout: `caravel-lane` holds the shared runtime: the SQLite store (`rusqlite 0.40`, bundled), the block builder, the mempool with pre-validation, checkpoint assembly and leaves, read-only views, and a synchronous `sequencer::Core`. `caravel-node` wraps it with `tokio 1.53`, `axum 0.8` (HTTP and WebSocket), `reqwest 0.13` (rustls) for validator calls and `tracing`. Node config is `config/sequencer.*.toml`: listen address, lane file, engine Wasm and hash, database, network passphrase (mainnet refused), settlement contract, `[signers]`, CORS origins, and the *name* of the environment variable that holds the internal bearer token. The `native` executor is refused with `production = true` | Validators and replay reuse the store, header assembly and views, so they agree byte for byte. A synchronous core is tested on a fake clock, including restart and quarantine | — |
| DEC-035 | Withdrawal leaves are rebuilt when a checkpoint is sealed, from its blocks and receipts in entry order: a bounced deposit, a `ForcedWithdrawalProcessed` with amount > 0, and a `WITHDRAW` with code OK. They are checked against the header's root, count and total before they are stored or served. `/v1/proofs/withdrawals` lists leaves of checkpoints accepted on Stellar; whether one is claimed is read from the contract (`is_claimed`) | The engine clears `pending` at the commitment, so no stored state holds the list, but the receipts do. The header check means a wrong rebuild can never be served. The sequencer does not watch claims | The engine exposes the leaves (M1) |
| DEC-036 | API details beyond §14.4: `/v1/status` also reports `state_hash`, `inbox.reported_acc` (the fold of every reported message, so tools can continue the chain), `mempool`, `backpressure`, `halted` and `host_metering` of the last `step` (labeled host metering, not fees). The stream also sends `subscribed`, `receipt` (the subscribed account's transactions and codes) and `lagged`. `/internal/oracle` takes `{update: hex}`; `/internal/inbox` answers 409 `INBOX_GAP` (send the expected index first) or `INBOX_MISMATCH` (inclusion halted). `/v1/tx` 400 codes: `DECODE`, `WRONG_LANE`, `BAD_SIGNATURE`, `EXPIRED`, `BAD_NONCE`, `UNKNOWN_ACCOUNT`, `DUPLICATE`, `MEMPOOL_FULL`, `ACCOUNT_QUEUE_FULL`; a transaction for an unknown account is refused rather than queued | The web app needs its receipts; the relayer needs to know what to resend; unknown-account transactions would only fill blocks with rejections | — |
| DEC-037 | Local lane `lanes/perps/config/lane.caravel-perps.local.toml`: the testnet parameters with the public fixture keys (seeds `0x21`, `0x22`, `0x31`) and the name `caravel-perps-local-0`, for development, `scripts/soak-sequencer.sh` and T-011. Never deployed. `caravel-node check-store` re-executes a node's store through the Wasm and rebuilds every checkpoint header; `platform/crates/caravel-node/examples/loadgen.rs` drives the soak | The load test and the local e2e need an oracle key whose secret is known; a separate lane name keeps it from being mistaken for the testnet lane | — |
| DEC-038 | Validator layout: `caravel-lane::validator::Follower` (synchronous: apply a record, compute headers, sign) inside `caravel-node validator` (axum). It follows by polling the sequencer's `/v1/blocks/{h}` every `poll_ms`; a block is "live" when `now − timestamp_ms ≤ 10 s`. Config is `config/validator-*.toml` with the `S...` secret in a separate key file; the executor is always the Wasm. It stores its own receipts, keeps every checkpoint snapshot (M0 does not prune, which covers "at least the last 3 plus the one accepted"), and keeps live-check flags in SQLite until `caravel-node validator-clear --through H`. A halt lasts until restart; a restart that fetches the same bad block halts again. `/v1/sign` refusals: `HALTED`, `HEADER_MISMATCH`, `BATCH_MISMATCH`, `SUSPICIOUS_BLOCK`, `ALREADY_SIGNED` (409), `NOT_CAUGHT_UP` (503), `BAD_HEADER` (400) | The same store, assembly and views as the sequencer, so headers match byte for byte; flags and signatures survive restarts because they are in the store | Snapshot pruning, if stores grow |
| DEC-039 | Validators learn which checkpoint Stellar accepted by reading the settlement contract's instance entry with RPC `getLedgerEntries`: `LastCkpt` is the key `Vec[Symbol("LastCkpt")]`, with `seq` (`U64`) and `header_hash` (32 `Bytes`) in its map. A settlement test pins that layout. A matching checkpoint (and every earlier one) is marked accepted | Independent of the sequencer, and needs no simulation or source account. The spec asks for acceptance "polled from `last_checkpoint()`"; this reads the same value from storage | Contract storage layout changes |
| DEC-040 | Relayer inbox and client choices: the inbox watcher's cursor is the sequencer's own count of reported messages (`/v1/status` `inbox.reported`), and it reads each message with the `inbox(index)` view by simulation, instead of paging `getEvents` from a cursor file. Contract calls use `@stellar/stellar-sdk` 17.2.0 `rpc.Server` directly with explicit `ScVal`s (`Sig` as a map with sorted symbol keys), not generated bindings. Secrets come from `CARAVEL_INTERNAL_TOKEN`, `CARAVEL_RELAYER_SECRET` and `CARAVEL_ORACLE_SECRET`; `config/relayer.*.json` holds the rest. The relayer accepts testnet and a local quickstart network ("Standalone Network ; February 2017") and refuses everything else | The sequencer's count survives relayer restarts and needs no local state, so resuming can neither skip nor repeat out of order. Views read the contract's own record, which does not age out of RPC retention the way events do. Six calls do not justify a generated client, and explicit encodings are tested | Many more contract calls (then generate bindings) |
| DEC-041 | Oracle sources per market, in priority order (spec §17.3): Reflector's testnet "External CEXs & DEXs" feed `CCYOZJCOPG34LLQQ7N24YXBM7LL62R7ONMZ3G6WZAAYPB5OYKOMJRN63` (`lastprice(Other(symbol))`, 14 decimals, 300 s resolution, checked on-chain 2026-09-29), then Coinbase's public spot price (`GET https://api.coinbase.com/v2/prices/{pair}/spot`), or a fixed price for local lanes. A source is used only if its own timestamp is within `maxSourceAgeSecs` (900 on testnet). Whatever the source, the relayer signs with the team's oracle key and a fresh `publish_time_ms`, publishing every 2 s on a one-tick move and at least every 10 s | Follows §17.3 and the OQ-002 default. Reflector moves only every 5 minutes, so the 10 s heartbeat keeps the lane within its 30 s staleness limit; the UI must say prices are signed by the Caravel team's oracle key | A lane oracle with its own feeds (M1) |
| DEC-042 | Replay reads everything from ledger entries and transactions: `Config`, `LastCkpt` (instance storage), `Ckpt(seq)` and `Claimed(seq, index)` (persistent entries, keys `Vec[Symbol(name), fields…]`) with `getLedgerEntries`; each checkpoint's transaction from its `ckpt` event, starting at the record's `stellar_ledger`; the `header` and `batch` from the `InvokeContract` arguments of the envelope. Every header is rebuilt with the same assembly code as the nodes and compared byte for byte, and the first differing field is named. `--genesis-config` takes the lane TOML. `--from-archive` is not built in M0: replay needs the transactions inside RPC's retention window (7 days on testnet, checked 2026-09-29), or a validator store checked with `check-store` | One byte comparison covers every commitment field. Storage reads need no simulation. A settlement test pins every key and field replay reads | Checkpoints older than RPC retention (M1: Galexie archive) |
| DEC-043 | The local end-to-end (`scripts/e2e-local.sh`) runs quickstart in Docker through `stellar container start local --limits testnet` (image `stellar/quickstart:latest`, protocol 28, 1 s ledgers, testnet resource limits), and the sequencer, 3 validators and the relayer as local release processes, not a compose file. It uses its own Stellar CLI config directory, a local USDC asset contract minted by a local issuer, the fixture oracle key of the local lane, and a settlement contract with `escape_timeout_secs = 30` and `force_inclusion_window_secs = 20`, because quickstart cannot move ledger time. `caravel-node tx` signs and submits lane transactions from an `S...` key file | One command from a clean clone, with nothing to build into images. Container images for hosting come with T-012. A short timeout stands in for "move ledger time" in §19.5 step 5 | Hosting images (T-012) |
| DEC-044 | **Decided at Gate 3 (2026-09-29).** The testnet lane checkpoints every 60 blocks (one a minute at 1 s blocks) instead of 10. `checkpoint_every_blocks` is a node setting, so the genesis hashes do not change; rules (b) and (c) of §14.2 still end a batch early when it fills | A small checkpoint cost 0.067 XLM on the local network with testnet limits (T-011). Every 10 blocks that would be about 8,600 transactions and 580 XLM a day; every 60 it is about 1,440 and 96 XLM. Withdrawals wait up to a minute longer to become claimable | Measured testnet fees (T-014) |
| DEC-045 | **Decided at Gate 3 (2026-09-29).** Hosting is the Google Cloud project `caravel-testnet` ("Caravel"), billed to the user's "My Billing Account 1" (BRL). A budget of R$100 a month for this project alerts at 50, 90 and 100%, and at 100% a Cloud Run function (`infra/gcp/billing-cap`, set up by `infra/gcp/setup-billing-cap.sh`) removes the project's billing account, which stops everything in it. The function's service account holds only Project Billing Manager and Browser on this project, not Billing Account Administrator as Google's guide suggests. Both paths were tested on 2026-09-29: below the budget it does nothing; above it billing was off within about 20 s, then relinked | The user asked for a spending limit; a budget alone only alerts. Google's caveats apply: notifications lag real costs, so the cap is not exact, and resources left without billing can be deleted | The budget amount, if the VM size changes |
| DEC-046 | Testnet hosting layout: VM `caravel-1` (e2-small, us-central1-a, Ubuntu 24.04, 20 GB standard disk, no service account, shielded boot) with static IP `35.224.76.64`, served as `35-224-76-64.sslip.io` with a free Let's Encrypt certificate from Caddy. SSH comes only through IAP (default SSH and RDP rules removed); ports 80 and 443 are open to the VM's tag. systemd runs the sequencer, `caravel-validator@1..3` and the relayer as the unprivileged `caravel` user with a read-only system (`ProtectSystem=strict`, writes only to `/opt/caravel/data`). Caddy exposes `/v1/*` (sequencer), `/validators/N/*` and the web app, never `/internal/*` or the validators' `/v1/sign`. Binaries and Wasm come from the CI `release` job (ubuntu-24.04, DEC-033) via `scripts/deploy-vm.sh`; secrets are copied once from the local Stellar keystore to `/opt/caravel/keys` (mode 600) | One cheap machine, as chosen at Gate 3; sslip.io gives TLS without buying a domain; building on the VM would be slow on 0.5 vCPU | A second machine or a domain |
| DEC-047 | Web app choices (T-013):
- a small pathname router instead of react-router, for five routes;
- the `buffer` package installed as `globalThis.Buffer` for the SDK and Freighter;
- the session key in IndexedDB, 24 h, `PERM_TRADE | PERM_CANCEL`;
- trades and cancels signed by that key when it exists, everything else through Freighter's SEP-53 `signMessage`;
- the price chart sampled in the browser from `/v1/markets` (the lane keeps no price history);
- buy/sell, bids/asks and PnL shown with words, signs and weight, not color, because `lane` and `harbor` are reserved for lane and Stellar things (§18.1);
- served from the same origin as the API on the VM (DEC-046), configurable with `VITE_*` for local lanes;
- `/v1/status` gains `config_hash`, which the browser needs to compute tx hashes | Fewer dependencies; the brand rule forbids a third accent | — |
| DEC-048 | How T-014 measures §19.6 (`scripts/measure-testnet.sh`, `scripts/measure-report.mjs`):
- on the live testnet lane, through its public HTTPS API, from throwaway accounts that get Circle's testnet USDC from the testnet DEX and deposit it through the settlement contract (no internal API, no minting);
- the load generator sends from one task per account, one request at a time, so nonces stay in order and the client's round trip does not cap the rate;
- soft latency is `POST /v1/tx` → the account's `receipt` on `WS /v1/stream` (the block message that carries it also carries its fills); hard latency is that receipt → its block inside a checkpoint the sequencer reports as accepted on Stellar, sampled for one receipt in ten and kept only for checkpoints whose blocks were all produced under load (after the load stops, the last receipts wait for an idle checkpoint);
- host `cpu_insns` per block is sampled once a second from `/v1/status` `host_metering` (the last block's `step`), for blocks produced under load;
- checkpoint size and fees come from the relayer's own log (`feeCharged`, simulation `minResourceFee`, transaction bytes), for checkpoints whose blocks all fall inside the load window; "per 1,000 lane tx" divides their total fee by the user transactions in those blocks | Real network fees and a user's real path to the API; no special access. One request at a time per account is how a client keeps nonces in order | A load generator next to the VM, to separate network latency from lane latency |
| DEC-049 | Signer rotation on a running lane (T-015): when the sequencer starts with a higher `[signers] epoch`, every checkpoint signed under an older epoch and not accepted goes back to waiting for signatures, keeping its old signatures. When it collects signatures, an old signature counts for each validator of the new set whose key it verifies under (the header does not hold the epoch and ed25519 signatures are deterministic), and only the others are asked. The §15 signing rules do not change | An admin rotation makes older epochs invalid at once (§13.2), so without this the relayer would retry a stale checkpoint forever while the escape timeout runs. Validators that stay in the set would refuse to sign an older seq again once they have signed a later one (§15), so asking them again cannot work | A validator endpoint that re-signs an older seq with the same header (a §15 change) |
| DEC-050 | Security pass (T-016): a validator with `rpc_url` reads the settlement contract's `Config` at startup and refuses to start unless `lane_id`, `engine_wasm_hash`, `config_hash` and `genesis_state_hash` match its own (§24 item 9); XDR from Stellar RPC is decoded with bounded `Limits` (depth 500, 1 MiB) instead of none; `E2E_NETWORK=testnet` runs the end-to-end script on testnet with a throwaway settlement contract, as the §24 freeze drill, so the demo lane's contract is never frozen. Filed: `stellar-xdr` GHSA-3xhm-p452-9wjh (serde only; Caravel decodes with `ReadXdr`; the Soroban 28 crates pin `=28.0.0`), `paste` unmaintained (build-time), `uuid` in the billing-cap function (not called with a buffer); details in `docs/SECURITY.md` | The sequencer has no RPC access, and a wrong engine there gives headers the contract rejects. Freezing the demo contract would end the testnet lane | Bump `stellar-xdr` when `soroban-sdk`/`soroban-env` allow 28.0.1 |
| DEC-051 | **M0.5 (P-02).** The Caravel Perps engine of record lives in a frozen, nested Cargo workspace at `lanes/perps/engine/`: `crates/{caravel-types, caravel-merkle, caravel-perps, caravel-testkit}`, `contracts/{perps-engine, engine-profile}` and `test-vectors/`. They were moved there with `git mv` and keep the M0 relative paths, manifests, `[profile.release]` and lock subgraph. The root workspace `exclude`s the directory and uses its crates by path. Nothing under it may change:
- `scripts/check-frozen.sh` pins its git tree (`versions.json lanes.perps.engine_tree`) and refuses local edits;
- `build-contracts.sh` builds the engine from it with `--manifest-path`;
- CI runs its fmt check, clippy, tests and `no_std` build on their own.

Checked on 2026-09-30:
- **Engine:** after the move it builds to `4571cd25…`, byte-identical, and all 109 compilation units have the same `-C metadata`.
- **Settlement contract** (root workspace): the paths of its `caravel-types` and `caravel-merkle` dependencies changed relative to the workspace root, and with them three units' `-C metadata`. On x86_64 Linux (CI) it now builds to `8280828f…`, which becomes the settlement build of record for new lanes (`versions.json artifacts.settlement_wasm_sha256`). On macOS arm64 the two variants swapped: `8a2fafbd…`, where it was `8280828f…`. The source is unchanged. Lane #1's deployed contract keeps running `8a2fafbd…` (`lanes.perps.settlement_deployed_wasm_sha256`, reproducible from tag `perps-m0`), and every settlement move re-records the build (plan item 2, flagged at Gate P1) | The deployed lane #1 settlement contract stores `engine_wasm_hash = 4571cd25…` and rejects any other engine (§13.3 check 3), so the platform split must not change one byte of the engine. Cargo's `-C metadata`, and through it the Wasm, depend on package paths relative to the workspace root; a nested workspace with the same layout keeps every one of them | A perps engine change (then a new lane, or a migration to the app SDK) |
| DEC-052 | **M0.5 (P-04).** The platform reads the M0 formats without knowing the app. `platform/crates/caravel-core` holds them. It is `no_std` and depends on no lane.

Copied verbatim from the frozen crates (a test compares the files): codec, tags, fixed, inbox, checkpoint, step, preimage, Merkle.

Reinterpreted, **bytes unchanged**:
- **Blocks (§9.6):** entries are `Inbox | Feed | User`. Entry type 2, "oracle" in M0, is a generic signed **feed** with an opaque payload. A user entry is a `TxEnvelopeV1`: the `LaneTxV1` header with an opaque body of `body_len` bytes.
- **Transaction kinds (§9.3):** 4 `WITHDRAW`, 5 `ADD_SESSION_KEY` and 6 `REVOKE_SESSION_KEY` are platform-standard, with the M0 bodies. 7 to 15 are reserved; 1 to 3 and 16 and up are app kinds.
- **Receipts (§11.10):** the container is generic, and each event is `type · len · fields`. The platform events are 6 `DEPOSIT`, 7 `FORCED_WITHDRAWAL_PROCESSED` and 10 `COMMITMENT`.
- **Code ranges:** receipt codes 0–9 and 40–59 are the platform's; 10–39 and 60+ an app's. Fatal codes 1–31 are the platform's (13 and 14 become "unknown feed key" and "duplicate feed slot"); 32+ an app's.
- **State (§9.10):** every lane's state starts with the 209-byte frame and ends with the 160-byte `CommitmentV1`. `StateFrameV1` reads both. The perps `next_order_id` (offset 168) and `flags` (offset 208) are the app-reserved `app_word` and `app_flags`, and the magic is the app's.
- **Vectors:** the generic ones get byte-identical copies in `platform/test-vectors/`.

**Consensus is unchanged.** The engine Wasm still decodes strictly, the app's mempool check stays strict, and the generic decoder accepts everything the strict one does. `lanes/perps/node/tests/format_compat.rs` checks it on every frozen vector, the 70-block golden trace, the fixture store's states, and a 512-case byte-flip property test.

`scripts/check-deps.mjs` (CI) fails if a platform package reaches `lanes/`. It lists the three remaining M0 couplings as exceptions that must shrink: `caravel-runtime` (P-05), `caravel-node` (P-06), `settlement` (P-09) | A platform that runs any app has to read blocks, receipts and states without the app's types. Reading the same bytes, instead of new formats, keeps lane #1 and its history valid | An app whose state does not fit the frame (it would need a new frame version) |
| DEC-053 | **M0.5 (P-05).** The runtime runs any app through one trait, `caravel_runtime::LaneApp`, with static dispatch: each app's node binary links its own implementation (P-06 adds the node half, `NodeApp`, and the binaries). The runtime reads the state frame, blocks, receipts and commitments itself (DEC-052). The trait gives it the rest:
- the decoded state, its limits, account nonces and pending-withdrawal count, to build blocks;
- a strict transaction check (`tx_decodes`), so the mempool takes only what the engine decodes;
- a native genesis and step that name the fatal entry, to quarantine it (the Wasm path does not report it);
- strict receipt decoding and event text, for acceptance and views;
- each account's escape equity, for the account leaves;
- optional feeds: decode (slot, publish time, payload), the engine's fatal checks, inclusion, and the validator's live flag. An app without feeds refuses every feed.

`lanes/perps/node` (`caravel-perps-node`) holds `PerpsApp`, the M0 rules as they were: oracle updates are feeds with the market id as slot, a configured key and a valid signature to be admitted, and the 60 s live window. The perps views (accounts, markets, books, fills), the account leaves from margin equity, the runtime's tests, the golden trace, the fixture store and `bench_full_caps` moved there too.

Checked on the split: the M0 golden trace and API snapshots are byte for byte the P-01 ones, without regenerating; the fixture store opens at the trace's end state; the parity gate passes; the engine and settlement hashes are unchanged. Two texts change, neither in consensus nor in the API: a quarantined feed's incident source reads `Feed(m)` instead of `Oracle(m)` in the logs, and `Reject::Decode` keeps its M0 message ("does not decode as LaneTxV1") until P-06 lets the app word it. `caravel-runtime` no longer depends on any lane, so its check-deps exception is gone. `caravel-node` now depends on `caravel-perps-node` until P-06 | Static dispatch keeps the hot path free of trait objects and lets the app's state be a real type. The Wasm stays the only consensus path (DEC-002); the native calls are for diagnosis and tests | A process that must run several apps at once (hosted trials run one process per lane, DEC-057) |
| DEC-053 (cont.) | **M0.5 (P-06).** The node and relayer halves of the app interface.
- **`caravel_node::NodeApp`** (on top of `LaneApp`). It gives:
  - the app's `[app] template` and its genesis config from a lane file;
  - the execution limits in that config;
  - the account view;
  - a view cache kept between blocks, and what each block adds to the stream;
  - its own public routes and stream messages;
  - its feed route, if any.
- **What the node serves for every app:**
  - public: `/v1/tx`, `/v1/status` (now with `template`), `/v1/accounts/{account}`, blocks, checkpoints, proofs and the stream (`{blocks, account}` plus the app's subscription fields);
  - internal: `/internal/inbox`, `/internal/{feed}` and the checkpoint routes.
- **Binaries.** `caravel-node` is now a library, with `caravel_node::cli::{Command, run}`. Each app's binary flattens those commands into its own CLI. Lane #1's binary is `caravel-perps-node`: the platform commands plus `tx`, with `loadgen` and `bench_full_caps` as its examples. At startup it refuses:
  - a lane file for another template;
  - an engine Wasm other than the one the lane file names.
- **Perps.** `PerpsApp` keeps the M0 JSON. It serves `/v1/markets`, `/v1/markets/{id}/book` and `/v1/markets/{id}/trades`, with 1,000 fills kept per market. Its stream order is `block`, `fill`, `book`, `receipt`, `account`. Its feed route is `/internal/oracle`, with the M0 errors (`DECODE`, `BAD_ORACLE`).
- **Relayer.** `platform/relayer` keeps the inbox, checkpoint and metrics loops. An app's feeds are ES modules the config lists under `feeds: [{module, intervalMs, options}]`:
  - each exports `createFeed(host, options)`;
  - the host gives the lane id, the RPC, public GETs and `postFeed(route, hex)`;
  - a module reads its own keys from the environment;
  - an M0 config with `loops.oracle` is refused with a pointer to `feeds`.
  - The perps oracle feeder, its price sources and the Reflector client moved to `lanes/perps/relayer-feeds` (its own package, with the `oracle_update.json` vectors). The VM release adds `relayer-feeds/perps`.
- **Checked:**
  - the M0 API and genesis goldens are unchanged, now from `lanes/perps/node/tests`;
  - the node API, validator and replay tests pass unchanged against `caravel-perps-node`;
  - `scripts/e2e-local.sh` passes with the feed module;
  - check-deps has no node exception left (only `settlement`, P-09).

Changed outside consensus:
  - `/v1/status` gains `template` (sequencer and validator);
  - the binary is renamed, which the VM picks up at P-07 | One node library for every app, with the app's parts behind one trait each, and relayer feeds the platform does not parse | An app needing a route or loop the traits cannot express |
| DEC-054 | **M0.5 (P-06).** The lane file's platform layout:
- **Generic sections:** `[lane] name`; `[app] template, engine_wasm_sha256`; `[node]` (not consensus); `[access] mode, allowlist`; `[limits]`. The limits are `min_deposit`, `min_withdrawal`, `max_accounts`, `max_session_keys`, `max_txs_per_account_per_block`, `max_entries_per_block`, `max_block_bytes`, `max_pending_withdrawals`, `exec_cpu_limit` and `exec_mem_limit`.
- **App section:** a table named after the template. Perps uses `[perps.accounts]`, `[perps.oracle]`, `[perps.funding]`, `[perps.fees]`, `[perps.limits]` (`max_orders_per_side`, `max_open_orders_per_account`) and `[[perps.markets]]`.
- **Refused:** any other top-level table.
- **M0 files:** a file without `[app]` is an M0 file. Only its app reads it; `PerpsApp` keeps the M0 parser.
- **Converted:** both perps lane files now use the platform layout. The M0 copies are test fixtures (`lanes/perps/node/tests/fixtures/*.m0.toml`).
- **Checked:** the two layouts give byte-identical `GenesisConfigV1` and genesis state for both lanes. Lane #1 keeps `lane_id 319b46e9…`, `config_hash f4b9db09…` and `genesis_state_hash 22702d9f…`.
- **Engine check:** a node refuses an engine Wasm other than `[app] engine_wasm_sha256` (sequencer and validator config load, replay, witness).

The lane file is a node input, not a consensus format. The bytes it produces are unchanged | Every template needs the same generic settings, and a node must know which app a file is for before parsing the rest | `AppGenesisV1` (Phase 2) gives the generic sections their own bytes |
| DEC-058 | **M0.5 (P-07a).** The perps oracle's first source is Coinbase's public WebSocket ticker, instead of Reflector (DEC-041), and the feed runs every block (1 s) instead of every 2 s.
- **The stream** (`lanes/perps/relayer-feeds/src/coinbase.ts`): one connection, the `ticker` and `heartbeat` channels for every product. It reconnects with backoff, and after 30 s of silence.
- **Freshness:** a quote is fresh for 5 s from its trade (`PriceSource.maxAgeSecs`, which `firstFresh` now honours). A heartbeat whose `last_trade_id` is the quote's trade refreshes it, for up to 120 s after the trade, so a quiet market such as XLM-USD stays priced between trades. A missed trade, a dead stream or an old trade falls through to Coinbase's spot API, then Reflector.
- **Checked 2026-09-30 on the live feed:** 38 ticker messages in 5 s for BTC, ETH and XLM, with no key; heartbeats about once a second per product. The channel is `heartbeat`, singular: `heartbeats` is refused.
- **Publishing:** the one-tick and 10 s heartbeat rules are unchanged, so a market moves at most once per block. The engine's staleness, circuit-breaker and future-time rules are unchanged.
- **Trust:** prices are still signed by the team's oracle key, and the web app says they come from Coinbase.

Pyth was the first choice. Hermes has required a Pyth Terminal API key since 2026-08-26 (it answered 401 when checked), and Pyth Pro needs a subscription. Both are paid after a trial, so the human chose Coinbase. Pyth Pro verified in-engine stays planned for new lanes (P-08b) | Free and sub-second, with no key and no new paid resource. The fallbacks keep a price when the stream drops | A subscription to a signed oracle (Pyth Pro, P-08b), or sub-second blocks, since one price per market per block caps what a faster feed can add |
| DEC-059 | **M0.5 (P-07b).** The perps web app connects any Stellar wallet through Stellar Wallets Kit 2.7.0 (`@creit.tech/stellar-wallets-kit`, MIT), instead of Freighter only.
- **Wallet layer:** `src/api/wallet.ts` keeps its functions (connect, current, the network check, `signSep53`, `signTx`), so the pages are unchanged apart from saying "wallet" instead of "Freighter".
- **Loading:** the kit loads on first use, with `defaultModules()` (the wallets that need no extra setup; WalletConnect, Ledger and Trezor need configuration and are left out) and its dark theme. The main bundle did not grow (1,003,601 bytes against 1,012,212).
- **Message signatures:** wallets return them in different encodings, and some can't sign messages at all: in 2.7.0, Albedo, Rabet and Ledger throw on `signMessage`. So the app checks each one locally against the SEP-53 hash (`@noble/ed25519`) before posting it. A wallet that fails can still deposit, claim and escape, and is told trading needs a SEP-53 wallet. The engine's check is the same one; this only moves the failure out of the block.
- **Tests:** the frozen `add_session_key_sep53_owner` vector passes the check, and a tampered, wrong-key or wrong-message signature does not; base64 and hex are both decoded; a raw-message signature and a wallet that can't sign messages are refused. Headless Chrome opened the picker against the live lane.
- **Not yet tested:** manual testnet flows with Freighter and xBull (RESULTS) | Users bring the Stellar wallet they have. The kit covers the wallets the ecosystem uses and is the one developers.stellar.org lists | A wallet the kit lacks, or SEP-53 support changing in a wallet (the local check shows it) |
| DEC-060 | **M0.5 Phase 2 formats, approved by the human on 2026-09-30 as proposed in §20.4:** `AppGenesisV1` (`CVAPPGN1`), the SDK state layout (`StateFrameV1` · embedded config · `AppAccountV1`s · app globals · pending · `CommitmentV1`), the SDK's standard pipeline, and Payments 0.1.0 (`CVSTPAY1`, kind 16 `TRANSFER`, codes 10–13, flat `transfer_fee` to the treasury, recipients must exist, INV-PAY1). They freeze when their vectors land (P-08, P-10) | One genesis format and one state layout for every SDK app, so the platform, the console and the registry treat templates alike. Payments is the smallest app that exercises every standard path | A template that needs feeds (P-08b), or state that doesn't fit the layout (a V2) |
| DEC-061 | **Settlement build of record for new lanes, approved by the human on 2026-09-30:** new lanes use the platform's settlement build `8280828f…`, the Linux build since the P-03 moves (`artifacts.settlement_wasm_sha256`). Lane #1 keeps its deployed `8a2fafbd…`, reproducible from tag `perps-m0` (`lanes.perps.settlement_deployed_wasm_sha256`). The registry that was to approve both was cut with the concept change (§20.3); lane files pin the build instead (`settlement_wasm`) | The source is lane #1's; only the build paths changed. Freezing settlement as well was not worth the churn | A settlement code change, which gets its own build and DEC |
| DEC-062 | **M0.5 (P-08).** `platform/crates/caravel-app-sdk` implements §20.4. It is `no_std`, depends only on `caravel-core`, and builds for `wasm32v1-none`.
- **App interface:** an app is an `AppEngine`, with static dispatch (`step::<App, _>(state, block, crypto)`). It declares its template, versions, state magic and permission bits. Its hooks are `params`, `body_len` and `session_permission` per kind, `apply`, `begin_block`/`end_block`, `free_balance`, `escape_equity`, `is_empty` (slot reuse) and `before_forced_withdrawal`. It gets an `AppCtx`: the state, its params, the account index, the time, the entry and its events. Apps can't create accounts in V1.
- **Crypto:** the SDK's own `Crypto` trait, the same as the perps engine's (`native` feature: `NativeCrypto` traps, `DiagnosticCrypto` names the entry).
- **Decoding:** strict, as M0's. A user entry whose kind is unknown or reserved (7 to 15), or whose body has the wrong length, is `BAD_ENTRY_ENCODING` for that entry, checked before anything else. A FEED entry is `UNKNOWN_FEED_KEY`.
- **Test app** (`testapp` feature): kind 16 `COUNT`, a per-account counter in `ext`, a lane total in `app_globals`, and a block-end event. It exercises every hook.
- **Tests:**
  - `tests/pipeline.rs`: genesis rules, accounts, signer rules, nonces, rate limits, session keys, withdrawals, forced withdrawals, the commitment and its roots, slot reuse, allowlist bounces, every fatal case, strict state decoding, and a SDK-INV1 property test.
  - `lanes/perps/node/tests/sdk_conformance.rs`: ten scripted blocks run through the frozen perps engine and the test app with the same limits and keys. It requires identical codes, platform events, frame, commitment (both roots) and every account's nonce, balance and session keys, and it reaches every standard code. Mutating the SDK's new-account nonce or its session-key limit makes it fail.
- **Vectors:** `platform/test-vectors/app_genesis.json` and `sdk_state.json` (genesis, then two blocks with their receipts) freeze the formats | The standard paths are code the SDK owns once, so every app gets M0's rules without copying them, and the frozen perps engine remains the reference they are checked against | An app that must create accounts or needs feeds (P-08b) |
| DEC-063 | **M0.5 (P-09).** The platform no longer depends on any lane: `scripts/check-deps.mjs` has no exceptions left.
- **`platform/crates/caravel-harness`** (test-only) is a native lane on the app SDK, driven block by block like the testkit's: queues, blocks, checkpoints, records, the previous block hash, `escape_leaves` and `pending`. It runs the SDK's test app by default and checks SDK-INV1 after every block.
- **The settlement contract** takes its codecs from `caravel-core` instead of `caravel-types` and `caravel-merkle`; they are byte-identical copies (DEC-052). Its 74 tests run on the harness instead of the perps testkit, unchanged apart from the imports and where the escape leaves come from.
- **New settlement build of record:** `fb68ee32…`, the x86_64 Linux build from CI (DEC-033; CI run 36733965429, 2026-09-30). It is 42,900 bytes, like `8280828f…`, and the code is the same: after the crate switch the compiler orders the function-type and function-index tables differently (902 bytes differ, measured on macOS). The macOS build of the same source is `d2c67d28…`, so `build-contracts.sh` only warns there, as DEC-033 allows. It replaces `8280828f…` (DEC-061), which no lane ever deployed. Lane #1 keeps `8a2fafbd…`. The human is told at Gate P2 | The platform's contract is tested with the platform's own lane, so no app's engine is part of its test surface. A build identity change on a contract nobody deployed costs nothing | A settlement code change, which gets its own build and DEC |
| DEC-064 | **M0.5 (P-10).** Caravel Payments, the second template (§20.4.4), is built on the app SDK and runs on the generic node with no payments code in `platform/`.
- **Crates:**
  - `lanes/payments/app` (`caravel-payments`, no_std): the `AppEngine`;
  - `lanes/payments/contracts/payments-engine`: the contract, 48,434 bytes of its 65,536 budget, sha256 `f8384963…`. The x86_64 Linux build of record from CI (DEC-033, run 36738100913) and the macOS build agree;
  - `lanes/payments/node` (`caravel-payments-node`): `PaymentsApp` as `LaneApp` and `NodeApp`, the account view (`balance`, `next_nonce`, `session_keys`), the `[payments]` lane-file section, and `tx transfer|withdraw`.
- **Local lane:** `lanes/payments/config/lane.caravel-payments.local.toml`, with the public fixture treasury key (seed `0x22`). lane_id `5528e7c7…`, config_hash `8c16d400…`, genesis_state_hash `589cb7a6…`.
- **Tests** (`lanes/payments/node/tests/`):
  - scenarios P1–P12, each replayed through the Wasm byte for byte, fatal blocks and bad genesis included;
  - the INV-PAY1 property (96 cases), which also checks that the treasury holds exactly the fees plus what users sent it;
  - parity on 300 random blocks, and 10,000 behind `--ignored` (31 s locally);
  - the budget: the costliest block the limits allow (a checkpoint over 256 accounts and 512 pending withdrawals, with 49 SEP-53 transfers filling 11,951 bytes) costs 68.2M instructions (34% of 200M) and 3.6 MB (8%). Budget exhaustion is deterministic;
  - the sequencer API over HTTP and WebSocket, with proofs checked against the header;
  - lane-file goldens and refusals;
  - vectors in `lanes/payments/test-vectors/payments.json`, which freeze the format (DEC-060).
- **e2e:** `E2E_TEMPLATE=payments ./scripts/e2e-local.sh`. A new step 4c, a forced withdrawal asked for on Stellar and claimed there, runs for both templates. Both passed on a local quickstart on 2026-09-30: payments in 197 s and perps in 164 s, each with 5 checkpoints replayed from Stellar. CI runs the e2e for both, and runs the payments parity gate and no_std build on every push | A second app on the same node, settlement contract and relayer shows the platform carries a lane without app code of its own. Step 4c covers the forced-withdrawal path, which the M0 e2e left to unit tests | Fee sponsorship, transfers that create accounts, or a template that needs feeds (P-08b) |
| DEC-065 | **M0.5 (P-11).** Node groundwork for the deploy tool (§20.3), released to the VM in one go:
- **Lane files** may hold `[env.<name>]` tables (deployments). `LaneFile::parse` removes `env` before anything else reads the file, so genesis, the app parsers and the perps M0 layout never see it, and `env` is a reserved template name. Tests show genesis is byte-identical with and without an `[env]` block for both perps files, both M0 fixtures and the payments file.
- **`/v1/status`** of the sequencer and of each validator reports what the node runs: `lane_id`, `config_hash`, `settlement` (C…), `engine_wasm_sha256`, `network_passphrase` and `release` `{version, commit}`. The commit is `CARAVEL_COMMIT` at build time, which the CI release sets, and `null` in local builds. The validator also reports `lane_name`. The deploy tool compares a host against the lane file and the chain with these fields.
- **`export-proofs --config <validator.toml>`** writes every exit a validator's store holds: every escape leaf of its last accepted checkpoint and every withdrawal leaf, each with its proof. With `rpc_url` set, that checkpoint must be Stellar's `last_checkpoint()` (seq and header hash). It reuses the escape and withdrawal proof code, and on the fixture store its proofs equal the per-account routes'.
- **The CI release** builds both node binaries, vendors the relayer's production `node_modules` (pure JS; package-lock integrity is checked in CI), covers the relayer and feed files in `SHA256SUMS`, and `deploy-vm.sh` installs `COMMIT` and `SHA256SUMS` in `/opt/caravel`. Hosts no longer run `npm` | The deploy tool reads hosts and the chain rather than a state file, so the nodes must say what they run. Destroy needs every exit in one file | — |
| DEC-066 | **M0.5 (P-12).** `platform/crates/caravel-deploy`: the deployment schema and the pure plan engine.
- **`[env.<name>]`** (`manifest.rs`, `deny_unknown_fields`) has these fields:
  - `network` (`local` or `testnet`; mainnet is refused by name), `rpc_url`;
  - `admin`, `usdc` (`circle` or `local`), optional `settlement` (a pinned C… address) and `settlement_wasm`, `threshold`, `[settlement_params]` (`min_deposit` defaults to `[limits] min_deposit`);
  - `[[validators]]` (`name`, `key`, `weight`);
  - `[sequencer]` (`port`; validator `i` listens on `port + i`);
  - `[relayer]` (`account`, `feed_keys`, and opaque `feeds`);
  - `[host]` (`provider` `local` or `ssh`, `address`, `transport` `ssh` or `gcloud-iap` with `project` and `zone`, `public_url`, `root`).
- **Every broken rule is reported at once.** Key fields must name Stellar CLI identities (a G… key or a 64-hex seed is refused). A secret key or a seed phrase anywhere in the table is refused without being echoed. Hashes elsewhere are fine.
- **Addresses are computed offline** (`address.rs`), matching `stellar contract id wasm` and `stellar contract id asset`:
  - the settlement contract comes from the admin and the salt `H("caravel/settlement" ‖ lane_id)`;
  - a local lane's USDC is the asset contract of `USDC:<admin>`.
- **The plan** (`plan.rs`) is a pure diff from the resolved deployment to ordered steps and blocking problems.
  - **Steps:** fund, create USDC, upload, deploy, wipe host data, release, write, start, restart, rotate, stop.
  - **Problems:** a constructor field changed, code drift or unknown code, frozen, a pinned contract missing, USDC missing, a signer set reused, a host not ready, a node mismatch.
  - **Order:** validators start before a rotation names them, and the sequencer restarts after it.
  - **The target epoch** is computed first, so the sequencer's config can carry it.
- **Tests:** 11 plan cases, 9 of them golden texts (`tests/golden/plan-*.txt`), and 6 manifest tests.
- **CI** fails if the name of one well-known IaC tool appears anywhere in the repo (the human's copy rule) | A plan that names every address and step before anything is sent, with the chain as the only record, is what the prior art lacks on Stellar (docs/SOURCES.md, 2026-09-30). A pure diff can be tested case by case | — |
| DEC-067 | **M0.5 (P-13).** `plan` and `apply` on Stellar with the `local` provider.
- **Commands.** Every app binary flattens `caravel_deploy::cli::Command` (`plan <lane> --env <name> [--release-dir]` and `apply … [--yes]`) next to the node commands, so genesis runs in-process with the app. The `caravel` dispatcher reads `[app] template` and runs `caravel-<template>-node`. Installed as `stellar-caravel`, it is a Stellar CLI plugin: `stellar caravel plan …`.
- **Reading the chain** (`chain.rs`) takes two batched `getLedgerEntries` calls and no simulation. It reads:
  - the admin and relayer accounts;
  - the USDC contract;
  - the settlement code;
  - the settlement instance: code hash, `Config`, `Epoch`, `LastCkpt`, `InboxCount`, `Frozen`;
  - `Signers(epoch)`, and `SignersEpoch(H(the file's set))`.

  `caravel_node::scval` now holds the storage decoders that replay also uses. `stellar_rpc::ledger_entries` keeps `liveUntilLedgerSeq` for TTL horizons.
- **Rendering** (`render.rs`): every file lives under the host root (`config/`, `keys/`, `data/`, the release) and uses absolute paths.
  - The rendered `lane.toml` is the lane file without `[env]`, with the same genesis (tested).
  - No secret is in any rendered file (tested). Validator keys are key files; the sequencer's internal token and the relayer's keys come from `keys/env`.
- **Ports.** A validator named `n` listens on the sequencer's port + n; any other name sets `port`. So a replacement never takes the port of the validator it replaces, and a health check compares the validator's key.
- **The `local` provider** (`local.rs`) runs processes under `.caravel/<lane>/<env>/`, detached, each with a PID file and a log. Keys are mode 600 in a mode-700 directory. The internal token comes from `/dev/urandom` and is kept across applies.
  - Each start records a fingerprint of the configs the node read (`run/<node>.started`).
  - A node is restarted when that fingerprint, the sequencer's signer epoch, or its release commit differs from the plan. That covers an apply stopped after writing a file but before the restart.
- **The release** is a CI artifact (`--release-dir`) or this checkout's builds, named `local-<H(binary ‖ Wasm hashes)>`. Testnet needs the settlement build of record (DEC-033). A local network accepts this machine's build, with a note.
- **Apply** checks each Stellar result against the plan (the uploaded Wasm hash, the deployed address). It then reads everything back: a finished apply leaves "No changes".
- **Tested on a local quickstart, 2026-09-30:**
  - The payments lane came up from its lane file, its first checkpoint was accepted on Stellar, and a second apply showed "No changes".
  - Validator 3 was swapped for validator 4, and the apply was killed with `-9` twice: once after validator 4 started, once during the rotation. Each re-run finished the swap, ending at epoch 2 with validators 1, 2 and 4 and every signed checkpoint accepted.
  - Three bugs were found this way and fixed: network flags placed after `--`, validator ports taken from list position, and a health check another validator could answer | Idempotent steps that re-read Stellar and the host make an interrupted apply safe to re-run without a journal | — |
| DEC-068 | **M0.5 (P-14).** `status` and `destroy`, and the e2e on the tool.
- **`status [--json]`** shows:
  - height and signer epoch;
  - the last accepted checkpoint and its age;
  - when anyone could freeze the lane: the earlier of the last checkpoint's `accepted_at + escape_timeout_secs`, and the oldest unprocessed inbox message's `enqueued_at + force_inclusion_window_secs`;
  - the relayer's XLM;
  - TTL horizons;
  - each node, up or down;
  - whether the deployment matches the lane file.
- **`destroy`.** A contract can't be deleted, and nobody (the admin included) can freeze one at will. So destroy:
  1. drains until every signed checkpoint is accepted;
  2. stops the relayer and the sequencer;
  3. runs `export-proofs` on a validator, so every escape and withdrawal leaf, checked against Stellar's `last_checkpoint()`, goes to `.caravel/<lane>/<env>/exit.json`;
  4. triggers with a 1-stroop forced withdrawal of the admin's own lane account, which the stopped lane leaves unprocessed;
  5. freezes once the force-inclusion window has passed (1 h with lane #1's params, not the 6 h escape timeout).

  The validators keep serving proofs unless `--stop-validators` is given. `--pay-out` claims every exit for its owner: the contract pays only the lane account's owner and needs no authorization. `--no-wait` stops after the trigger and prints the time. `--wipe` removes host data. Each step checks what is done, so destroy can be run again. Without `--yes`, the operator types the lane's name.
- **`scripts/e2e-local.sh`** now writes the template's lane file plus an `[env.e2e]` deployment (20 s window, 30 s timeout) and runs:
  1. `caravel apply`, then a plan that must show no changes;
  2. the user flows;
  3. a validator swap made in the lane file and applied as a rotation, with a checkpoint signed by the old set but not yet submitted, which the new set signs again;
  4. a forced withdrawal;
  5. `caravel destroy`;
  6. escapes paid from `exit.json` at the frozen ratio;
  7. replay, whose escape proof must equal `exit.json`'s.
- **Results on a local quickstart, 2026-09-30:** payments passed in 134 s and perps in 178 s | A lane's wind-down is part of its lifecycle. The fastest legitimate freeze is the censorship trigger the contract already has | Contract upgrades or a deletable settlement design |
| DEC-069 | **M0.5 (P-15).** The `ssh` provider (`ssh.rs`), and one `HostProvider` over `local` and `ssh` for plan, apply, status and destroy.
- **Transport:** `ssh` (`BatchMode`), or `gcloud compute ssh/scp --tunnel-through-iap` for a GCP VM. A dropped connection (exit 255) is retried, since every step can be repeated.
- **Reading the host** is one script. It checks the prerequisites (systemd, passwordless sudo, a `caravel` user, the root, rsync, curl, Node 22+, and Caddy with a public URL) and reads:
  - the release `COMMIT`;
  - the hashes of `<root>/config/*`, `/etc/systemd/system/caravel-*.service` and `/etc/caddy/Caddyfile`;
  - the start fingerprints;
  - the units' states, including running validator units the lane file doesn't list;
  - each node's `/v1/status` through `curl` on the host.

  The tool installs no packages. A missing prerequisite is a plan problem that points to `provision.sh`.
- **Writing:** the release is a tarball (binary, Wasm, relayer with `node_modules`, the template's feeds and web app, `COMMIT`), `rsync`ed into `<root>`. Configs go through `sudo install`; units go to `/etc/systemd/system` followed by `daemon-reload`; the Caddyfile goes to `/etc/caddy` followed by a reload. Keys stream over ssh stdin into mode-600 files and never touch this machine's disk. The internal token is made on the host once and kept. Nodes are the units `caravel-sequencer`, `caravel-validator@<name>` and `caravel-relayer`.
- **Rendered host files:** units and a Caddyfile (API, validators' public APIs, `/internal/*` and `/v1/sign` closed, the web app when the release has one). For lane #1's deployment the units are byte for byte the ones its VM runs (tested).
- **Read-only check against lane #1's VM, 2026-09-30:** `plan` with its deployment written out and the installed CI release shows no Stellar step and no problem, the same release and the same units. Only config normalization remains (the lane file is renamed to `lane.toml`, and the node configs are regenerated). The write path runs for the first time when lane #1 is brought under the tool (P-16), with the human's go-ahead.
- **Cut:** `--preflight`. The human judged check-store and the shadow validator unnecessary for releases that don't change consensus. `import`: lane #1 is the only lane deployed before the tool, so its `[env.testnet]` is written by hand from its chain state and deploy files | A team's own Linux host is the self-hosting case. Reusing lane #1's exact units keeps the first import to config files | Several lanes per host, or hosts without systemd |
| DEC-070 | **M0.5 (P-16).** Lane #1 is under the deploy tool.
- **Its lane file carries its deployment:** `[env.testnet]` in `lanes/perps/config/lane.caravel-perps.testnet.toml`, with:
  - the pinned contract `CBIHBEUZ…` (it was deployed with a random salt) and its build `8a2fafbd…`;
  - the M0 params `{3600, 21600, 3600, 2}`;
  - validators `1..3`, threshold 2, and the sequencer on 8080 in production mode;
  - validators polling Stellar every 30 s;
  - the relayer's intervals and the perps oracle feed (DEC-058), as its VM had them;
  - the VM over IAP, with the public URL.

  Genesis is unchanged (tested).
- **`plan --diff`** prints, for each file a plan would write, a unified diff of the host's version against the lane file's.
- **The first apply, on 2026-09-30, with the human's go-ahead:**
  - the plan had no Stellar step, the same release (`2159e7a`) and the same units;
  - 7 config files were normalized: the lane file became `lane.toml`, comments changed, `weight = 1` became explicit, and JSON keys were reordered;
  - the 5 nodes restarted over ssh in about 4 minutes;
  - afterwards the plan reports "No changes", checkpoints keep being accepted with signed equal to accepted, oracle prices stay under 9 s old, and the web app answers.
- **The imperative VM path is gone:** `scripts/deploy-vm.sh`, `scripts/vm-preflight.sh` and the hand-written node configs in `lanes/perps/deploy/testnet/`. `provision.sh` stays for a new host. The units and the Caddyfile stay as exact copies of what lane #1 runs, and a test checks them against the renderer. The RUNBOOK now upgrades and rotates through `caravel apply` | One way to change a lane: the lane file | — |
| DEC-071 | **M0.5 (P-17).** A payments lane on Stellar testnet from its lane file, through its whole lifecycle.
- **`--wasm-dir`** (plan, apply, status, destroy) takes the contracts from CI's `contracts-wasm` artifact (the x86_64 Linux builds of record, DEC-033) while the binaries come from this checkout. A macOS machine can then deploy the settlement build of record `fb68ee32…` instead of pinning its own build.
- **The run:** `E2E_NETWORK=testnet E2E_TEMPLATE=payments E2E_WASM_DIR=<CI artifact> ./scripts/e2e-local.sh` on 2026-09-30, using the `local` provider against testnet, passed in 215 s:
  - `caravel apply` deployed `CDVVLXV6T5LBIQUS2KHYD7C6UMUBMIOLG4O27G6UMMQXYGJTTPXO47PX` at its derived address;
  - Circle's USDC came from the testnet DEX;
  - deposits, a transfer with its fee, and a withdrawal claimed on Stellar;
  - the lane file's validator swap was applied as a rotation, with checkpoints 6 and 7 accepted under epoch 2;
  - a forced withdrawal was claimed;
  - `caravel destroy` froze the lane 10 s after its trigger;
  - both users escaped from `exit.json` at 1:1;
  - replay from Stellar covered 11 checkpoints.
- **An e2e race, found and fixed:** a relayer given SIGINT finishes the step it is in, which can be a submission. So step 4b now waits for the relayer to exit before choosing the stale checkpoint. The first attempt left its throwaway test contract `CAKOSGUC…` unfrozen, holding only test funds | The same file-driven lifecycle works on the real network | — |
| DEC-072 | **M0.5 (P-19).** The settlement token is configurable (asked for by the human, 2026-09-30).
- **The contract already allowed it.** The settlement contract holds any SEP-41 token: it only calls `transfer` and `balance`, and the constructor argument named `usdc` in the M0 ABI is simply its address. No contract, engine or frozen-format change was needed; the name stays in the ABI.
- **The lane file's `[env.<name>]` names its `token`:**
  - `"circle-usdc"`: Circle's testnet USDC;
  - `{ asset = "CODE:ISSUER" }`: a Stellar asset through its Stellar Asset Contract, which `apply` deploys when missing, since anyone may;
  - `{ contract = "C…" }`: any SEP-41 contract;
  - `{ local = "CODE" }`: a test asset issued by the admin, on local networks.

  The plan shows the token's address and a `create token contract` step. 1–12 character codes are supported (a vector for a 12-character code is in `address.rs`).
- **Decimals guard:** `NodeApp::token_decimals` lets a template require the token's decimals. Perps requires 7, because its collateral, prices and margins are in 10^-7 units. A Stellar Asset Contract always has 7; for another contract, the tool reads `decimals()` by simulation and refuses a mismatch. Payments takes any token.
- **Lane #1** is now `token = "circle-usdc"`, and its plan still shows no changes. The local payments lane uses `{ local = "USDC" }`. `E2E_TOKEN_CODE=EURC E2E_TEMPLATE=payments ./scripts/e2e-local.sh` passed in 157 s with a EURC lane.
- **What the tool can't check:** a contract token that takes a fee on transfer, or that rebases, would break the vault's accounting. The claims (§2.4) and the README say so | A lane's economics shouldn't be tied to one stablecoin, and the contract never was | A token interface beyond SEP-41 |
| DEC-073 | **M0.6 (C-01).** A plugin protocol between the `caravel` CLI and each template's binary.
- **Where it lives:** every template binary gets a hidden `plugin` subcommand (`caravel_node::plugin`). Each command prints one JSON document, and `PROTOCOL = 1` is bumped when a reply changes shape.
- **The commands:**
  - `plugin info`: the template, this binary's version and commit, the engine's file in a release, and the token decimals the template needs;
  - `plugin example`: the lane file `caravel init` starts from (`lanes/<t>/config/init/lane.toml`). Its placeholders are `{{name}}`, `{{port}}`, `{{id.<role>}}` (a Stellar CLI identity) and `{{g.<role>}}` (that identity's key). `example_roles` and `fill_example` read and fill them;
  - `plugin body [--decimals D] <body…>`: a transaction body's kind and bytes, in the template's own `tx` syntax. With `--decimals`, token amounts are token units ("12.5"), parsed with integer arithmetic.
- **Bodies, not envelopes.** The envelope is the platform's `TxEnvelopeV1` for every template (perps' `LaneTxV1` is the same bytes, `format_compat.rs`). So `caravel` builds, hashes, signs (SEP-53) and submits it, and no key ever reaches a template's binary. That replaces the `build-tx` command in the plan of record.
- **The perps example** generates its own backstop, treasury and oracle identities rather than the public fixture keys, so its oracle key in genesis and the relayer's feed key are one identity.
- **Unchanged:** the `tx` command still reads base units, and the existing node commands are as they were. Together with `genesis`, `replay` and `export-proofs`, this is everything the CLI needs from a template | The CLI must work for any template without linking it, and a third party's template must not need a fork | A template needs a deploy-time hook beyond these (e.g. validating its `[env]` feeds) |
| DEC-074 | **M0.6 (C-02).** The deploy tool no longer links a template.
- **The interface:** `caravel_deploy::template::Template` has `name`, `token_decimals` and `genesis` (lane id, config hash, genesis state hash). It has two implementations:
  - `InProcess<A: NodeApp>` is used by the template binaries' own `plan`, `apply`, `status` and `destroy`, which still work;
  - `Plugin` drives `caravel-<t>-node` through its plugin protocol (DEC-073). It is found in `CARAVEL_PLUGIN_DIR`, then next to the running binary, then on PATH, and is refused when it speaks another protocol or is another template.
- **The plugin's genesis input.** The plugin runs `genesis --config -` (stdin, new) over the genesis document the hosts get (`lane.toml`, the parsed sections re-serialized). A test pins it equal to the linked app's for every lane file, including lane #1 and the M0 fixtures.
- **`prepare` is split** so the pieces can be tested without a keystore or a network:
  - `Keys::from_keystore` reads the keystore;
  - `addresses()` gives the token, its asset and the settlement address, derived or pinned;
  - `desired()` builds the plan's `Desired`.

  Lane #1 now has a test that runs these on its lane file, against a matching chain and host, and plans "No changes.".
- **`prepare` takes a state root:** a local host's processes, and an ssh lane's exit file, live under `<state_root>/.caravel/`. The old commands pass the current directory, as before.
- **Where users reach the lane:** `api_url()` and `validator_url()`. For an ssh host that is its `public_url` (`/validators/<n>` for validators), so `status --json` no longer reports 127.0.0.1 for lane #1 | A CLI that runs any template can't link them all, and the pure pieces are what later tasks (the language, the graph) change | A template needs more than genesis and decimals at plan time |
| DEC-075 | **M0.6 (C-03).** `caravel` is one CLI, in `platform/crates/caravel-cli`, for any template. It replaces the dispatcher that exec'd `caravel-<t>-node` (`caravel-deploy/src/bin/caravel.rs`, deleted).
- **Which lane file:**
  1. `-f` (or a positional lane file, as before);
  2. `CARAVEL_FILE`;
  3. `./lane.toml`;
  4. the one `lane*.toml` here (several are an error that lists them);
  5. the nearest `lane.toml` above, stopping at a directory with `.git` or at `$HOME`.
- **Which deployment:**
  1. `--env`;
  2. `CARAVEL_ENV`;
  3. the one with `default = true` (a new, non-consensus key in `[env.<name>]`; at most one);
  4. the only one.

  Nothing is remembered between runs: an apply can only land on a deployment the command line names or the file marks.
- **Which template:** `Plugin::locate` (DEC-074). Stderr names the file and how the deployment was chosen.
- **Local state** is `.caravel/` next to the lane file. Older deployments with state under the current directory are still found, with a note.
- **Commands:**
  - `plan [--diff] [--exit-code]`, `apply [-y]`, `status [--exit-code]`, `destroy`, all as before;
  - `validate`: offline. It checks the genesis through the template's binary, each deployment's rules, and the identities in the keystore;
  - `env list`;
  - `output [NAME]`: the addresses and URLs derived from the lane file and its keys, with no network;
  - `version`: this CLI, every template binary found, and the Stellar CLI against its pin;
  - `doctor`: the Stellar CLI, Node.js 22+, the template, genesis, the release and its engine, the rules, the identities, and Docker for local networks.
- **Output and exit codes:** `--json` works on every command: one document on stdout, and errors as `{"ok":false,"error":…}`. Exit codes are 0 ok, 1 error, 2 usage, and 3 when `--exit-code` finds changes.
- **The deploy library's progress (`→ …`) and prompts now go to stderr.** Stdout carries only results.
- **The template binaries' own `plan`, `apply`, `status` and `destroy` are hidden.** They still work for older scripts | A quickstart needs one installed binary with discoverable defaults, not a path into `target/` and two required flags | Several lane files per directory become common, or a hidden "selected env" is asked for |
| DEC-076 | **M0.6 (C-04).** Caravel installs from source, and the release travels with the binary.
- **`scripts/assemble-release.sh <out>`** is shared by CI's release job and the installer. It makes the release layout `apply` installs on hosts:
  - `bin/caravel` and `bin/caravel-<t>-node`;
  - `contracts/*.wasm`;
  - `relayer/` with production `node_modules`;
  - `relayer-feeds/<t>/`;
  - `web/<t>/`;
  - `COMMIT` and `SHA256SUMS`.

  `COMMIT` is the git sha of a clean tree (also passed to the build as `CARAVEL_COMMIT`), otherwise `local-` and the start of the hash of `SHA256SUMS`.
- **`scripts/install.sh`** checks the tools (cargo, Node 22+, npm, and the pinned Stellar CLI), builds everything, and installs:
  - `$PREFIX/share/caravel/<release>/` with a `current` link;
  - the binaries in `$PREFIX/bin`;
  - `stellar-caravel` as a link.

  `PREFIX` defaults to `$CARAVEL_HOME`, else `~/.caravel`. Options: `--templates`, `--with-web`, `--wasm-dir` (take the record Wasm from CI) and `--skip-build`. It never edits shell files; it prints the PATH line instead.
- **`Release::locate`** looks for a release in this order: `--release-dir` (or `CARAVEL_RELEASE_DIR`), then `<exe>/../share/caravel/current`, then the checkout's builds. So an installed `caravel` needs no checkout and no `--release-dir`.
- **A web app per template:** `web/<t>/`. Releases from before M0.6 have the perps app at `web/`, which only perps lanes take. This fixes a payments lane on an ssh host serving the perps web app.
- **A platform guard.** The release's node binary is read as ELF or Mach-O (the `uname -sm` form), and an ssh host's read reports `uname -sm`. A release that would be installed on another platform is the plan problem `WrongPlatform`, so a macOS build never reaches lane #1's Linux VM.
- **CI** builds `caravel-cli` too, assembles with the script, and runs the installer over its builds | The quickstart needs one install step and no checkout paths, and the VM must never get a binary for another platform | Public prebuilt releases (needs the human: an outward-facing publish) |
| DEC-077 | **M0.6 (C-05).** `caravel init` and `caravel keys`, and local applies create the identities they name.
- **`caravel init [TEMPLATE] [DIR]`** writes `DIR/lane.toml` from the template's `plugin example`.
  - The template is the only one installed, unless named.
  - The lane's name comes from `DIR` (lowercase letters, digits and dashes, at most 48 characters), unless `--name` is given.
  - It creates every identity the example names, `<prefix>-<role>`, where the prefix defaults to the lane's name. Existing identities are reused, never replaced.
  - The sequencer gets the first free port from 18080, unless `--port` is given.
  - Before writing, it checks the file: it must parse, give a genesis through the template's binary, and pass every deployment's rules.
  - It adds `.caravel/` to `DIR/.gitignore` once. `--force` replaces an existing `lane.toml`.
  - The scaffolds mark `[env.local]` as `default = true`.
- **`caravel keys list | ensure | show <who>`**: each role's identity (admin, relayer, `validator-<name>`, `feed <VAR>`), whether the keystore has it, and its key. `ensure` creates the missing ones.
- **On a local network,** `apply` creates the identities the deployment names, and `plan` lists them first, because every address derives from the admin's key. **On testnet** both refuse and point to `caravel keys ensure`. Funding stays `apply`'s friendbot step.
- **Keys stay in the Stellar CLI's keystore** (`stellar keys generate`, CLI 28.1.0, `docs/SOURCES.md`). No secret is written anywhere else | The quickstart's `for … stellar keys generate` loop goes, and nothing beyond the keystore holds a key | Identities in a hardware wallet or the OS secure store for testnet admins (`--secure-store`) |
| DEC-078 | **M0.6 (C-06).** The lane file language starts: `platform/crates/caravel-lanefile`, pure TOML with no Stellar and no network, on toml 1.1.6's `DeTable` for spans (no new dependency, never `preserve_order`).
- **`include = ["envs.toml", …]`** (top level, before any table) loads more deployment tables, relative to the including file.
  - An included file holds only `[env.*]` and its own `include`. A genesis section there is an error that points at it.
  - Each name is defined once across all files, and a cycle is an error.
- **`extends = "base"` or `["a", "b"]`** in an `[env.<name>]`. Parents apply in order, then the deployment's own values.
  - Tables merge key by key. Anything else replaces what it inherits: a value, an array, an array of tables such as `validators`.
  - `abstract = true` marks a deployment that can only be extended: it isn't listed, planned or chosen, and it can't be the default.
  - `default` isn't inherited.
  - An unknown parent gets a did-you-mean. A cycle names its chain.
- **Origins.** Each value of a merged deployment keeps where it came from (file, line, column, and the deployment it was inherited from), for later errors on resolved values.
- **Errors** are all collected and printed with `file:line:col`, the line, carets, notes and help.
- **`vars`, `locals` and `outputs`** are reserved and refused for now, until the expression layer (C-07, C-08).
- **The node's parser** (`LaneFile::from_table`/`parse`) sets aside every deployment key (`env`, `include`, `vars`, `locals`, `outputs`). It refuses any `${` outside `[env]`: genesis sections are consensus config and stay literal. No template may be named after a deployment key.
- **Who uses the loader:** `caravel_deploy::manifest::load_lane` and `Manifest::load`/`parse` read lane files through it, and so does the CLI. Every existing lane file loads to the same tables and genesis hashes (tested) | The quickstart's lane files repeat a deployment per environment, and the e2e builds one with a heredoc | A deployment needs to drop an inherited key (comes with expressions: a value of `${null}`) |
| DEC-079 | **M0.6 (C-07).** The lane file's expressions: `caravel_lanefile::expr`. A hand-written parser and evaluator, with no dependency.
- **Strings:** a string that is exactly one `${…}` takes the expression's type (`threshold = "${var.n}"` is a number). Any other string with `${…}` in it is a string. `$${` writes `${`. Strings inside an expression interpolate too, and see comprehension variables (`[for v in xs : 'acme-v${v}']`).
- **Values:** null, booleans, 64-bit integers, strings, lists and maps. TOML floats and dates pass through but can't be computed with.
  - Arithmetic is checked: overflow and division by zero are errors, and `/` truncates.
  - `+` adds numbers only; strings join through templates or `format()`.
  - `==` compares values of one kind, or anything against `null`.
  - `<` and the other orderings compare two numbers or two strings.
- **Grammar:** `?:`, `||`, `&&` (both short-circuit), comparisons, `+ -`, `* / %`, unary `! -`, `.name`, `[index]` and calls.
  - Literals: lists, maps (`{ k = v }`), and comprehensions (`[for k, v in m : e if c]`).
  - Names may contain `-` (`node.validator-1`), so subtraction needs spaces.
  - Nesting is capped at 64, operator chains at 256 and lists at 10,000. A property test checks that no input panics.
- **Functions:** `range length concat merge lookup keys contains join split replace upper lower tostring tonumber min max coalesce format`.
  - No function reads the clock, randomness, the environment or files, so a lane file and its inputs always give the same deployment.
  - `file()` and hashing functions are left for when a task needs them.
- **`Value::Deferred(refs)`:** a value known only after Stellar and the host are read, such as `contract.settlement.address`. Anything computed from it is deferred with the references it needs, for the attributes stage (C-10).
- **Errors** carry the byte range in the string and a did-you-mean for names, fields and functions. Typos are measured as optimal string alignment, so a swap of two letters counts once | A lane file needs computed values (thresholds, names, lists of validators) without a full programming language or any I/O | Real need for floats, file reads or hashing in deployments |

Agents append new decisions here as `DEC-018+` with the same columns.

---

## 23. Open questions for the human

| ID | Question | Default if unanswered |
|---|---|---|
| OQ-001 | Who operates the 3 validators for the demo? (ideally 3 different people or orgs) | **Answered 2026-09-29 (Gate 3):** the team runs the sequencer and all 3 validators on one machine (see OQ-003), and the UI and docs say exactly that |
| OQ-002 | Oracle source for testnet (Reflector feeds available for BTC/ETH/XLM?) | Reflector if available, else a public spot API, signed by the team's oracle key; UI says so |
| OQ-003 | Hosting for the sequencer, validators and web | **Answered 2026-09-29:** Google Cloud for everything, kept cheap. **Chosen at Gate 3:** one e2-small VM in us-central1 (about $12 a month) running the sequencer, the 3 validators, the relayer and the web app |
| OQ-004 | Demo timing for freeze/escape (needs short `escape_timeout_secs`) | Separate demo instance with 20-minute timeout; main instance 6 hours |
| OQ-005 | Should the landing-page waitlist link to the testnet app? | No until T-013 is done |
| OQ-006 | License for the repo | `MIT OR Apache-2.0` |
| OQ-007 | Should the deck be updated to match §2.3 (ed25519 now, BLS later; no stellar-core close-time claim)? | Yes, before any public pitch |
| OQ-008 | Open access invites slot squatting (1,024 accounts × 1 USDC of free testnet USDC). Use an allowlist for the public demo? | Open mode with `min_deposit` 10 USDC; switch the demo instance to allowlist if squatted |

---

## 24. Security checklist (T-016)

Walked on 2026-09-29; evidence, fixes and filed items per line in `docs/SECURITY.md`.

- [x] Every settlement check in §13.3 has a negative test.
- [x] `claim_withdrawal` and `escape_claim` only pay the owner's `G...` address (§13.5 check 1).
- [x] No path lets the relayer or sequencer alter a signed header or its batch.
- [x] Validators persist `(seq, header_hash)` before returning a signature, and refuse to sign a different header for a signed seq.
- [x] Inbox accumulator matches between contract, engine and relayer (vector test).
- [x] Integer overflow: every `checked_*` failure is a rejection or `Fatal`, never a wrap.
- [x] The engine rejects oracle keys not in config; the sequencer never includes an unverified signature.
- [x] Session keys cannot withdraw or manage keys; expiry ≤ 7 days.
- [x] Engine Wasm hash checked at node startup against config and on-chain `engine_wasm_hash` (validators with `rpc_url` since T-016, DEC-050).
- [x] Admin functions emit events and are listed in the UI `/about` page as testnet powers.
- [x] TTL extension on all persistent entries the contract reads.
- [x] Freeze drill executed on testnet; replay OK after freeze (on a throwaway contract with the Wasm of record, 2026-09-29, `docs/RESULTS.md`).
- [x] Secrets only via env/files outside git; `.gitignore` covers `*.key`, `.env*`, `*.sqlite`.
- [x] Dependency audit (`cargo audit`, `npm audit`); soroban-sdk ≥ patched versions for known CVEs. Checked 2026-09-29: `cargo audit` finds no vulnerability; the soroban-sdk advisories (GHSA-x2hw-px52-wp4m, GHSA-4chv-4c6w-w254, GHSA-96xm-fv9w-pf3f) end at 25.x, so 28.0.0 is not affected; `stellar-xdr` GHSA-3xhm-p452-9wjh (serde only) is not reachable and filed (DEC-050).

---

## 25. Glossary

| Term | Meaning |
|---|---|
| Lane | A Caravel appchain with its own settings, settling to Stellar |
| Caravel Perps | The first lane: USDC-margined perpetual futures with an order book |
| Sequencer | The node that orders lane transactions into blocks |
| Validator | A node that re-executes every block and co-signs checkpoints |
| Checkpoint | A signed header + batch posted to the settlement contract |
| Batch | All lane blocks since the previous checkpoint (`BatchV1`) |
| Inbox | Stellar → lane message queue kept by the settlement contract |
| Soft confirmation | A fill included in a lane block (≈ 1 s) |
| Hard settlement | The checkpoint containing it is accepted on Stellar (≈ checkpoint interval + one ledger) |
| Freeze | Terminal mode of the settlement contract after a timeout; enables escape claims |
| Escape claim | Withdrawal of last checkpointed equity after freeze |
| Backstop | System account 0; receives liquidated positions and part of fees (insurance fund) |
| Treasury | System account 1; receives the rest of the fees |
| Lot / tick | Integer size unit / minimum price step, per market |
| Witness transaction | A real Stellar transaction invoking the engine's `step`, proving the call is valid on-chain |

---

## 26. Sources

Prior art:
- SoroDOOM: https://github.com/kalepail/sorodoom. Read `README.md`, `AGENTS.md`, `docs/ARCHITECTURE.md`, `DETERMINISM.md`, `REPLAY-FORMAT.md`, `SETTLEMENT-OPTIONS.md`, `FEES-AND-METERING.md`, `STELLAR-STORAGE.md`, `SECURITY.md`, `DECISIONS.md`, `apps/host-runner/src/main.rs`, `LICENSES.md`.
- Soroflare: https://github.com/stellar/soroflare
- Starlight: https://github.com/stellar-deprecated/starlight · https://www.stellar.org/blog/developers/starlight-a-layer-2-payment-channel-protocol-for-stellar
- Axelar Stellar gateway: https://github.com/axelarnetwork/axelar-amplifier-stellar (`contracts/stellar-axelar-gateway/src/auth.rs`, `contract.rs`)
- Rollup collateral pool: https://github.com/rails-xyz/soroban-rollup-contract
- One-way channel / MPP: https://github.com/stellar-experimental/one-way-channel · https://github.com/stellar/stellar-mpp-sdk
- Perun Soroban channels: https://github.com/perun-network/perun-soroban-contract

Crypto and ZK:
- Soroban examples (`bls_signature`, `groth16_verifier`): https://github.com/stellar/soroban-examples
- Provable Asteroids with RISC Zero: https://github.com/kalepail/kalien
- https://github.com/NethermindEth/stellar-risc0-verifier · https://github.com/NethermindEth/rs-soroban-ultrahonk
- ZK on Stellar docs: https://developers.stellar.org/docs/build/apps/zk

Libraries:
- OpenZeppelin Stellar contracts: https://github.com/OpenZeppelin/stellar-contracts

Protocol and standards:
- CAP-70: https://github.com/stellar/stellar-protocol/blob/master/core/cap-0070.md
- CAP-88 (discussion): https://github.com/orgs/stellar/discussions/1998
- SEP-53: https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0053.md
- SEP-54: https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0054.md
- SEP-40 oracles: https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0040.md · Reflector: https://github.com/reflector-network/reflector-contract
- Protocol 28 upgrade guide: https://stellar.org/blog/developers/adapter-protocol-28-upgrade-guide
- soroban-sdk 28.0.0 release notes and migration guide: https://github.com/stellar/rs-soroban-sdk/releases/tag/v28.0.0 · https://docs.rs/soroban-sdk/28.0.0/soroban_sdk/_migrating/index.html
- soroban-env-host 28.0.2 API: https://docs.rs/soroban-env-host/28.0.2/soroban_env_host/struct.Host.html

Network and tooling:
- Testnet USDC (issuer and SAC): https://developers.stellar.org/docs/build/guides/basics/verify-trustlines
- Resource limits and fees: https://developers.stellar.org/docs/networks/resource-limits-fees
- Galexie: https://developers.stellar.org/docs/data/indexers/build-your-own/galexie
