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
- hosting, or a lane registry or console: they were cut from M0.5 (§20.3);
- that `caravel` makes machines: it deploys onto hosts that exist. Since M0.8 the repository ships OpenTofu modules that make a Google Cloud VM for a lane (`infra/opentofu/`, DEC-114), a separate step whose outputs the lane file reads;
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
4. POST `{header, batch}` to every validator's `/v1/sign`. Collect signatures until the threshold weight is reached, with a 10s timeout and retries. With `[sequencer] key`, each request is signed by the sequencer (DEC-095).
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
| `GET /v1/accounts/{G...}` | collateral, equity, free collateral, positions (with upnl, liq price), open orders, next_nonce, pending_nonce (the nonce after the account's queued transactions, DEC-085), session keys |
| `GET /v1/markets` | params + oracle price + funding |
| `GET /v1/markets/{id}/book?depth=50` | aggregated levels |
| `GET /v1/markets/{id}/trades?limit=100` | recent fills (from receipts), kept across restarts (DEC-103) |
| `GET /v1/markets/{id}/candles?interval=1m\|5m\|15m\|1h&limit=500` | candles of the oracle price (the mark), built by the node from its blocks (DEC-103) |
| `GET /v1/blocks/{height}` | record hex + decoded entries + receipts |
| `GET /v1/blocks/{height}/raw[?wait_ms=N]` | the `BlockRecordV1` bytes (`application/octet-stream`), read outside the core lock; with `wait_ms` (at most 5,000) a block not made yet is answered as soon as it is, else 404 (F-11) |
| `GET /v1/checkpoints/{seq}` | header (hex + decoded), batch hash, signatures, Stellar tx hash, status |
| `GET /v1/proofs/withdrawals?account=G...` | all unclaimed withdrawal leaves for the account: `{seq, index, amount, proof[]}` |
| `GET /v1/proofs/escape?account=G...` | leaf from the **last accepted** checkpoint: `{seq, index, equity, proof[]}` |
| `WS /v1/stream` | subscribe `{"blocks":true,"markets":[1,2,3],"tickers":true,"account":"G..."}` → messages `block`, `tickers` (every market's price line, each block, DEC-103), `fill`, `book`, `account`, `checkpoint` |
| `POST /internal/inbox` | relayer → sequencer: `{index, msg_hex, acc_after_hex}`; sequencer checks the acc chain matches its own fold, and on mismatch halts inbox inclusion and alerts |
| `POST /internal/oracle` | relayer → sequencer: `OracleUpdateV1` hex (pre-verified) |
| `GET /internal/checkpoints/pending` | relayer pulls `{seq, header, batch, epoch, sigs}` |
| `POST /internal/checkpoints/{seq}/accepted` | relayer reports `{stellar_tx_hash, ledger}` |

The sequencer must keep state history to serve proofs and replays: all blocks, every checkpoint header and its withdrawal leaves, and the state snapshot of genesis, of the last checkpoint accepted on Stellar, of the 3 before it and of every later one. It MAY prune older snapshots and the batch bytes of accepted checkpoints older than those, which are on Stellar and can be rebuilt from the blocks (DEC-105).

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
  - catch up by fetching `/v1/blocks/{h}/raw?wait_ms=2000` from its last height (F-11; `/v1/blocks/{h}` every `poll_ms` from a sequencer without the raw route).
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
  A validator configured with the sequencer's key (`sequencer_key`) first refuses, with 401 `UNSIGNED`, a request the sequencer didn't sign within the last 60 s (DEC-095).
- **Never sign two different headers for the same `seq`.** Persist every signed `(seq, header_hash)` before returning. This is what makes M1 equivocation slashing safe for honest validators.
- Stores a state snapshot at every checkpoint it computes, keeping at least genesis, the one last accepted on Stellar (polled from `last_checkpoint()`), the 3 before it and every later one; older snapshots and old accepted batch bytes may be pruned as for the sequencer (§14.4, DEC-105). It drops the blocks of accepted checkpoints up to its oldest kept snapshot (DEC-121): the sequencer keeps every block, archived, and Stellar keeps every checkpoint; a dropped height answers `410 PRUNED`.
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
  - The perps app's trading terminal (M0.7, DEC-101) uses its own tokens: Inter, OKLCH near-blacks, coral for soft and teal for settled as above, plus green and red for direction only.
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
- hosts are `local` or `ssh` (a host the team already has); the tool provisions no cloud machines (M0.8 adds OpenTofu modules for that, outside the tool, §20.7);
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

The plan of record is `~/.claude/plans/understand-this-project-and-zesty-zephyr.md`. Phase gates are G1 (C-05), G2 (C-10), G3 (C-14), G4 (C-17), G5 (C-21) and G6 (C-25). **Gate G1 passed on 2026-10-02**, with #49–#54 merged. Gates G2 to G5 passed on 2026-10-03 (#56–#72), and **Gate G6 on 2026-10-03**, with #73–#76 merged: M0.6 is done.

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
| C-06 | `caravel-lanefile`: loader with spans and diagnostics, `include`, `extends`, reserved keys; genesis refuses `${` (DEC-078) | C-05 | done |
| C-07 | The expression evaluator: grammar, types, functions, no time or randomness (DEC-079) | C-06 | done |
| C-08 | Vars, locals, `for_each`, per-env `[env.<name>.node]` (DEC-080) | C-07 | done |
| C-09 | The manifest on resolved values; `caravel render`; the e2e without heredoc or `sed` (DEC-081) | C-08 | done |
| C-10 | Attributes and outputs; `caravel output`; references in relayer feeds (DEC-082) — **Gate G2** | C-09 | done |
| C-11 | Lifecycle: `stop`, `start`, `restart`, `logs`, `replay` from the lane file, `wait`, `api` (DEC-083) | C-10 | done |
| C-12 | Users' Stellar flows: `account create/fund`, `balance`, `deposit` (waits for the credit) (DEC-084) | C-11 | done |
| C-13 | Lane transactions: `tx`, `withdraw`, `claim`, `force-withdraw`, `escape` (DEC-085) | C-12 | done |
| C-14 | The e2e on the CLI only; README, landing quickstart, RUNBOOK, `docs/LANE_FILE.md` — **Gate G3** (DEC-086) | C-13 | done |
| C-15 | The resource graph, with identical plans (goldens byte for byte) (DEC-087) | C-14 | done |
| C-16 | Addresses in plans; `plan --json`; `graph`; `depends_on`, `--target`, `--replace` (DEC-088) | C-15 | done |
| C-17 | Saved plans: `plan --out`, `apply <planfile>` refused when anything moved — **Gate G4** (DEC-089) | C-16 | done |
| C-18 | Accounts: funding, trustlines, balances topped up (DEC-090) | C-17 | done |
| C-19 | Tokens: issued assets and their contracts; a declared token as the settlement token (DEC-091) | C-18 | done |
| C-20 | Contracts: any Wasm, constructor arguments, derived addresses, `prevent_destroy` (DEC-092) | C-19 | done |
| C-21 | Local modules with inputs and outputs — **Gate G5** (DEC-093) | C-20 | done |
| C-22 | Several hosts per deployment and node placement (DEC-094) | C-21 | done |
| C-23 | Networking across hosts: signed `/v1/sign` requests, private or public addresses, validators run elsewhere (DEC-095) | C-22 | done |
| C-24 | Lane namespaces: several lanes on one host (DEC-096) | C-23 | done |
| C-25 | The web app as a resource, configured from outputs — **Gate G6** (DEC-097) | C-24 | done |

### 20.6 Milestone M0.7: ready for HackMeridian

**Decided with the human on 2026-10-03,** after the HackMeridian track discussion (Stellar Unlocked and Frankenstack). Caravel goes to HackMeridian as an optional building block that teams can pick up, with the human mentoring the teams that use it. This milestone does what the human told the organisers would be ready:
- a page that says who Caravel is for;
- a short path from nothing to a first lane, on prebuilt binaries;
- a lane that runs its own smart contract execution engine, so hackers can try Groundhog (on hold since 2026-10-03, OQ-010);
- a starter kit for Frankenstack teams.

The privacy stack is left out for now; the human will take it up separately.

**The first user,** from what the human saw after Istanbul, is a team whose app needs many fast actions and its own rules, with assets, wallets and exits on Stellar. Two cases:
- perps, where 5 s ledgers are too slow for an order book;
- just-in-time card payments from a non-custodial wallet, which block time also rules out.

**Rules that hold for every task:**
- M0.6's rules still apply. Lane #1 keeps "No changes." and its genesis hashes, and the frozen engine is untouched.
- Copy stays honest:
  - "testnet only, not audited" stays prominent;
  - validity proofs and bonded validators are plans, not features;
  - block time is configurable, so never "1 s blocks" as a property of Caravel (lane #1 runs 1 s);
  - nothing claims Groundhog or confidential-token support before it exists.

**Needs the human first (§0.4):**
- the landing page deploy;
- publishing releases;
- Groundhog's design, because a new execution engine touches consensus (DEC-002) and the trust model;
- anything sent to the HackMeridian organisers.

Branches are `h-0x-short-name`. Gates: H1 (H-01), H2 (H-04), H3 (H-06, the Groundhog design), H4 (H-08) and H5 (H-11).

| ID | Task | Depends | Status |
|---|---|---|---|
| H-01 | **The landing page tells it straight** — **Gate H1**. Add "Who it's for", with the perps and just-in-time card payment cases. Describe block time as configurable. Move validity proofs and bonded validators into a section clearly labelled planned. Keep "testnet only, not audited" in the hero and the trust section. Deploy only with the human's OK | — | review |
| H-02 | **Prebuilt binaries.** The CI release job builds `caravel` and each template's node binary for macOS (arm64) and Linux (x86_64, arm64), with the relayer, the web apps and the contracts of record, plus `SHA256SUMS`. A tag publishes them as a GitHub Release, the first public one, with the human's OK. Reuses `scripts/assemble-release.sh` and the platform guard (C-04) | H-01 | review |
| H-03 | **One-line install.** `curl -fsSL <raw>/scripts/install.sh \| sh` fetches the release for this platform, checks its checksum, and installs into `~/.caravel`. `--from-source` keeps today's path. Covers the Stellar CLI the CLI needs (an install hint, or a pinned download), and `caravel doctor` says what's missing | H-02 | review |
| H-04 | **The 15-minute path, measured** — **Gate H2**. On a clean macOS machine and a clean Linux one, take the time from nothing to a lane in use (install, `init`, `apply`, deposit, `tx`, withdraw) and record it in RESULTS. Fix what blocks it (first start of the local network, keys, Docker). README and landing quickstart on the release path. `check-quickstart.sh` runs it from a release in CI | H-03 | todo |
| H-05 | **Groundhog: find out.** What it is today (the Stanford parallel execution engine): status, interface, what contracts it runs, license, readiness for Meridian, and who mentors it (Tyler's note in the tracks doc). Record the facts with sources in `docs/SOURCES.md` and the open questions as an OQ. No code | H-01 | review |
| H-06 | **Groundhog: design** — **Gate H3**. A lane template that runs user-deployed contracts (deploy and invoke as lane transactions), with deposits, checkpoints and exits like any lane. Decide with the human:<br>- how Groundhog executes them, against DEC-002 (consensus runs the engine Wasm through `soroban-env-host`) and INV-P5 (native and Wasm match);<br>- what validators re-execute;<br>- what goes in a checkpoint;<br>- what the copy may claim | H-05 | on hold |
| H-07 | **A contracts lane template (MVP).** The engine from H-06's design on the app SDK. Users deploy and call contracts on the lane, and a token balance moves in and out through settlement as on Payments. Scenarios, vectors, a parity gate, `caravel init contracts`, and an e2e on the CLI | H-06 | on hold |
| H-08 | **Groundhog in the lane** — **Gate H4**. Groundhog as the lane's executor, as H-06 decided, with replay and validators matching it. Measured against the H-07 baseline, with numbers in RESULTS. If Groundhog isn't ready, the template ships without it and the copy says so | H-07 | on hold |
| H-09 | **The Frankenstack starter kit.** A lane file and a one-page guide for each recipe, built on the Payments or contracts template:<br>- a lane with x402 or agent payments (many small payments, settled in USDC);<br>- a lane with a passkey or smart wallet (payments that feel instant);<br>- a lane with an oracle (trading, as Caravel Perps does).<br>Each recipe runs end to end with `caravel` alone | H-04 | todo |
| H-10 | **The first outside run.** Someone outside the team goes from the README to a working lane and one recipe, with the human watching. Record the friction and fix what blocks. This is the first external run, and it happens before HackMeridian | H-09 | todo |
| H-11 | **Mentor kit** — **Gate H5**. A 5-minute lightning talk, a FAQ (what it is, who it's for, the tradeoffs, what isn't built yet), a troubleshooting page, and how mentors help a team pick it up. Nothing goes to the organisers without the human | H-10 | todo |
| H-12 | **The docs site** (DEC-100). Docusaurus in `docs-site/`, its own Vercel project. It has Getting started, Concepts, Guides and Reference. The lane-file reference moves there from `docs/LANE_FILE.md`, and the CLI reference is generated from `caravel help` and checked in CI. Local search, the landing page's palette, copy checks. Deploy only with the human's OK | H-01 | review |
| H-13 | **The perps trading terminal** (DEC-101). A full redesign of `lanes/perps/web` as a professional perps DEX: market bar, chart, book and trades, order ticket with Market and Post-only, positions with Close, first-run steps, soft and settled everywhere, stale-oracle and halted states, phone layout. Same API and signing code. Ships to lane #1 only with the human's OK | H-01 | review |
| H-14 | **The oracle feed never stops silently** (DEC-102). On 2026-10-03 the testnet oracle stopped at 00:59 UTC for about 19 hours while blocks and checkpoints went on: a feed tick waited forever on a Stellar RPC call. Give every RPC client a timeout and every feed tick a deadline, and add the symptom to the runbook. Ships to lane #1 with the human's OK | H-01 | review |
| H-15 | **The live terminal** (DEC-103). Prices on every block over the stream (`tickers`), candles of the oracle price from the lane's own blocks, trades kept across restarts, and the web app driven by the stream: candle chart with history, live last candle, trade highlights, a Live indicator. The relayer feed runs at a fixed rate with markets in parallel | H-13 | review |
| H-16 | **Lane #1 at 0.5 s** (DEC-104). Lane #1 makes a block every 500 ms and checkpoints every 120 blocks, so still once a minute; the oracle feed publishes every 500 ms. Node settings only: no new genesis. Ships with the human's OK | H-15 | review |
| H-17 | **Leaner stores** (DEC-105). An index for the per-block checkpoint queries, withdrawal proofs that skip the batch bytes, and pruning of old snapshots and accepted batches on every node, with a `compact` command to shrink a stopped node's file. Amends §14.4 and §15 on the human's call | H-16 | review |
| H-18 | **The rest of the perps app, and test USDC in one click** (DEC-106). Portfolio as an account page with a balance sheet and Get test USDC / Deposit / Withdraw; Explorer as a live lane view with the checkpoint pipeline and search; Exit (the old Escape) useful at all times, with the freeze timers and a downloadable escape proof; How it works (the old About) with a diagram and a trust table; a wallet menu | H-13 | review |
| H-19 | **Withdraw through to the claim** (DEC-107). A withdrawal is followed from the lane to the user's wallet: the app tracks it until its checkpoint is accepted, then puts a Claim button where the user is (top bar, Trade account box, Portfolio) | H-18 | review |


### 20.7 Milestone M0.8: containers and machines as code

**Decided with the human on 2026-10-04.** Lane #1 runs straight on one VM: the machine and its firewall were made by hand, and `caravel apply` installs a release tarball and systemd units over ssh. M0.8 puts two established tools under Caravel, each owning one layer, and keeps Caravel the layer on top:
1. **OpenTofu** owns the machines: VM, address, firewall, budget cap (`infra/opentofu/`). Caravel itself still provisions no machines (§2.4, §20.3).
2. **Containers** run the nodes, the same way on a laptop and on a VM: one image per node template, the relayer and each web app, built from the release (`docker/`, GHCR).
3. **Caravel** stays the lane layer: Stellar contracts, keys, signers, node configs, releases and plan/apply, now with a `docker` runtime beside systemd and plain processes.

The human's calls:
- images per release on GHCR, pinned by digest;
- Caddy in compose, so a host needs only Docker;
- OpenTofu state in a versioned GCS bucket;
- the rule against the other IaC tool's name exempts `infra/opentofu/` only, where the syntax needs it; copy, docs and the spec still never write it.

Branches are `d-0x-short-name`, stacked. Gates: after D-03 (lanes in Docker on a laptop) and after D-06 (lane #1 on Docker).

| ID | Task | Depends | Status |
|---|---|---|---|
| D-01 | **Images in the release** (DEC-111). `docker/{node,relayer,web}.Dockerfile` package an assembled release without rebuilding it; `scripts/build-images.sh` builds them for this machine, or pushes them for one or more architectures and joins them into one image. CI pushes `ghcr.io/<owner>/caravel-*:sha-<12>` on main and `:v*` (amd64 and arm64) on tags, and the release carries `IMAGES`. Base images pinned by digest in `versions.json` | M0.7 | done |
| D-02 | **A `docker` runtime in caravel-deploy** (DEC-112). `runtime = "process" \| "systemd" \| "docker"` per host; a rendered compose file per host with the systemd units' hardening; the plan, fingerprints and restarts as for the other runtimes | D-01 | done |
| D-03 | **Lanes in Docker on a laptop** — **Gate**. `caravel init` lanes run in containers when the release ships images; Docker and the `caravel` CLI suffice, not Node.js; the Stellar quickstart image pinned; e2e in both runtimes (DEC-113) | D-02 | done |
| D-04 | **OpenTofu modules for GCP** (DEC-114). A host module (VM, address, firewall, IAP SSH, a startup script that installs Docker), a billing-cap module, lane #1's root module with a GCS backend; outputs feed `caravel --var-file` | D-01 | done |
| D-05 | **Lane #1's project, imported.** Every existing resource imported; `tofu plan` says no changes; nothing recreated | D-04 | done |
| D-06 | **Lane #1 on Docker** — **Gate**. With the human's OK: Docker on the VM, `runtime = "docker"`, apply, verify, rollback ready | D-03, D-05 | done |
| D-07 | **Spec and docs.** Guides (lanes in Docker, machines with OpenTofu), RUNBOOK §3, CLAUDE.md, SOURCES | D-06 | done |
| D-08 | **A Linux builder anywhere** (DEC-116). `scripts/build-linux-release.sh` builds the Rust binaries in the pinned Rust image for Docker's architecture and assembles a release from this checkout, so a Mac builds images and runs the docker e2e with nothing downloaded | D-07 | done |
| D-09 | **Lane #1's VM cleaned up.** With the human's OK: the disabled systemd units and the host's Caddy and Node.js removed; the rollback is now a reinstall (RUNBOOK §3.4). **Done 2026-10-04:** the three unit files, the `caddy` package with its apt source and key, Node.js from `/usr/local`, the old release files in `/opt/caravel` (`bin`, `contracts`, `relayer`, `relayer-feeds`, `web`, `staging`, `.npm`, about 190 MB) and the `caravel` user; `/opt/caravel` keeps `COMMIT`, `caddy`, `config`, `data`, `keys`, `run`; the lane kept checkpointing and `caravel plan` said No changes | D-06 | done |
| D-10 | **A release with images.** With the human's OK: a `v*` tag, multi-arch images on GHCR, the draft checked and published, `install.sh` users get the docker runtime. **Done 2026-10-04:** `v0.2.0` on main `5d9ffbd` (CI green, both runtimes); archives for x86_64 and arm64 Linux and arm64 macOS; `caravel-{perps,payments}-node`, `caravel-relayer` and `caravel-perps-web` as one image each for amd64 and arm64, public on GHCR; published as latest. Checked on macOS from the one-line install: `IMAGES` installed, `caravel init` chose `runtime = "docker"`, and `caravel apply` brought a payments lane up from the published digests (checkpoint 0 accepted, five nodes up) | D-08 | done |
| D-11 | **CI runs only what a change touches** (DEC-117). A `changes` job sorts the diff into code, npm apps and OpenTofu; docs-only changes skip the Rust build, e2e, quickstart and release; `[skip ci]` documented | D-10 | done |
| D-12 | **The README, reorganized.** A header image in both themes (rendered from `docs/assets/readme-header.html`), a centered intro, a diagram of how the layers fit, Docker-only requirements, lane #1 at 0.5 s in containers, the details in collapsible blocks; the quickstart block unchanged | D-11 | done |
| D-13 | **The landing page after M0.8.** A "Where it runs" band (the lane, containers, machines) with lane #1 live through a same-origin rewrite (`/lane/status`), Docker as the one requirement, the docs and site redeployed | D-12 | done |
| D-14 | **The landing page as a pitch.** The hero says who it's for ("For Stellar apps that need their own rules", with example apps and the lane-file lines that set their rules) and shows lane #1 live; then why a lane (Stellar's shared rules next to a lane's own; the money stays on Stellar), how it works, every change gets a plan, the exit guarantees and what they don't prevent, where it runs, where it stands (measured, not yet, open source), and the five-minute start. `docs/PRODUCT.md` holds the brand context | D-13 | done |
| D-15 | **The installer sets up PATH** (DEC-118). Like rustup: `$PREFIX/env` sourced from the shell's startup files, `--no-modify-path` to opt out; the docs drop the manual `export` | D-14 | done |

---

### 20.8 Milestone M0.9: a performance cycle

**Decided with the human on 2026-10-04.** The aim is to measure, then fix what doesn't touch consensus: storage, disk I/O, throughput, latency and CI.

What exploration and a live read of lane #1 found:
- **Storage:** each of the four stores (sequencer and 3 validators, one 20 GB disk) keeps every block since genesis. That was 432,883 rows on 2026-10-04, ~280 B each, ~121 MB per store. Pruning (DEC-105) trims only snapshots and batches, and the files never shrink. Growth is ~190 MB/day at 500 ms and ~485 MB/day at 200 ms.
- **Disk I/O:** every node rewrites the full state blob and fsyncs on every block.
- **Throughput:** sustained TPS is bounded by the serial relayer, ~60–90 orders/s at any block time.
- **Latency:** about 2 s of hard latency is a signer retry sleep.
- **CI:** a code PR took ~10 min.

The human's calls:
- validators prune old blocks while the sequencer keeps full history, compressed;
- consensus-level ideas come back as a costed list, not in this cycle;
- 200 ms on lane #1 only after the fixes, if the measurements allow;
- state is persisted at checkpoints and every N blocks, with blocks replayed on restart. Full fsync stays for validator signatures.

Branches are `f-0x-short-name`. Gates: F-04, F-09 and F-14.

| ID | Task | Depends | Status |
|---|---|---|---|
| F-01 | **Per-phase metrics.** Lock wait, build, execute, decode, commit, seal, sign-collect and fan-out timings (p50/p99), in `/v1/status` and `caravel status --json`. Node-side only (`caravel-runtime::perf`, a 1,024-sample window per phase); validators also report fetch, checkpoint and sign, the sequencer seal-to-signed | M0.8 | review |
| F-02 | **Load tooling.** `loadgen` past 255 accounts (`--accounts` u16, hashed seeds past 255) with `--inflight` requests open per account (a nonce refused for room is sent again, so no gap stalls the account) and `--csv`; `scripts/soak-lane.sh`, a local full-stack soak (sequencer, 3 validators, relayer, accounts that deposit through Stellar) at any `BLOCK_MS`, reporting every node's phases, CPU and memory, store bytes per block and per day, latency and rate | F-01 | review |
| F-03 | **Baseline report.** `docs/RESULTS.md` "Performance baseline": six local soaks (500 and 200 ms; idle, 45 and 100 tx/s), lane #1's stores, and testnet's limits in `docs/SOURCES.md`. Storage is the first limit (~325 B a transaction in each store), every signature waits 2 s, block time is not a throughput limit locally, and on testnet the serial relayer caps sustained load near 85 to 90 tx/s | F-02 | review |
| F-04 | **CI, fast path** (DEC-119) — **Gate.** Both workspaces cached; no disk cleanup; four parallel Rust jobs; parity gates concurrent and both gates threaded; nextest; main-only cache saves; release built once for the e2e; no cancelled main runs | M0.8 | review |
| F-05 | **Logs and small files.** Rendered containers log through Docker's `local` driver, 3 × 10 MB; the process runtime rotates `logs/<node>.log` past 10 MB as a node starts (3 kept); colors only on a terminal; the relayer's checkpoint jsonl moves to `.1` past 10 MB. The prune log already says nothing when nothing was pruned. Measured: a node logs ~6 lines per checkpoint, ~2 MB a day at one a minute | F-04 | review |
| F-06 | **SQLite housekeeping.** New stores are `auto_vacuum=INCREMENTAL` and every prune pass hands the free pages back (pruned rows, and the head row rewritten every block), so files shrink while nodes run; `compact`'s VACUUM converts an older store. The WAL is truncated to 4 MiB after SQLite checkpoints it. Per-block statements are cached. The block loop counts signed checkpoints and the signer and `/internal/pending` read the first one with `LIMIT 1`, instead of loading every waiting batch. Cleared flags are dropped as the store prunes | F-05 | review |
| F-07 | **Lazy state persistence** (DEC-120). The head is written at checkpoint ends and every ~10 s of blocks; on open, the blocks after it are re-executed and checked. Validators commit blocks with `synchronous=NORMAL` and write signatures with FULL; the sequencer keeps FULL | F-06 | review |
| F-08 | **Block history** (DEC-121). Validators drop the blocks of accepted checkpoints up to their oldest kept snapshot; the sequencer archives them as one deflated blob per checkpoint and serves them as before (spec §15 and DEC-105 change) | F-07 | review |
| F-09 | **Checkpoint cadence by time** — **Gate.** Lane files keep about a checkpoint a minute at any block time: the lane-file reference says how (`checkpoint_every_blocks = 60000 / block_time_ms`), and `caravel validate` notes a deployment off the local network that would checkpoint more often than every 30 s. Lane #1's storage measured on the new code once the human approves its deploy | F-08 | doing |
| F-10 | **Signing without the 2 s stall.** After a failed round the signer retries in 150 ms, doubling to 2 s (it slept 2 s every time); `/v1/sign` waits up to 1.5 s for the follower to reach the checkpoint before answering `NOT_CAUGHT_UP`, and that refusal is logged at debug. Measured: `seal_to_signed` was 2.01 s p50 on the soak; the API test's checkpoint now signs in well under 1.5 s (asserted) | F-09 | review |
| F-11 | **Validator fetch path.** `/v1/blocks/{h}/raw` serves the record bytes from a read-only SQLite connection, outside the core lock, instead of the JSON view a validator decoded only for `record_hex`; `?wait_ms=` holds the request on a height watch until the block exists, so a validator gets each block when it is committed instead of up to a poll later. An older sequencer without the route is followed as before | F-10 | review |
| F-12 | **Locks and async hygiene.** Every request and loop that takes the core lock (a std mutex held for a whole block) runs on the blocking pool (`on_pool`), never on an async worker: `POST /v1/tx`, `/v1/status`, the account's pending nonce, blocks, checkpoints, proofs, the internal inbox, feed and pending routes, and the signer loop. `publish` (the app's views, and the perps history's minute save) runs in the block task after the lock is released. The mempool keeps each account's queued nonces, so admission and `next_queued_nonce` no longer scan the queue. Splitting the core lock was planned and is not done: the soak measured the block loop's `lock_wait` at 33 µs max at 45 tx/s | F-11 | review |
| F-13 | **WebSocket fan-out once per block.** `publish` builds a `BlockFrame` per block: the block input decoded once, the `block` message serialized once, and each user transaction's `receipt` message (events rendered, hash computed) once. A subscriber's work is filtering receipts by its account and the app's own per-subscriber views; before, every socket decoded the block, rendered every event and hashed every transaction | F-12 | review |
| F-14 | **Relayer pipeline** — **Gate.** The relayer asks for the next signed checkpoint with `?wait_ms=5000` and the sequencer holds the request until the signer stores one (a watch on the last signed seq), instead of a 2 s poll. The checkpoint right after one this run saw accepted goes out without reading `last_checkpoint()` first (the contract refuses any other seq, so a surprise fails the simulation; any error brings the read back). The relayer's account is kept between submissions (building a transaction advances its sequence; any failure drops it), and inclusion is polled every 250 ms instead of every second. **Not done:** two checkpoints in flight. Simulating seq N+1 fails until N is applied, because the contract checks `seq = last + 1` and simulation cannot see a pending transaction; doing it would mean hand-built footprints. It goes on the out-of-cycle list with the bigger batch. The native `opt-level=3` speedup was not tried. GETs to the sequencer are sent again once if the connection drops. Re-run (`docs/RESULTS.md` "After the M0.9 fixes"): hard latency about 2.5 s lower at every load, seal to signed 9 to 15 ms instead of 2.0 s, validator stores flat, the sequencer's 2.4× smaller. The 200 ms decision for lane #1 waits for the human and a CPU measurement on the VM | F-13 | review |

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
| DEC-043 | The local end-to-end (`scripts/e2e-local.sh`) runs quickstart in Docker through `stellar container start local --limits testnet` (image `stellar/quickstart:latest`, protocol 28, 1 s ledgers, testnet resource limits), and the sequencer, 3 validators and the relayer as local release processes, not a compose file. It uses its own Stellar CLI config directory, a local USDC asset contract minted by a local issuer, the fixture oracle key of the local lane, and a settlement contract with `escape_timeout_secs = 30` and `force_inclusion_window_secs = 20`, because quickstart cannot move ledger time. `caravel-node tx` signs and submits lane transactions from an `S...` key file | One command from a clean clone, with nothing to build into images. Container images for hosting come with T-012. A short timeout stands in for "move ledger time" in §19.5 step 5 | Hosting images (T-012) **Revisited in M0.8:** images per release (DEC-111), the quickstart image pinned and the e2e in both runtimes (DEC-113). |
| DEC-044 | **Decided at Gate 3 (2026-09-29).** The testnet lane checkpoints every 60 blocks (one a minute at 1 s blocks) instead of 10. `checkpoint_every_blocks` is a node setting, so the genesis hashes do not change; rules (b) and (c) of §14.2 still end a batch early when it fills | A small checkpoint cost 0.067 XLM on the local network with testnet limits (T-011). Every 10 blocks that would be about 8,600 transactions and 580 XLM a day; every 60 it is about 1,440 and 96 XLM. Withdrawals wait up to a minute longer to become claimable | Measured testnet fees (T-014) |
| DEC-045 | **Decided at Gate 3 (2026-09-29).** Hosting is the Google Cloud project `caravel-testnet` ("Caravel"), billed to the user's "My Billing Account 1" (BRL). A budget of R$100 a month for this project alerts at 50, 90 and 100%, and at 100% a Cloud Run function (`infra/gcp/billing-cap`, set up by `infra/gcp/setup-billing-cap.sh`) removes the project's billing account, which stops everything in it. The function's service account holds only Project Billing Manager and Browser on this project, not Billing Account Administrator as Google's guide suggests. Both paths were tested on 2026-09-29: below the budget it does nothing; above it billing was off within about 20 s, then relinked | The user asked for a spending limit; a budget alone only alerts. Google's caveats apply: notifications lag real costs, so the cap is not exact, and resources left without billing can be deleted | The budget amount, if the VM size changes |
| DEC-046 | Testnet hosting layout: VM `caravel-1` (e2-small, us-central1-a, Ubuntu 24.04, 20 GB standard disk, no service account, shielded boot) with static IP `35.224.76.64`, served as `35-224-76-64.sslip.io` with a free Let's Encrypt certificate from Caddy. SSH comes only through IAP (default SSH and RDP rules removed); ports 80 and 443 are open to the VM's tag. systemd runs the sequencer, `caravel-validator@1..3` and the relayer as the unprivileged `caravel` user with a read-only system (`ProtectSystem=strict`, writes only to `/opt/caravel/data`). Caddy exposes `/v1/*` (sequencer), `/validators/N/*` and the web app, never `/internal/*` or the validators' `/v1/sign`. Binaries and Wasm come from the CI `release` job (ubuntu-24.04, DEC-033) via `scripts/deploy-vm.sh`; secrets are copied once from the local Stellar keystore to `/opt/caravel/keys` (mode 600) | One cheap machine, as chosen at Gate 3; sslip.io gives TLS without buying a domain; building on the VM would be slow on 0.5 vCPU | A second machine or a domain **Revisited in M0.8:** the machine is described in OpenTofu (DEC-114) and the nodes and Caddy run as containers (DEC-115). |
| DEC-047 | Web app choices (T-013):
- a small pathname router instead of react-router, for five routes;
- the `buffer` package installed as `globalThis.Buffer` for the SDK and Freighter;
- the session key in IndexedDB, 24 h, `PERM_TRADE | PERM_CANCEL`;
- trades and cancels signed by that key when it exists, everything else through Freighter's SEP-53 `signMessage`;
- the price chart sampled in the browser from `/v1/markets` (the lane keeps no price history; superseded by DEC-103: candles from the node, prices on the stream);
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
- **Cut:** `--preflight`. The human judged check-store and the shadow validator unnecessary for releases that don't change consensus. `import`: lane #1 is the only lane deployed before the tool, so its `[env.testnet]` is written by hand from its chain state and deploy files | A team's own Linux host is the self-hosting case. Reusing lane #1's exact units keeps the first import to config files | Several lanes per host, or hosts without systemd. **Revisited:** namespaces (C-22) give several lanes per host, and the `docker` runtime (DEC-112) needs only Docker on the host, no systemd units, Node.js or host Caddy |
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
| DEC-080 | **M0.6 (C-08).** Vars, locals, `for_each` and per-deployment `[node]`.
- **`[vars.<name>]`** declares a var:
  - `type`: string, integer, boolean, list, map or any;
  - `default`, which is literal, since vars are inputs;
  - `description`;
  - `sensitive`: kept out of messages and masked in the plan;
  - `validation = [{ condition = "${…}", message = "…" }]`.

  A var's value comes from its default, then `CARAVEL_VAR_<name>`, then each `--var-file` (TOML, literal), then each `--var name=value`, the last one winning. A `--var` is read by the var's type: a string as written, anything else as a TOML value. A value for an undeclared var is an error with a did-you-mean. A var with no value is an error only in a deployment that uses it.
- **`[locals]`** are named values. They are computed per deployment, in the order they need each other, from `var`, `lane` (`name`, `template`), `env` (`name`) and other locals. A cycle is an error.
- **A table with `for_each`** becomes a list of tables, one per item of a list or map, with `each.key` and `each.value` in scope. That holds both for a key (`[env.x.validators]`) and for an element of an array of tables. It allows up to 1,024 instances, and `for_each` must be known before anything is read.
- **A value of `${null}`** leaves its key out, so a deployment can drop what it inherits.
- **Vars and locals** may be in included files. Each name is defined once.
- **`[env.<name>.node]`** merges over the lane file's `[node]` for that deployment. It reaches the hosts' `lane.toml` and never genesis, because `[node]` isn't consensus (tested).
- **`Manifest::load_with(path, env, inputs)`** resolves the one deployment a command needs. The CLI has `--var` and `--var-file`, and `CARAVEL_VAR_*` is read. `prepare` takes the inputs.
- **Secrets** are refused where they are written (vars, locals) and where they land, including through `--var`. Mainnet is refused on the resolved `network`.
- **The plan's header** shows `vars: …` only when the file declares vars, so lane #1's plan text is unchanged | A rotation, a second network or another validator count is a `--var`, not an edited or duplicated file | Remote var sources (a secrets store), or per-deployment var defaults |
| DEC-081 | **M0.6 (C-09).** Deployments are read resolved, and problems point at the lane file.
- **Locations.** A broken rule in a deployment now ends with `at <file>:<line>:<col>`, and an inherited value adds `(from [env.<base>])`. The location is found through the language's origins, by the field the message names: its leading path, serde's "in \`a.b\`" or "unknown field \`x\`", or `validator "<name>"`. A shorter path is used when the full one has no origin.
- **`caravel render [--genesis]`** prints the deployment as `plan` reads it: includes, inheritance, vars and expressions resolved, with `# vars: …`. With `--genesis` it prints exactly the document the template hashes and the hosts get as `lane.toml` (`--json` too). That is the input to check before an apply, and the right `replay --genesis-config`.
- **The e2e composes its deployment** instead of generating it:
  - `scripts/e2e/env.toml` is one `[env.e2e]` for any template, with vars for the network, token, port, validators, a pinned settlement build, and the feeds and feed keys;
  - the e2e copies the template's lane file with `include = ["e2e-env.toml"]` in front, writes the vars to a `--var-file`, and checks `caravel render`;
  - the rotation in step 4b is `--var 'validators=["1","2","4"]'` on every later command. The heredoc and the `sed` edit are gone | Composition is only real once the project's own lifecycle test uses it | The e2e moves fully onto the CLI (C-14), with no raw `stellar`, `curl` or `jq` loops |
| DEC-082 | **M0.6 (C-10).** Attributes and outputs: what a deployment's expressions read once its keys and addresses are known.
- **Attributes** (`caravel_deploy::attrs`):
  - `lane` (`name`, `template`, `id`, `engine_wasm_hash`, `config_hash`, `genesis_state_hash`);
  - `network` (`name`, `passphrase`, `rpc_url`);
  - `account.admin` and `account.relayer` (`identity`, `public_key`);
  - `token.settlement` (`address`, `asset`);
  - `contract.settlement` (`address`, `pinned`);
  - `node.sequencer` and `node.validator-<name>` (`url`, `port`, `key`, …);
  - `validators` (a list);
  - `signers.settlement` (`threshold`, `count`);
  - `release.commit`.
- **Two stages, still no state file.**
  - Stage 1 reads the deployment with these roots deferred, beside `lane.name`, `lane.template`, `lane.id` and `lane.engine_wasm_hash`. A deferred value stays as written, and its path is listed. Only `relayer.feeds` may hold one; anywhere else is an error that points at the line and says why.
  - Stage 2 runs in `prepare`, once the keys, addresses, genesis and release are known. It reads the deployment again with the real attributes (`Manifest::finish`), which fills the feeds before the files are rendered.
  - A deferred local stays deferred.
- **`[outputs]`** (top level, or included) and **`[env.<name>.outputs]`** (over them) declare outputs. Each is an expression, or `{ value, description, sensitive }`. An output computed from a sensitive var is sensitive.
  - `caravel output [NAME]` lists the built-in outputs and the declared ones; a declared one wins on a name. Sensitive values are masked in listings and printed when asked for by name.
  - `status --json` has `outputs`.
  - An output that needs something unknown says what.
- **The e2e** reads the settlement and the token through `caravel output`. Its deployment declares `sequencer` and `settlement_contract` outputs, and it checks both, plus `status --json`'s `outputs` | Other tools (a web app's config, scripts) need a lane's addresses without parsing `status`, and feeds need contract ids without hand-copying them | Declared contracts and tokens (C-19, C-20) add their own attributes |
| DEC-083 | **M0.6 (C-11).** Running a lane day to day, without scripts.
- **`caravel stop [NODE…]`** stops nodes and leaves the lane as it is: the relayer first, then the sequencer, then validators. It says when a freeze becomes possible while the sequencer is down. Off a local network it asks, unless `--yes`.
- **`caravel start [NODE…]`** runs only the plan's start and restart steps for those nodes. It refuses when the deployment differs in anything else (that is `apply`'s job). A stopped node shows in `plan` and `status` as drift.
- **`caravel restart [NODE…]`.**
- **Node names:** `sequencer`, `relayer`, and `validator-<n>` or plain `<n>`, with a did-you-mean.
- **`caravel logs [NODE] [--follow] [-n N]`:** the local log file, polled for `--follow` and following a restart (`-f` stays the lane file), or `journalctl -u <unit>` over ssh. It reads no chain.
- **`caravel replay [--prove-escape WHO] [--prove-withdrawals WHO]`** runs the template's replay with the deployment's RPC, passphrase, settlement address, release engine and genesis document. The genesis is written to `.caravel/<lane>/<env>/genesis.toml`. `WHO` is an identity or a G… account.
- **`caravel wait checkpoint [--seq N] [--signed] [--epoch E] | api PATH POINTER[=VALUE] | healthy | frozen`** takes `--timeout` (exit 4 when it runs out) and `--api-url`. `checkpoint` alone waits for the next one. The conditions are pure functions over the API's JSON, polled every second.
- **`caravel api PATH [--validator N]`** does a GET on the lane's API, or on a validator's | The quickstart and e2e polled with `until … curl | jq; sleep` and killed processes by PID file | A remote host without a public URL needs an ssh-tunnelled API |
| DEC-084 | **M0.6 (C-12).** A lane's users on Stellar, from the CLI (`caravel_deploy::flows`).
- **`caravel account create NAME [--amount A]`** creates the identity if the keystore lacks it. **`caravel account fund NAME [--amount A]`** funds an existing one. Each:
  1. funds XLM by friendbot when the account isn't on the network (and waits for it);
  2. adds a trustline to the settlement token's asset;
  3. with `--amount`, gets the token:
     - the admin mints it when the admin issues it (a local token, or an `{ asset }` the admin issues);
     - Circle's testnet USDC is bought with XLM on the DEX (`path-payment-strict-receive`, `--max-xlm`, default 9,000), issuer pinned in `versions.json` `testnet.usdc_issuer`;
     - anything else is an error: get it from its issuer.
- **`caravel balance WHO`:** the settlement token on Stellar (a read, nothing sent) and the lane account from the API.
- **`caravel deposit WHO AMOUNT [--no-wait] [--timeout S]`:**
  - It checks the lane's minimum deposit and the account's token balance first.
  - It sends `deposit` and reads the inbox index it took.
  - Unless `--no-wait`, it returns once the sequencer has processed that message (`/v1/status` `inbox.processed > index`), with the lane account.
  - The lane account is the owner's key in raw hex, with no helper script.
- **Amounts and errors:** amounts are token units (`12.5`), with integer arithmetic only; the decimals are 7 for a Stellar asset, otherwise the token's `decimals()`. Settlement contract errors (`Error(Contract, #N)`) are explained (`#11`: below the lane's minimum deposit).
- **Verified:** the Stellar CLI 28.1.0 `tx new change-trust`, `tx new path-payment-strict-receive` (`--send-max`, `--dest-amount`) and `contract invoke --send=no` (`docs/SOURCES.md`) | The quickstart's users needed `stellar tx new`, `contract invoke` and `node -e` to get a token in and see it on the lane | Tokens with their own minter, or an anchor (SEP-24) in front of deposits |
| DEC-085 | **M0.6 (C-13).** Lane transactions, withdrawals, claims, forced withdrawals and escapes from the CLI (`caravel_deploy::flows`). This includes a review pass over C-11 and C-12.
- **`caravel tx --from WHO <body…>`.**
  - The body comes from the template's `plugin body`, with amounts in token units; `@name` is replaced by an identity's G… account.
  - The CLI builds the platform envelope with `signer = account` and SEP-53. The nonce is `--nonce`, else `max(next_nonce, pending_nonce)` from `/v1/accounts`. The sequencer now reports `pending_nonce`, the nonce after the account's queued transactions (read under the core lock with the state), so back-to-back sends don't reuse a nonce. Two senders that read it at once would still sign the same nonce, so the mempool now refuses a second queued transaction with an account's queued nonce (`/v1/tx` 400 `NONCE_QUEUED`; only one of them could ever run), and the CLI re-reads the nonce and signs again, up to 5 times. This is mempool policy, not consensus: blocks and the engine are unchanged. It signs `"Caravel lane tx " ‖ hex(tx_hash)` with `stellar message sign --sign-with-key WHO` (CLI 28.1.0, base64 on stdout, verified 2026-10-03). It checks the signature locally (`tx_signature_ok`) and POSTs `/v1/tx`.
  - It prints the tx hash and nonce on stderr, then scans `/v1/blocks` after the pre-send height for its exact bytes, because there is no receipt-by-hash endpoint. A failed read is retried until the timeout (the transaction is already sent). It reports the receipt code with its name and events, or "dropped" once a block passes its expiry (a used or skipped nonce, or a quarantine).
  - No key is written anywhere. Ledger identities can't sign: `message sign` has no Ledger option.
- **`caravel withdraw WHO AMOUNT`** builds the platform's WITHDRAW (kind 4), so it doesn't depend on the template. It waits for the block (a non-zero receipt fails), then for its leaf, then claims it (`--no-wait`, `--no-claim`). Leaves carry no tx hash. But every withdrawal pending at a checkpoint's end is one of that checkpoint's leaves (§11.8), so the leaf is the account's leaf of that amount in the checkpoint whose block range holds the inclusion height. That checkpoint is the first with `last_block_height ≥ height`, found by a binary search over the sealed checkpoints. Within that checkpoint its leaf is exact: every push to the pending queue is a WITHDRAW with code 0, a bounced deposit or a forced withdrawal that queued something (the last two by their platform events), and leaves keep push order. So the command's leaf is the one whose rank among the account's leaves of that amount equals the count of the account's same-amount pushes before its entry in the checkpoint's blocks. Concurrent withdrawals of one amount therefore claim distinct leaves. If the blocks can't be read, it takes the first leaf of that amount not yet `Claimed` (any of them pays the same), moving past one claimed meanwhile (#53). `claimed` means this command paid it; `already_claimed` that every candidate was claimed already; a claim that fails is reported with the leaf (`claim_error`, exit 1).
- **`caravel claim WHO`** claims every leaf that isn't yet `Claimed(seq, index)`. That state is read in batches of 100 with `getLedgerEntries`, with no simulation. Proofs come from the sequencer, else a validator (both filtered to the account, and only from a node whose accepted checkpoints reach Stellar's `LastCkpt`: a sequencer whose relayer died before reporting one lags), else `exit.json` when it is for Stellar's last checkpoint, else the node closest to Stellar with a warning. It goes past a failed leaf and reports each: paid, already claimed by someone else meanwhile (#53; a claim always pays the owner), or failed (exit 1).
- **`caravel force-withdraw WHO AMOUNT`** resolves the lane's API, then sends `request_forced_withdrawal`, whose wrapper now returns the inbox index. Its leaf is in the first checkpoint whose `inbox_through` passes the index. The lane queues at most what the account can withdraw, and says how much in the inbox entry's `FORCED_WITHDRAWAL_PROCESSED` event (a platform event, DEC-052), read from that checkpoint's blocks and receipts. Nothing queued is reported without waiting longer; otherwise its exact leaf is found and claimed as for `withdraw`.
- **`caravel escape WHO`** works on a frozen lane only, read from `FrozenInfo` and `LastCkpt` in the instance storage.
  1. It first claims the account's open withdrawals, best effort (`--no-withdrawals` skips this). A failed read or claim is reported (`withdrawals_error`, `withdrawals.failed`, exit 1) and the escape goes on. Once anything is sent, the report is always printed: a failed escape claim or balance read goes in it (`escape_error`, `balance_error`) rather than replacing it.
  2. It skips an escape already claimed, including one claimed for the owner meanwhile (#53). It reports `none` for an account with no leaf in the last checkpoint (absent from exit.json, or a 404 from a node at `LastCkpt`), and for every account of a lane frozen before its first checkpoint, whose deposits come back through `refund_unprocessed_deposit`.
  3. It takes the escape leaf from `exit.json` (only when its seq and header hash are `LastCkpt`'s, so an earlier deployment's file at the same path is passed over), else a validator or the sequencer, for Stellar's last checkpoint.
  4. It skips a claim that would pay 0 or less (it would use the escape up), with a note, unless `--allow-zero`.
  5. It reports `withdrawals_paid`, `escape_paid` and the expected amount, `max(0, floor(equity × num / den))`.
- **Users need only the admin's public key.** `addresses()` takes only the admin key, so a user can add it with `stellar keys add <admin> --public-key G…`, which is what a missing admin identity's error says (never `keys generate`, which would derive another lane's addresses). Simulated reads use the user's own identity.
- **Errors are explained by the contract that raised them.** The stellar CLI's error keeps its `Error(Contract, #N)` diagnostic lines in order. The host's event log is newest first, and a token's error inside a settlement call is repeated twice in the settlement's frame, so the raiser is the oldest error event (the highest index). The Stellar Asset Contract's codes (13: no trustline, 10: not enough) are told apart from the settlement's. `balance` reports a failed Stellar read (`stellar_error`) apart from no trustline. A bounced deposit is reported as refused, and a timed-out wait exits 4 with its result.
- **Fixes from the C-11/C-12 review** (17 findings confirmed by 3 skeptics each):
  - `wait checkpoint --epoch` without `--seq`;
  - `stop` asking for the relayer and below-threshold validators too;
  - `deposit` resolving the API before sending;
  - 404 versus an unreachable API;
  - a lazy wait target;
  - JSON pointer and path checks;
  - non-JSON replies;
  - `start`'s message;
  - `replay --json` as one document;
  - `stop --json` needing `--yes`.
- **Fixes from the C-13 reviews** (16 findings, then 8 more on the fixes, each confirmed by 3 skeptics): the ones above, plus `wait checkpoint --epoch` scanning every checkpoint from the next one; `stop 1 2 1` naming each node once; and `replay` without the admin identity pointing at `stellar keys add`, not `keys generate` | The quickstart and e2e signed with exported key files (`stellar keys secret > file`), parsed proofs with jq, and claimed through `contract invoke` | Ledger signing for lane transactions, or a receipt-by-hash endpoint |
| DEC-086 | **M0.6 (C-14), Gate G3.** The e2e on the CLI alone, and the docs for it.
- **`scripts/e2e-local.sh`** makes its lane with `caravel init <template> --prefix e2e`, includes `scripts/e2e/env.toml` (whose identities are now `e2e-<role>`) and drives every step with `caravel`: `validate`, `render`, `doctor`, `keys ensure`, `apply`, `plan --exit-code`, `output`, `account create`, `deposit`, `tx`, `wait api`, `wait checkpoint`, `withdraw`, `stop relayer`, the rotation by `--var`, `logs`, `force-withdraw`, `destroy`, `balance`, `escape` and `replay`, with `jq` for the checks.
  - Gone: the raw `stellar` invocations, `curl`, `node -e` (the perps fixture oracle key: init generates the oracle identity), and the `until_ok` sleep loops.
  - `scripts/check-e2e.sh` (in CI's checks job, on every pull request) fails if the script calls `stellar`, `curl`, `node -e/-p`, `kill` or `sleep` outside its cleanup trap.
  - Amounts are compared in base units, with no floats.
- **`scripts/check-quickstart.sh`** installs from the checkout into a scratch prefix and runs the README's quickstart block, the one after its `<!-- quickstart -->` marker, as written, in a scratch keystore. It runs locally and in a CI job on main.
- **`CARAVEL_YES=1`** answers `apply`, `destroy` and `stop` as `--yes` does, for scripts and CI. At a terminal, `apply` still shows the plan and asks.
- **`caravel balance C…`** reads a contract's balance of the settlement token (the settlement's vault, say), with the admin as the simulation's source.
- **Release checksums:** `assemble-release.sh` and `install.sh` hash only the release directories that exist. A payments-only install has no `relayer-feeds`, and `find` failed on it under `pipefail`. This carries the fix of the open #55 (`xargs` can't run a shell function).
- **Docs:**
  - `docs/LANE_FILE.md` is the language reference, and its worked example is checked with `caravel validate` and `caravel render`;
  - the README's quickstart goes from `install.sh` and `caravel init` to `caravel escape`, and lists the commands and exit codes;
  - the RUNBOOK uses the CLI for the e2e, the rotation check, the freeze drill and replay;
  - the landing page's quickstart matches the README, in the repo only until approved;
  - `docs/SOURCES.md` has `stellar keys add --public-key` | The quickstart was a set of commands and a loop, and the e2e reached past the CLI for half its steps, so neither showed that the CLI is enough | Shell completions (`caravel completions`), left out: they would add `clap_complete` |
| DEC-087 | **M0.6 (C-15).** The plan comes from a graph of resources (`caravel_deploy::graph`); every plan is the same as before.
- **Resources** have an address and a kind:
  - `account.admin` and `account.relayer`;
  - `token.settlement`, `wasm.settlement`, `contract.settlement` and `signers.settlement`;
  - `host`, `host.data` and `release`;
  - `file.<path>` for each generated file;
  - `node.<name>` for each node, and for a running node the lane file no longer names.

  Each resource compares the lane file with Stellar and the host and gives its steps and problems. This is the same comparison `diff` made, split up by resource.
- **Edges:**
  - `Order` edges carry what a resource reads or must follow. The admin, token and Wasm come before the contract. The contract comes before the host's data and before the nodes, which check against it. The validators come before the signer rotation, and the rotation before the sequencer. Funding the relayer comes before the relayer node.
  - `Restart` edges run from each file to the nodes that read it, and from the release and a wiped store to every node. This replaces the `nodes_of` restart map.
- **Order:** steps come out in a topological order (Kahn's algorithm). Ties go to the kind's rank (Stellar, the host's checks and data, the release, files, nodes and the signer set, orphans), then to declaration order. That reproduces the fixed order step for step. Problems come out in declaration order. A cycle is an error that names its resources, which matters once `depends_on` arrives (C-16).
- **Checked:**
  - the 10 plan goldens and every plan test pass unchanged;
  - a property test compares the graph's plan with the old fixed-order `diff` (kept under `cfg(test)` until C-16) on 2,000 random states of the deployment, Stellar and the host per run, and mutating the code makes it fail;
  - lane #1's read-only plan is still "No changes.".
- **Still no state file:** every resource is read back from the lane file, Stellar and the host. `Graph::dot` is there for `caravel graph` (C-16) | Accounts, tokens, contracts, modules and several hosts (C-16 to C-25) are new kinds of resources with their own edges, and a fixed step order can't take them | — |
| DEC-088 | **M0.6 (C-16).** Addresses in plans, `caravel graph`, `--target` and `--replace`.
- **Every step and problem names its resource.** A step or problem maps to its resource's address deterministically (`plan::step_addr`, `plan::problem_addr`), so `Plan` keeps its shape and filters like `start`'s can't drift from the addresses.
  - `plan` prints each step's address in a column (it starts at most 42 characters in; a longer line just takes two spaces).
  - The 10 goldens changed only by that column: stripping it gives back the old files byte for byte (checked).
- **`plan --json` is `caravel-plan/1`.** It has `format`, `targets`, `replaced`, steps `{address, change, action, line}` and problems `{address, message}`; problems used to be strings.
- **`caravel graph`** prints the deployment's graph as Graphviz DOT, with restart edges dashed, or `--json` (`resources`, `edges`).
- **`--target ADDR`** (on `plan` and `apply`, repeatable) keeps the targets and everything upstream of them through edges of both kinds.
  - Targeting the relayer includes its account, the contract and what it needs, the release, and the files it reads, but no validator.
  - Targeting the sequencer includes every validator, because the rotation follows them.
  - Patterns use `*` for any characters (`node.validator-*`, `*.settlement`). An address that names nothing gets a did-you-mean from the graph.
  - The plan's header says `target: …`. After a targeted apply, the check that follows is targeted too.
- **`--replace ADDR`** forces a change on a node (restart, or start), a file (written again, so its nodes restart) or the release (installed again, so every node restarts). Every other kind is refused with the reason:
  - a settlement contract's address derives from the admin and the lane's name, so a new one is a new lane;
  - the signer set changes through a rotation;
  - accounts, the token, the Wasm, the host and its data each have their own reason.

  The check after `apply --replace` doesn't replace again.
- **`depends_on`:** the graph takes extra `Order` edges (`Graph::depends_on`), and a cycle is an error that names it. No lane-file table is a free-standing resource yet, so the lane-file syntax comes with declared accounts, tokens and contracts (C-18 to C-20).
- **The old fixed-order diff** stays under `cfg(test)`: the property test still checks the graph's untargeted plans against it.
- **Lane #1's plan** is still "No changes." (no lines, so no column) | Operators need to act on one part of a lane and to read plans as data; addresses are what `--target`, `--replace` and saved plans (C-17) refer to | `depends_on` in the lane file (C-18) |
| DEC-089 | **M0.6 (C-17), Gate G4.** Saved plans (`caravel_deploy::saved`): `plan --out FILE`, then `apply FILE`.
- **The file (`caravel-saved-plan/1`)** records:
  - the lane file, the deployment, the release and Wasm dirs, `--target` and `--replace`;
  - the plan (`caravel-plan/1`), and the release commit;
  - the caravel version that made it;
  - its basis:
    - the sha256 of every file the lane file loaded (itself and its includes) and of every `--var-file`;
    - each var's value as `--var` reads it back; a sensitive var only as `sha256:` of its value salted with the lane id and the var's name;
    - three digests, of the resolved deployment (`desired`), of what Stellar has (`chain`) and of what the host has (`host`). Each is the sha256 of the value's `Debug` form, which is deterministic (ordered maps and sets) within one caravel version, so another version's plan is refused.
- **No secret goes in:** no key, no file's content, no sensitive value. A run checked every identity's secret against a saved plan.
- **`apply FILE`** runs as the plan was made. It takes the plan's lane file, deployment, release and wasm dirs (unless given) and the plan's vars as `--var`; a sensitive var must be given again as before. A different `--env` is refused. It refuses a missing identity rather than create it.
- **It recomputes everything.** It refuses, naming each thing that moved, when:
  - a loaded file or var file changed, appeared or went away;
  - a var differs (a sensitive one says only that it differs);
  - the deployment differs (the release, if that's what changed);
  - Stellar differs;
  - the host differs.

  If the basis matches but the steps computed now aren't the saved ones (or have problems), that is refused too.
- **A plan runs once.** A plan with problems can't be saved. After it applies, the file is marked `applied` and refused afterwards, so a `--replace` can't run twice by accident. `--json` prints one document, `{ok: false, moved}`, when refused.
- **Checked on a local lane:**
  - save then apply;
  - apply again (refused);
  - a stopped node (refused: the host changed);
  - a `[node]` edit (refused: the lane file changed);
  - another `--env` (refused);
  - vars restored from the plan;
  - no identity's secret in the file.

  Unit tests cover each refusal, the salted hash, the version and format checks, and the applied mark | Review a plan, then apply exactly what was reviewed, for CI and team review, still with no state file | Remote or signed plan files |
| DEC-090 | **M0.6 (C-18).** Declared accounts: `[env.<name>.accounts.<n>]`.
- **Fields:**
  - `identity` (the name when not given);
  - `fund` (default true: friendbot when missing);
  - `trustlines`: `"settlement"` for the settlement token's asset, or `"CODE:G…"`;
  - `balances = { settlement = "<units>" }`, a minimum to top up to, as a decimal string;
  - `depends_on`, a list of addresses.

  Checks: a name isn't `admin` or `relayer`; an identity is not a key; amounts parse; a balance needs the settlement trustline; a contract token can't be trusted or topped up, because it's no Stellar asset and its balances aren't readable.
- **Each account is a resource, `account.<n>`.** Its steps are `fund`, `trust` and `mint`; its problems are `AccountMissing` (missing, with `fund = false`) and `CannotMint`.
  - A top-up mints `want − have` through the token's contract, and only when the admin issues the settlement asset; otherwise it is `CannotMint`, with both amounts.
  - An issuer needs no trustline to its own asset.
  - An account that trusts or holds the settlement token follows `token.settlement` (and the admin, when it mints). `depends_on` adds `Order` edges, and an unknown address is an error naming the account. This is the lane-file `depends_on` that DEC-088 deferred.
- **`Desired`** gains `accounts` and `settlement_asset`, the Stellar asset behind the settlement token, Circle's USDC included. **`Chain`** gains `trustlines` (account, code, issuer → balance).
- **The chain reader** reads the declared accounts and their trustlines in batched `getLedgerEntries` (nothing to read when none are declared, so lane #1's reads are unchanged). `address::trustline_key` builds the keys.
- **`account.<n>.identity` and `.public_key`** are attributes. Declared identities join `keys list` and `ensure`, and a local `apply` creates them.
- **The admin and relayer** stay the implicit `account.admin` and `account.relayer`. `Step::Fund` names its account by a string now.
- **Checked:**
  - a golden plan with accounts;
  - a top-up of only the difference, and nothing once enough is held;
  - `CannotMint` and `AccountMissing`, in text and JSON;
  - `depends_on` order and unknown addresses;
  - manifest checks;
  - the property test, unchanged;
  - on a local lane: apply funds, trusts and mints (100); "No changes." after; raising the balance to 150 mints 50; an output reads `account.alice.public_key` | Test users, market makers and treasuries were shell steps after `apply`; declaring them makes them part of the deployment | Balances in other tokens (C-19), and taking a balance back down |
| DEC-091 | **M0.6 (C-19).** Declared tokens: `[env.<name>.tokens.<n>]`, with `code` and `issuer` (`"admin"`, a declared account's name, or a `G…` address).
- **Each token is a resource, `token.<n>`.** `apply` deploys its Stellar Asset Contract when missing, paid by the admin (anyone may deploy one). It follows the admin and an issuer that is a declared account. Accounts that trust or hold it follow it, and so does the account that mints their top-ups. The chain reader checks the contracts in one batch; nothing is read when none are declared.
- **The settlement token can be a declared one: `token = "<n>"`.** The token is rewritten after the checks:
  - issued by the admin → `{ local = CODE }`, which is `CODE:<admin>`, now on any network (the testnet refusal of `local` applies only to the form as written);
  - issued by a `G…` address → `{ asset = "CODE:G…" }`.

  Only these two issuers are allowed, because the lane's addresses (and its users' flows) derive from the admin's key alone. Today's forms are unchanged, and so are their plans (the original 10 goldens didn't move). It stays `token.settlement`: a declared token equal to it isn't a second resource.
- **Accounts' `trustlines` and `balances`** take a declared token's name, and a balance needs a trustline to its token. A top-up (`Holding`) is minted by the token's issuer when the file has it: the admin, or a declared account through its identity. Otherwise it is `CannotMint`. `Step::Mint` carries the contract and the minting identity; `Step::DeployToken` names its token.
- **Expressions** read `token.<n>.address`, `.asset`, `.code` and `.issuer`.
- **Checked:**
  - manifest checks: names, codes, issuers, and the two settlement-token rewrites;
  - a golden plan with a token issued by a declared account (issuer, then token, then holder), and its "No changes." once deployed;
  - C-18's golden, whose mint line now names the minting identity;
  - on a local lane: settling in a declared `usd`, with `eur` issued by a declared `treasury`; contracts deployed, trustlines, mints by each issuer; "No changes." after; outputs; a deposit in `usd` | A lane's tokens were fixed forms, and a second asset (for users, or a contract in C-20) needed shell steps | Tokens that aren't Stellar assets (SEP-41 contracts) as declared resources |
| DEC-092 | **M0.6 (C-20).** Declared contracts: `[env.<name>.contracts.<n>]`. **Constructor arguments are set once (the human's choice).**
- **Fields:**
  - `wasm`: a `.wasm` file relative to the lane file, or the sha256 of uploaded Wasm;
  - `deployer`: `"admin"` (default) or a declared account;
  - `salt`;
  - `args`, the constructor's arguments by name: strings as written, numbers and booleans as written, lists and tables as JSON. They may use values known only with the keys (account, token and contract addresses, the lane's hashes), filled in before the contract is deployed;
  - `depends_on`;
  - `lifecycle.prevent_destroy`. `[env.<name>] lifecycle` has `prevent_destroy` too.
- **The address:** `contract_id(network, deployer, sha256("caravel/contract" ‖ lane_id ‖ name ‖ 0 ‖ salt))`. A unit vector matches `stellar contract id wasm`.
- **Each contract is a resource, `contract.<n>`.**
  - When it's missing: `UploadWasm` (when the network lacks the Wasm and a file is named; with a hash only, the problem is `WasmMissing`), then `DeployContract` (`stellar contract deploy --wasm-hash --salt -- --<arg> <value>…`). Apply checks the deployed address against the planned one.
  - When it's deployed: running other Wasm is `ContractCodeDrift`, since this tool upgrades nothing.
  - It follows its deployer, any resource whose address one of its arguments names, and its `depends_on`.
- **Arguments aren't read back.** Stellar keeps no record of a constructor's arguments, so they are used at deploy and not checked again. `plan` gives a note (`Plan.notes`, in its text and `caravel-plan/1`) for a deployed contract that has some. Changing them means a new contract with a new salt, and `--replace contract.<n>` is refused with that hint. The rejected options were an on-chain fingerprint (a data entry on the deployer, ~0.5 XLM, bending "no state") and a local fingerprint file (a state file).
- **`caravel destroy`** refuses while the deployment or any declared contract sets `lifecycle.prevent_destroy`, and names them.
- **Expressions** read `contract.<n>.address` and `.deployer`. `contracts.*.args` joins `relayer.feeds` as a place a deferred value may be.
- **The chain reader** reads each contract's instance (the Wasm it runs) and its Wasm's code entry, in one batch; nothing is read when none are declared.
- **Checked:**
  - the address vector;
  - manifest checks;
  - a golden plan (upload and deploy after the token and admin its arguments name);
  - "No changes." with the note once deployed;
  - `ContractCodeDrift`, `WasmMissing`, and the `--replace` refusal;
  - on a local lane: a contract with no constructor (Wasm uploaded and deployed), and a second settlement contract whose arguments come from expressions (addresses, the lane's hashes, a signer struct, params). It was deployed at the planned address, then "No changes." with the note, and `--replace` and `destroy` refused | Lanes need their own contracts (oracles, registries) next to the settlement one, wired by address | Upgrades of declared contracts, and checking arguments that a contract exposes through a getter |
| DEC-093 | **M0.6 (C-21), Gate G5.** Local modules: `[env.<name>.modules.<m>]`, with `source` (a local `.toml`; anything with `://` is refused), `inputs`, and optionally `for_each`. The language crate expands them (`caravel_lanefile::modules`).
- **A module file** holds only `[inputs.<n>]` (`type`, `default`), `[locals]`, `[accounts]`, `[tokens]`, `[contracts]` and `[outputs]`.
  - Inputs are read in the deployment's scope (with `each` under `for_each`), defaulted, type-checked, and refused when unknown (did-you-mean) or missing.
  - Its expressions read `input`, its own `local`, `lane`, `env`, and the deployment's later names (`account`, `token`, `contract`, `network`, …), where its own resources answer to their short names. It doesn't read the lane file's `var`.
- **Its resources join the deployment** as `<instance>.<n>` in `accounts`, `tokens` and `contracts`. With `for_each`, an instance is `<m>-<key>` (the map key, or the list's string value).
  - References to its own resources are qualified: a token's issuer, a contract's deployer, trustlines and balances, and `depends_on` (as `module.<instance>.<kind>.<n>`).
  - A module account's identity defaults to `<instance>-<n>`.
  - A contract's Wasm file is found next to the module file.
  - A `.` in a name declared outside a module is refused.
- **Two stages, as for the deployment:** at the first read, the later names are deferred; once keys are known, the module is read again with them. A module contract's `args` therefore wait at `contracts.<instance>.<n>.args…`, a path where deferred values are allowed.
- **The deploy tool** gives `<instance>.<n>` the address `module.<instance>.<kind>.<n>` (`plan::res_addr`), in the graph, plan lines, problems, `--target` and `depends_on`.
- **Outputs:** an instance's are `module.<instance>.<name>` in the deployment's `[outputs]`.
- **Fixes found on the way, now covered by tests:**
  - An issuer that lists its own token got edges both ways, a false cycle. An issuer needs nothing of its own token, so those edges are gone.
  - The chain reader asked for duplicate ledger keys when two contracts shared a Wasm, and the RPC refuses such a batch ("could not query captive core … 404"). Every declared batch now asks for each key once, and never for an issuer's trustline to itself.
- **Checked:**
  - unit tests: resources joining, qualification, Wasm paths, deferred args, the second stage with own names, outputs, `for_each` instances, and every refusal;
  - a plan test of an issuer holding its own token;
  - on a local lane: one module file used twice (`for_each` btc and eth). Each instance's feeder was funded and issued its token, and each instance's contract (a settlement Wasm, args from the module's own names and the lane's hashes) was deployed by its feeder at the planned address. Then "No changes.", outputs through `module.<instance>`, and `--target 'module.oracle-eth.*'` | Repeated groups of resources (one oracle per market, one vault per asset) were copy-paste in the lane file | Remote module sources, and module outputs read elsewhere than `[outputs]` |
| DEC-094 | **M0.6 (C-22).** Several hosts per deployment: `[env.<name>.hosts.<h>]`, each a `host` table as before, plus an optional `private_address` (an IP).
- **Placement:** `[sequencer] host = "<h>"` names the sequencer's host, which also runs the relayer (it calls the sequencer's `/internal/*` on loopback). It may be left out when there is only one host. Each validator takes `host = "<h>"`, defaulting to the sequencer's. `host` and `hosts` together are refused. A single `host` is still one host, and its output is byte for byte what it was (lane #1's pins pass unchanged).
- **The sequencer's host stays primary:** its resources keep their one-host addresses (`host`, `host.data`, `release`, `file.<path>`, `node.<name>`). Another host's are `host.<h>`, `host.<h>.data`, `host.<h>.release` and `host.<h>.file.<path>`. A host can't be named `data`, `release` or `file`, or contain a `.`.
- **Each host gets only what it runs:** another host gets `lane.toml`, its validators' configs and unit, and, with a `public_url`, a Caddyfile serving their public API (`/v1/sign` closed). A validator's key goes only to its own host, and the env secrets (relayer, feeds) only to the sequencer's. When a validator moves or is rotated out, its key file is removed from the host it left.
- **Reaching across hosts:** a node another host calls listens on its host's `private_address`; everything else stays on 127.0.0.1. Two `local` hosts are the same machine and use loopback. A validator on another host must be reachable both ways (it follows the sequencer, and the sequencer calls its `/v1/sign`), or the manifest says which `private_address` is missing. Until C-23 signs those calls (OQ-009), the private network is the only thing between `/v1/sign` and other machines on it, so a cross-host deployment on testnet should wait for C-23.
- **Moves without a state file:** a host reports the nodes it runs. A node running on a host the file doesn't place it on is `node.<name>@<h>`, and the plan starts it on its new host before stopping it on the old one. A host removed from the file is no longer read: move its nodes off first.
- **Local state:** another `local` host's root is `.caravel/<lane>/<env>@<h>`.
- **Checked:**
  - unit tests: parsing, placement, `reach`/`listen_on`, and every refusal; lane #1's lane file split across two hosts renders the right files for each; a plan golden for two hosts and for a moved validator; host-qualified steps, problems and targets;
  - on a local lane with two `local` hosts (validator 3 on `b`): apply, "No changes.", checkpoints accepted, validator 3 moved to `a` and back (start, then stop on the old host, its key following it), and destroy, which froze the lane and stopped and wiped both hosts | One host was a single point of failure, and every validator's key sat next to the sequencer's | Validators operated by others (C-23), traffic over `public_url` without a private network (C-23), the relayer on its own host |
| DEC-095 | **M0.6 (C-23), per OQ-009.** Networking across hosts, and validators run by others. Every call to a validator's `/v1/sign` is authenticated; the network isn't the boundary.
- **Signed requests:** `[env.X.sequencer] key` names the sequencer's own identity. Its secret goes only to the sequencer's host (`keys/sequencer.key`, `key_file` in `sequencer.toml`). Each validator's config names the public key (`sequencer_key`).
  - The sequencer signs every `/v1/sign` request: ed25519 over `"caravel/v1/sign" ‖ time (Unix seconds, u64 BE) ‖ sha256(body)`, sent as the `X-Caravel-Time` and `X-Caravel-Signature` (hex) headers (`caravel_node::sign_request`).
  - A validator with a `sequencer_key` answers 401 `UNSIGNED` to a request that is unsigned, signed by another key, or more than 60 s from its clock, before it reads the body.
  - A replay within the window can only ask for the same signature again, which validators already give idempotently (§15).
  - No token, no CA, and no state file. Not consensus, and no frozen format changes. Configs without the key behave as before, so lane #1's files are byte for byte unchanged.
- **Required once a validator is afar:** a deployment whose validators are all on the sequencer's host may leave `key` out. One with a validator on another host, or run elsewhere, is refused without it.
- **The private network is optional:** with `private_address`, another host's nodes are reached there, and those nodes listen there. Otherwise they go through the host's `public_url` (`<url>` for the sequencer, `<url>/validators/<name>` for a validator), and the nodes stay on loopback behind Caddy.
  - That host's Caddyfile passes a validator's `/v1/sign` only when the sequencer reaches it that way. `/internal/*` stays on localhost: the relayer shares the sequencer's host.
  - The manifest refuses a placement that neither address can reach, both ways.
- **Validators run elsewhere:** `[[validators]] name, key, url` (no `host` or `port`).
  - `key` names an identity added from the operator's public key (`stellar keys add <name> --public-key G…`). `apply` never creates it, and says so.
  - It is in the signer set and the sequencer's `[signers]` at its `url`, with no node, files, keys or unit.
  - `status` and `output` list it, and proofs are fetched from it like the others. `node.sequencer.key` gives the key its operator puts in `sequencer_key`.
- **C-22 fixes found on the way:**
  - A validator on the sequencer's host followed `127.0.0.1` while the sequencer listened on its private address. Every URL now comes from one function, `EnvSpec::url_of`.
  - `validator_url` named the sequencer's host for validators on other hosts.
- **Checked:**
  - `sign_request` unit tests;
  - the validator API test with signed requests end to end. Checkpoints are signed and accepted, and unsigned, wrong-key and stale requests get 401;
  - manifest tests for URLs, the public fallback, the key requirement and validators run elsewhere;
  - lane #1 split over two hosts, with and without private addresses: the sequencer key in the configs, and `/v1/sign` open on b's proxy only through its public URL;
  - lane #1's pins are unchanged;
  - on a local lane with two `local` hosts, a sequencer key and a validator run by hand outside the deployment, with the threshold at 4 of 4: checkpoints accepted (so the outside validator answered the signed requests), unsigned requests to it and to validator 3 refused with 401, "No changes.", and destroy | A validator on another machine, or run by someone else, must not be asked to sign by anyone who can reach it | mTLS, or keys rotated without a restart; a relayer on its own host (it would need `/internal/*` across hosts) |
| DEC-096 | **M0.6 (C-24).** Several lanes on one ssh host: `namespace = "<ns>"` in a host table (up to 32 lowercase letters, digits and `-`; refused on `local` hosts, whose lanes are already apart under `.caravel/<lane>/<env>`).
- **What a namespace sets apart:**
  - its units, `caravel-<ns>-sequencer`, `caravel-<ns>-relayer` and `caravel-<ns>-validator@`, each depending on its own sequencer;
  - its Caddy site, a snippet at `/etc/caddy/caravel.d/<ns>.caddy` (the host's Caddyfile must `import /etc/caddy/caravel.d/*.caddy`, or the host lacks it);
  - its root, `/opt/caravel-<ns>` unless `root` is given. The first release creates it, so a namespace needs no provisioning beyond the host's.
- **Without a namespace,** the names are a single lane's (`caravel-*`, `/etc/caddy/Caddyfile`, `/opt/caravel`). Lane #1's files and plan are unchanged.
- **Sharing is checked, never assumed:** reading an ssh host now also reports:
  - the lane and deployment named by its root's `lane.toml` header;
  - the root its units run, from the `--config` path in the first unit found;
  - which nodes' ports something listens on (`ss`).
  The local provider probes the ports of nodes it doesn't run.
- **New problems:**
  - `HostTaken`: another lane's root or unit names, at `host`. The plan names the other lane or root and suggests a namespace.
  - `PortInUse`: a node that isn't running and whose port answers for something other than this lane, at `node.<name>`.
  Both refuse apply. The read on the ssh side lists only the lane's own unit files and Caddy site, so other lanes' files never show up as its own. `nodes_of` maps `systemd/caravel-[<ns>-]<role>.service` to the nodes each unit runs.
- **Checked:**
  - unit and golden tests: namespaced names, roots and refusals; lane #1's lane file in a namespace renders `caravel-perps-*` units and a `caravel.d` snippet; `HostTaken`, `PortInUse` (golden `plan-port-in-use`), a namespaced unit restarting its node, and the header parser;
  - live: a local lane whose validator port another program held planned `PortInUse`;
  - read-only against lane #1's VM: lane #1 still plans "No changes.", and a copy of its file with another root and no namespace planned `HostTaken` ("the units caravel-* run the lane at /opt/caravel") | Lanes had to have a host each, and a second lane on the same host would have overwritten the first one's units and Caddyfile | Lane #1's Caddyfile importing `caravel.d`, so a namespaced lane can join its VM (a change to lane #1's files, so a VM release for the human) |
| DEC-097 | **M0.6 (C-25), Gate G6.** The web app's config as a resource: `[env.<name>.web]` with `host` (default: the sequencer's) and `config`, a table of anything the app reads.
- **Values:** `config` may use values known only once keys and addresses are (`web.config` joins `relayer.feeds` and contracts' `args` in `attrs::DEFERRED_OK`), and `Manifest::finish` fills them in.
- **Where it goes:**
  - it is rendered to `<root>/config/web.json` on the web host, a plain file resource (`file.web.json`) that restarts no node;
  - that host's Caddy serves it at `/config.json` (`root * <root>/config`, `rewrite * /web.json`), before the app's catch-all, so nothing else in `config/` is reachable;
  - on a host other than the sequencer's, the site also serves the web app.
- **Refused:** a web host that isn't declared, or an ssh one without a `public_url`.
- **The perps web app** fetches `/config.json` once at boot (`no-store`), before the modules that read the config load (`App` is imported after it). It lays known keys of the right type over its defaults, resolving URLs against the page. A missing file, an HTML fallback or a fetch error keeps the defaults: `VITE_*`, else the testnet lane.
- **Lane #1:** it declares no `[web]`, so its files and plan are unchanged. Giving its VM a `web.json` is a VM release, for the human.
- **Checked:**
  - unit tests: deferred values filled in and host refusals; lane #1's file with a web config renders `web.json`, with the route ahead of the app's catch-all; a `web.json` change restarts nothing; the web app's `applyConfig`/`loadConfig` (known keys only, URLs resolved, defaults kept without a file);
  - `caddy adapt` (Caddy 2, Docker) accepts lane #1's Caddyfile with the route;
  - on a local lane: `web.json` holds the sequencer's and validators' URLs, the settlement address and the passphrase, then "No changes." | The web app's deployment was baked in at build time (`VITE_*`), so one build couldn't serve another lane | Serving `web.json` from a local lane's dev server |
| DEC-098 | **M0.7 (H-02).** Prebuilt releases, built by `.github/workflows/release.yml`.
- **Build:** one job builds the contracts of record on x86_64 Linux and checks them against `versions.json` (DEC-033). One job per platform then builds `caravel` and every template's node, the relayer, the feed modules and the web apps, and assembles them with `scripts/assemble-release.sh`, the layout `install.sh` already installs. The platforms:
  - `x86_64-linux` and `aarch64-linux` on Ubuntu 22.04 (glibc 2.35, so they run on 22.04 and later);
  - `aarch64-macos` on macOS 14.
- **Check:** each platform runs its own binaries (`caravel version`, `plugin info`, a genesis check). It packs `caravel-<version>-<target>.tar.gz` with a `.sha256`, and installs from the archive.
- **Publish:** a `v*` tag opens a **draft** GitHub Release with the archives and a combined `SHA256SUMS`. A person publishes it, and the first public release waits for the human.
- **Test runs:** a pull request that touches the workflow or the release scripts builds and checks everything and publishes nothing.
- **`install.sh --archive FILE`** installs a release archive without build tools. It checks the archive against its `.sha256` when present and every file against the release's `SHA256SUMS`, then installs as the source path does. Node.js 22 is still needed at run time, for a lane's relayer.
- **Still separate:** the CI `release` job stays as the VM's artifact. | Building from source took a Rust toolchain and a long first build, too much for a hackathon's first hour | Intel macOS, Windows, signed and notarized macOS binaries |
| DEC-099 | **M0.7 (H-03).** One-line install: `curl -fsSL https://raw.githubusercontent.com/wmendes/caravel/main/scripts/install.sh \| bash`. It's the same script as the source install, with new modes.
- **Which mode:** piped, or with `--release` in a checkout, it downloads the release. In a checkout without flags it builds from source as before, so `check-quickstart.sh` and the docs' source path don't change. `--archive FILE` installs a local archive (H-02).
- **Download:**
  - picks the target from `uname` (x86_64 or arm64 Linux, arm64 macOS; anything else is told to build from a checkout);
  - takes `--version vX.Y.Z` or the latest published release from the GitHub API;
  - downloads the archive and the release's `SHA256SUMS`, and refuses an archive that isn't listed or doesn't match;
  - then the archive path checks every file against the release's own `SHA256SUMS`.
  - `CARAVEL_REPO` and `CARAVEL_DOWNLOAD_BASE` (a mirror laid out like GitHub's) are for forks and tests.
- **Stellar CLI:** when the `stellar` on PATH isn't the pinned version, the installer downloads the pinned 28.1.0 for this platform into the prefix and checks it against GitHub's published digest (SOURCES, 2026-10-03). `--no-stellar-cli` skips this. `check-versions.mjs` now holds the installer's, CI's and the release workflow's pins to `versions.json`.
- **What's still needed:** Docker for the local network and Node.js 22 for a lane's relayer. The installer says so, and `caravel doctor` checks both.
- **Checked:**
  - piped with no release published (a clear refusal);
  - piped against a local mirror (installed);
  - a tampered `SHA256SUMS` (refused);
  - an archive install with no `stellar` on PATH (the pinned CLI downloaded, checked and run).
  - The release workflow installs from its own archives on every platform. | Hackers should be one command from a working `caravel`, without Rust or a matching Stellar CLI | Bundling Node.js; a Homebrew formula; Windows |
| DEC-100 | **M0.7 (H-12).** Caravel's documentation is a Docusaurus 3.10.2 site in `docs-site/`, its own Vercel project (`caravel-docs.vercel.app`).
- **Why Docusaurus:** developers.stellar.org runs Docusaurus 3.10.1 (checked 2026-10-03), so Stellar developers find the navigation familiar. It is mature and versioned, with MDX and Algolia DocSearch when we want it. The human chose it, its own project, local search now, and all four sections in the first release.
- **Layout:**
  - `docs-site/docs/` holds the user's docs: Get started, Concepts, Guides, Reference, What's new.
  - `docs/` stays the internal record: the spec, SOURCES, RESULTS, BENCHMARKS, SECURITY, and lane #1's RUNBOOK.
  - The docs are the site (`routeBasePath: '/'`).
  - `.md` pages are CommonMark (`markdown.format: 'detect'`), so a lane file's `${...}` and `<name>` stay text.
- **One source per fact:**
  - the lane-file reference moved from `docs/LANE_FILE.md`, which is now a pointer, into `reference/lane-file/` (ten pages, split by section);
  - the CLI reference (39 pages, one per command and subcommand) is generated from `caravel help` by `scripts/gen-cli-docs.mjs`;
  - CI fails when the pages differ from the binary (`scripts/check-cli-docs.sh`).
- **Checks:**
  - the build fails on any broken link or anchor;
  - `scripts/check-docs.sh` holds the copy to spec §2 (no "trustless", "audited" only as "not audited", "mainnet" only as refused, no production-ready claims, the IaC tool never named, no em dashes);
  - `check-versions.mjs` covers the docs site's exact pins and npm 10 lock;
  - CI's apps job runs typecheck and build.
- **Look:** a quieter palette than the landing page's, by the human's call: white and a near-black charcoal with neutral greys. The landing page's coral (links, the active page, cautions) and teal (notes, markers) appear only as accents, with self-hosted Schibsted Grotesk. Admonitions have even borders, never a side stripe. There is a docs share image, and an announcement bar that can't be dismissed: testnet software, not audited.
- **Search:** `@easyops-cn/docusaurus-search-local` 0.55.3 (lunr, built at build time; its `open-ask-ai` peer is optional and not installed). Algolia DocSearch can replace it later. | The landing page's job became attracting visitors; the docs were team-facing Markdown with no navigation or search, and the lane-file reference lived in one 30 KB file | Versioned docs (after the first tagged release), DocSearch, a custom domain |
| DEC-101 | **M0.7 (H-13).** The perps web app is redesigned as a trading terminal, by the human's brief (reference: Hyperliquid; dark only; desktop first; whole app). It overrides §18.1's brand and DEC-047's "direction without colour" for the perps app only:
- **Colour roles:** green and red mark direction only (long/short, bids/asks, profit/loss) and always come with a sign, arrow or word, so colour is never the only signal; coral marks lane state (soft), teal marks Stellar state (settled) and Stellar actions; amber marks stale or degraded data. Surfaces are OKLCH near-blacks tinted toward the brand's teal hue, split by hairlines, not cards.
- **Type:** Inter (variable, 400 to 700, OFL) self-hosted in `public/fonts/`, tabular figures for every number; the wordmark is the Caravel logo SVG. Schibsted Grotesk and Google Fonts are gone from the app.
- **Layout:** market bar (oracle mark and its age, best bid/ask, spread, open interest, max leverage = 10000 / `imf_bps`, fees, price band); chart, book with trades, and the order ticket with the account under it; positions, open orders and your trades below; a status bar with the lane block (soft) and the accepted checkpoint (settled). Only numbers the lane API serves are shown: no funding rate, no 24-hour change.
- **Ticket:** Limit (GTC), Market and Post-only. Market is an IOC limit at 90% of the price band from the oracle (the engine accepts \|price − oracle\| ≤ band), so a small oracle move between signing and sequencing does not reject it; Close on a position sends the same order, reduce-only. Clicking a book level sets the price; B and S pick the side.
- **First run:** the ticket pane shows four steps (connect, deposit on Stellar, enable fast trading, place an order) until the account exists.
- **State:** the connected account moves into the app state, polled every 5 s and updated by the stream, so the top bar, ticket and portfolio share it. The API, codec, signing and session-key code are unchanged.
- `lanes/perps/web/PRODUCT.md` records the users, principles and anti-references the design follows; `copy.test.ts` still holds the §2 claims. | The human asked for a professional perps DEX look. Traders read direction by colour on every venue they know, and the soft/settled distinction keeps its own two colours | A light theme, or real price history from the lane |
| DEC-102 | **M0.7 (H-14).** A feed tick that never settles must not stop its feed. What happened on lane #1 (2026-10-03): at 00:59 UTC the VM stalled briefly (the relayer's inbox and checkpoint calls to the sequencer timed out), and the oracle loop's tick never returned. No error was logged, the relayer stopped posting to `/internal/oracle` (checked with tcpdump), and prices stayed at 00:59:11 for about 19 hours while blocks and checkpoints went on. The only unbounded call on the oracle's path is Reflector's simulation through `rpc.Server`: `@stellar/stellar-sdk` 17.2.0 accepts a `timeout` option there but never passes it on (its `createHttpClient` takes only headers), and the HTTP client's default is no timeout. Fixes:
- `rpc.Server.httpClient.defaults.timeout` is set on every RPC client: 10 s for Reflector reads (`RpcReflector`), 30 s for the relayer's settlement calls (`RpcContract`), so a hung checkpoint call cannot run out the escape timeout either;
- the relayer runs each feed tick under a deadline (`feeds[].deadlineMs`, default 30 s): a late tick is abandoned and logged as `<feed> tick: no answer in N ms`, and the loop goes on. It is not cancelled; feed updates carry their own publish time, so a late post is harmless;
- tests: an RPC server that never answers (`reflector.test.ts`), a feed module whose tick never settles, and the client's default timeout. | A silent stop is the worst failure for an oracle: positions are marked at a stale price and nothing says so. The stock option does nothing in this SDK version, so the timeout is set where requests read it | An SDK that honours `timeout`; a liveness alert on `oracle_time_ms` |
| DEC-103 | **M0.7 (H-15).** The perps terminal runs on the stream and the lane's own price history:
- **`tickers`:** a stream subscriber that sets `tickers: true` gets one message per block, `{type: "tickers", height, timestamp_ms, markets: [{market_id, oracle_price, oracle_time_ms, best_bid, best_ask, open_interest_lots}]}`, before its `fill`/`book`/`receipt`/`account` messages. The web app polled `/v1/markets` every 2 s; it now patches prices and the block height from this message and polls only as a fallback (15 s while the stream is up, 2 s while it is down).
- **Candles:** the perps node indexes every accepted oracle update (`Event::Oracle {accepted: true}` in a block's receipts) into one-minute candles of the oracle price per market, which is the mark price, and keeps 7 days. `GET /v1/markets/{id}/candles?interval=1m|5m|15m|1h&limit=` (default 500, at most 5,000) serves them oldest first, the last one open; 5m, 15m and 1h are built from the minutes. Prices are stroops per lot, like every price.
- **Kept across restarts:** candles and the last 1,000 fills per market go to `perps-history.sqlite` beside the node's store, written once a minute with the height they reach. At start the node loads it and replays the stored blocks after that height (at most 200,000, about a day at 0.5 s) through the same indexing code, by a new `NodeApp::warm` hook (platform, default no-op). Before this, `/v1/markets/{id}/trades` came back empty after every restart. The file is display data, not consensus; deleting it rebuilds the last day from the store.
- **Relayer:** the loop sleeps the rest of its interval after a step, not the whole interval, so a 500 ms feed publishes every 500 ms; the oracle feeder fetches its markets in parallel, so one slow source does not hold up the others.
- **Web:** a candlestick chart (lightweight-charts 5.2.1) with 1m/5m/15m/1h, history on open, the last candle moving with every ticker; new trades flash in the tape; the status bar shows Live while the stream is up and Reconnecting when it is not. | The page polled every 2 s and the chart started empty, which is not how a trading venue feels. Every number still comes from the lane: candles are the oracle prices the lane accepted, not an outside feed | A candle store that outlives 7 days, or OHLC of fills |
| DEC-104 | **M0.7 (H-16).** Lane #1 runs 0.5 s blocks: `[node] block_time_ms = 500`, `checkpoint_every_blocks = 120`, and the relayer's oracle feed `intervalMs = 500`. Both node settings are outside the genesis config (§10.1), so the lane, its config hash and its contracts do not change; apply rewrites the node files and restarts the nodes.
- **Cost:** the relayer pays about 0.30 XLM fixed per checkpoint plus about 0.0005 XLM per KB of batch (RESULTS.md: 0.306 XLM at 10 KB, 0.341 at 87 KB). An empty block adds 129 bytes to a batch (a 93-byte header, a 4-byte length and its 32-byte state hash), so 120 blocks a minute add about 7.7 KB per checkpoint, about 0.004 XLM. Expected: about 447 XLM a day idle instead of about 441 (+1–2%). Keeping 60 blocks per checkpoint would have doubled it. These measured figures replace DEC-044's local-network estimate (0.067 XLM per checkpoint).
- **What gets faster:** a price reaches a block every 0.5 s (one oracle update per market per block is a consensus rule), and soft confirmation of an order takes about half a block less. Withdrawals still wait for the next checkpoint, about a minute.
- **What it costs the VM:** every block is executed by the sequencer and three validators, so idle CPU roughly doubles (about 13% of the e2-small measured on 2026-10-03, so about 25%), and blocks are stored twice as often. Under load the per-block caps are unchanged, so capacity per second doubles.
- **Nothing else counts blocks:** the validators' 5 s clock check, the oracle's 30 s staleness and 5 s future limits, the settlement contract's 60 s timestamp rule, the escape and force-inclusion windows, funding, the 200 ms validator poll and the relayer's intervals are all in wall-clock or ledger time.
- **Rollback:** set the three values back and apply. | The human chose 0.5 s with a checkpoint a minute: the lane should feel as fast as it can without the bill moving | VM CPU above about 40%, or the relayer fee per checkpoint rising |
| DEC-105 | **M0.7 (H-17).** Store indexes and pruning, on every node (the sequencer and validators share `caravel-runtime::store::Store`):
- **Index:** `checkpoints_status_seq ON checkpoints (status, seq)`, created at open. Before it, the query that picks signed checkpoints ran before every block and scanned the whole table, as did the signer loop's every 2 s, the three `last_checkpoint_with` reads behind each `/v1/status` call and the validators' acceptance updates. A store from an earlier release gains it, and the `batch_len` column, when it opens (idempotent).
- **Narrow reads:** `/v1/proofs/withdrawals` and `export-proofs` read `seq, header, withdrawals` of accepted checkpoints (`Store::accepted_withdrawals`), not the batch bytes of every checkpoint.
- **Pruning** (`Store::prune`, every 30 s, at most 200 rows of each kind per pass so the block loop never waits long):
  - snapshots older than the last accepted checkpoint minus 3, except genesis. The escape tree (`/v1/proofs/escape`, `export-proofs`) reads the last accepted one; signing reads none of the older ones;
  - the batch bytes of accepted checkpoints older than that. A batch is on Stellar, its hash is in the header, and `checkpoint::assemble` rebuilds it from `blocks`; `batch_len` keeps its size, so `/v1/checkpoints/{seq}` still reports `batch_bytes`;
  - never pruned: blocks (validator catch-up, `check-store`, the perps history, and the lane's only history past Stellar RPC's 7-day window; since DEC-121 the sequencer archives old ones and validators drop them), checkpoint headers, withdrawal leaves (withdrawals never expire on chain), the inbox, `head`, `signed`, `flags`.
- **Space:** freed pages are reused, so a store stops growing for these tables. `caravel-perps-node compact --config <sequencer or validator toml>` prunes a stopped node's store to the end and runs `VACUUM`.
- Amends §14.4 ("a state snapshot at every checkpoint") and §15, by the human's call (2026-10-03). | Lane #1 kept about 5,700 full snapshots and every batch twice per store, four stores on one VM, and queries on every block grew with them | Blocks themselves, once a ledger archive or snapshot sync replaces them as history (M1) |
| DEC-106 | **M0.7 (H-18).** The perps app's other pages follow the H-13 terminal, and a user gets testnet USDC without leaving it.
- **Get test USDC:** the lane settles in Circle's testnet USDC, so a new user needs some. The app buys it on Stellar's testnet DEX instead of minting a token of its own (which would need a new settlement contract and lane): friendbot funds a new account with 10,000 XLM; then one transaction, signed once in the wallet, adds the USDC trustline if missing and makes a strict-receive path payment of exactly 100, 500 or 1,000 USDC for at most the Horizon quote plus 2%; the deposit form opens with what arrived. Checked 2026-10-04: 500 USDC cost about 529 XLM; a throwaway account went from friendbot to 500 USDC in about 10 s. The Circle faucet stays as the fallback. Horizon and friendbot URLs are config (`horizonUrl`, `friendbotUrl`; empty turns the flow off), testnet defaults only.
- **Portfolio:** a header (equity, unrealized PnL, margin ratio, ready to claim), a balance sheet of where the USDC is, each row marked soft or settled, tabs for positions, orders and claims, and one action panel: Get test USDC, Deposit, Withdraw.
- **Explorer:** the stream's `block` messages feed a live list (gaps between the first fetch and the stream are fetched once); a pipeline shows the latest block, sealed, signed and accepted checkpoints with ages; search takes a height, `c<seq>` or a G… account; details open inline.
- **Exit** (route `/escape`, always in the nav): the contract's two freeze conditions as timers, from `config().params` and `last_checkpoint().accepted_at` and the oldest unprocessed inbox message's `enqueued_at`; a Freeze button appears only once one holds; the escape proof can be fetched and downloaded while the lane runs; the three ways out; the claim and refund flow when frozen.
- **How it works** (route `/about`): a diagram of wallet, sequencer, validators and the settlement contract, live figures, a table of what is trusted and how to check it, what is not claimed (§2), and the contracts with copy buttons.
- **Shell:** a wallet menu (copy, stellar.expert, fast-trading state, disconnect); on phones the nav takes its own row. | The human asked for the whole app at the Trade page's standard and for a way to fund an account; the DEX already has the depth, so no new token or faucet service is needed | Testnet DEX liquidity drying up (then a faucet of our own) |
| DEC-107 | **M0.7 (H-19).** A withdrawal is one flow, from the lane to the wallet. After `withdraw` is queued, the app keeps the amount and time as pending (in the browser, per account, dropped after 15 minutes) and shows its three steps: left the lane, waiting for the next checkpoint on Stellar, claim. It polls the account's withdrawal proofs every 4 s while one is pending (every 15 s otherwise); when a new claimable leaf of the same amount appears, the pending entry becomes a Claim button. Claimable withdrawals show everywhere the user is: a Claim button in the top bar, a line under the Trade page's account, and a tracker above Portfolio's actions. "Claim" claims every ready withdrawal in turn, one wallet signature each (a Soroban transaction holds one contract call). Portfolio's separate Claims tab is gone. | The human withdrew and then could not find how to claim: the claim lived in a tab | A contract call that claims several leaves at once |
| DEC-111 | **M0.8 (D-01).** A release ships container images, built from the release itself, never recompiled, so an image runs the same bytes as the tarball:
- `docker/node.Dockerfile` (`TEMPLATE` arg): `debian:trixie-slim` (glibc 2.41, newer than both build hosts: Ubuntu 22.04's 2.35 for tagged releases and 24.04's 2.39 for main), `ca-certificates`, the template's node binary and engine Wasm under `/opt/caravel`, user 10001, entrypoint the node binary. Images `caravel-perps-node`, `caravel-payments-node`.
- `docker/relayer.Dockerfile`: `node:22.23.3-trixie-slim` (the VM's Node), the relayer with its production `node_modules` and every template's feed modules, user 10001. Image `caravel-relayer`.
- `docker/web.Dockerfile`: `caddy:2.11.6` with a template's web app at `/opt/caravel/web`; the Caddyfile and `web.json` are mounted at run time. Image `caravel-<template>-web`.
- Base images are pinned by digest in `versions.json` (`images`), and `check-versions` fails a Dockerfile whose `FROM` differs.
- `scripts/build-images.sh` builds them for this machine (tags, `caravel-*:<commit tag>`), or with `--push --registry` for one or more architectures (`amd64=<release> arm64=<release>`): each architecture is pushed by digest, then joined into one image with `buildx imagetools create`. It writes `IMAGES` into each release: one `<role> <ref>` line per image (`perps-node`, `relayer`, `perps-web`, …) plus the pinned `caddy` for lanes without a web app.
- CI: on main the release job pushes `ghcr.io/<owner>/caravel-*:sha-<12>` (amd64, lane #1's architecture) and puts `IMAGES` in the release artifact; on a `v*` tag `release.yml` pushes `:v*` for amd64 and arm64 from the Linux archives and attaches `IMAGES` to the draft; a pull request that touches the images builds and smoke-tests them without pushing. `install.sh` keeps a release's `IMAGES` next to it, and `Release` reads it (`release.rs`, `parse_images`).
- A smoke test on every build: the node image prints its version and computes lane #1's genesis (`f4b9db09…`, checked 2026-10-04 on the amd64 image under emulation). | Images that rebuild from source would be other bytes than the release of record (DEC-033). A digest pins exactly what runs | Signing the images (cosign or attestations), or a smaller relayer image (the Stellar SDK is in it twice, about 70 MB each) |
| DEC-112 | **M0.8 (D-02).** `[env.<name>.host] runtime = "process" | "systemd" | "docker"`; the defaults stay `process` for `local` and `systemd` for `ssh`, and `local`+`systemd` or `ssh`+`process` is refused. With `docker`:
- **Provider:** `caravel-deploy/src/docker.rs`, over a transport of its own: shell scripts run here (`bash -c`) or on the host (`Ssh::exec`, ssh or gcloud-iap). The `process` and `systemd` code is untouched; splitting their transport out too is left for when it pays.
- **Files:** the same layout under the host's root (`config/`, `keys/` 700, `data/` 700, `run/`), plus `config/compose.yml` and, with a `public_url`, `config/Caddyfile`. Node configs name the root the containers mount, `/opt/caravel` (`render::RUN_ROOT`); the compose mounts map the host's root onto it. Nodes listen on `0.0.0.0`, reach each other by service name (`http://sequencer:8080`), and publish their port on `127.0.0.1` (and the host's private address when another host calls them). A local network's RPC (`localhost`) becomes `host.docker.internal`, with `extra_hosts: host-gateway` so it works on Linux too.
- **Services:** `sequencer`, `validator-<n>`, `relayer` from the release's images (`IMAGES`), each `read_only`, `tmpfs /tmp`, `no-new-privileges`, `cap_drop: ALL`, `restart: unless-stopped`, as `${CARAVEL_UID:-10001}:${CARAVEL_GID:-10001}` (uid 10001 on an ssh host, where the tool writes the files as that owner; the user's own uid here). Each validator mounts only its own key; the sequencer and relayer read `keys/env`. With a public URL, `web` (the template's web image, else the pinned Caddy) publishes 80/443 with only `NET_BIND_SERVICE`, keeps its certificates in `caddy/`, and proxies to the service names; it is recreated when the compose file or Caddyfile changes. A docker lane with a public URL can't use a namespace (one Caddy per host's ports).
- **Lifecycle:** release = record `COMMIT` and pull the images (a local build's tags are checked instead); start = `compose up -d --no-deps --force-recreate <node>` and `run/<node>.started`; stop = remove the node's containers by Compose labels, also one the file no longer names; `export-proofs` = `compose run --rm <validator>`; logs = `compose logs`; status = `curl` on the published loopback port. The plan's rules don't change: a node restarts when its configs or the release change, and the read reports running containers, fingerprints, files, data, the owner and listeners.
- **Checked 2026-10-04** on macOS with Docker Desktop and main's amd64 images under emulation: `caravel apply` of the payments local lane with `runtime = "docker"`, then account create, deposit, transfer, withdraw (claimed through checkpoint 10), `logs`, `plan` (no changes), `destroy` (exits exported in a one-off container, contract frozen), `escape` (40 of 40 paid) and `destroy --stop-validators`. Tests: lane #1's files under docker (`render-testnet-docker/` golden, `docker compose config` accepts the compose file), manifest rules, the image ref check. | The human chose containers so a lane runs the same way on a laptop and on a VM. Keeping the configs, plan and fingerprints identical means the three runtimes can't drift apart | The `process`/`systemd` transport split; a lane with a public URL sharing a host |
| DEC-113 | **M0.8 (D-03).** Lanes in containers on a laptop:
- **`caravel init`** writes `runtime = "docker"` into the local deployment when the installed release ships the template's images (`IMAGES`), else processes as before; `--runtime docker|process` chooses. The release of 0.1.0 has none, so its users keep processes and Node.js until the first release with images.
- **`caravel doctor`** checks Docker with Compose for container lanes and Node.js 22 only for process lanes. `install.sh` says which one a release needs.
- **The local Stellar network** runs a pinned quickstart, `stellar/quickstart:v672-b1475.1-latest` (what `latest` was on 2026-10-04, pushed 2026-09-29, digest in `versions.json` `images.stellar_quickstart`), through `stellar container start local --image-tag-override`, instead of whatever `latest` is that day.
- **The e2e** takes `E2E_RUNTIME=docker`: on Linux it assembles a release from the checkout and builds its images (`build-images.sh`); elsewhere it needs `E2E_RELEASE_DIR`, a Linux release with `IMAGES` (a macOS checkout can't build Linux binaries). CI's e2e runs both templates in both runtimes. Its rotation check now reads the sequencer's log from a file: `grep -q` closing a streamed `docker compose logs` early failed the pipe.
- **Checked 2026-10-04 on macOS** (main's amd64 images under emulation): `E2E_TEMPLATE=payments E2E_RUNTIME=docker` passed every step in 203 s (deposits, transfer, checkpoint, withdraw, the validator rotation with checkpoints 6 and 7 signed again under epoch 2, the forced withdrawal, destroy, both escapes, replay of 10 checkpoints from Stellar); the process run still passes (184 s).
- **Docs:** a "Lanes in containers" guide, the containers section of the lane-file reference, and the install requirements. | A laptop then needs Docker and the CLI, the same images run on the laptop and on a server, and the quickstart image no longer changes under the tests | A Linux builder for macOS checkouts (until then, CI artifacts or a published release); the first release with images |
| DEC-114 | **M0.8 (D-04).** Machines as code with OpenTofu, under `infra/opentofu/` (the only place the IaC copy rule's word may appear: its syntax and file names need it; CI's grep skips that directory, and its docs and comments still say "infrastructure as code"). Caravel still makes no machines (§2.4): these modules do, and hand their result to the lane file.
- **`modules/caravel-host-gcp`:** a VM (Ubuntu 24.04, e2-small, 20 GB pd-standard, shielded boot, no service account, OS Login), a static premium address, a firewall rule for 80/443 to the VM's network tag and one for ssh from IAP's range (`35.235.240.0/20`). A later image release never recreates the VM (`ignore_changes` on the boot image). `startup.sh` (the VM's startup script, safe to run again) installs Docker Engine and the Compose plugin from Docker's apt repository at pinned versions, checking the repository key's fingerprint, makes `/opt/caravel` for uid 10001, and adds 1 GB of swap. Output `public_url` is the address's `sslip.io` name.
- **`modules/billing-cap`:** DEC-045's cap as resources: the APIs, the Pub/Sub topic, the service account with its two roles, the budget (thresholds, `CURRENT_SPEND`, all credits), the gen2 function (internal ingress, one instance, no retries) and the invoker binding. It zips and uploads `infra/gcp/billing-cap` to a bucket, or uses an archive already in Cloud Storage (`existing_source`, lane #1's, which gcloud uploaded).
- **`envs/caravel-testnet`:** lane #1's project, with state in the GCS bucket `caravel-testnet-tofu-state` (versioned, uniform access, public access prevented, `prevent_destroy`), which `bootstrap-state.sh` makes once with gcloud and the root then imports and manages; the script also turns on the Resource Manager and Service Usage APIs the provider reads a project with. The root names lane #1's resources as they were made by hand (`caravel-ip`, `allow-web`, `allow-ssh-from-iap` on every VM) and leaves `install_docker` off until D-06. The google provider bills the budget API's quota to the project (`user_project_override`).
- **The hand-off:** output `caravel_vars` is TOML (`host_address`, `host_project`, `host_zone`, `public_url`) for `caravel --var-file`. Lane #1's lane file now reads its host from those vars, whose defaults are today's VM, so its plan doesn't change without the file.
- **Pins:** OpenTofu 1.13.1 (`required_version = "~> 1.13.1"`, CI downloads it checked against the release's SHA256SUMS), providers `hashicorp/google` 8.5.0 and `hashicorp/archive` 2.8.1 (exact, with a committed lock file for linux and darwin, amd64 and arm64), Docker `5:29.8.2`, Compose `5.6.0`, containerd `2.3.6` (`versions.json` `opentofu`, `docker_host`); `check-versions` compares every `.tf`, the lock file, CI and `startup.sh` with them.
- **CI:** the `infra` job runs `tofu fmt -check` and, for every root and module, `init -backend=false` (the lock file read-only for roots) and `validate`, offline.
- **Checked 2026-10-04:** a trial `tofu plan` with a local backend against the live project imported the VM, address and both firewall rules with nothing to change in GCP (one state-only setting, `allow_stopping_for_update`); the rest waited for the Resource Manager API. **D-05, 2026-10-04,** with the human's OK: `bootstrap-state.sh` turned on the APIs and made the bucket; `imports.tf` adopted all 21 resources (the VM, address, two rules, nine APIs, topic, service account, two roles, budget, function, invoker binding, state bucket), `apply` imported them and changed only that state flag, and the next `tofu plan` said **No changes**; `caravel plan` for lane #1 with `--var-file` from `caravel_vars` also said **No changes**; the lane kept producing blocks throughout. | OpenTofu is the open-source, widely used tool for machines; Caravel stays the layer for what's above them (contracts, keys, nodes, releases) and takes the machines as vars | Modules for other clouds; the function's source uploaded by OpenTofu for lane #1 too |
| DEC-115 | **M0.8 (D-06).** Lane #1 runs as containers since 2026-10-04, with the human's OK. The lane file says `runtime = "docker"`; the render goldens keep the systemd view pinned (the file without that line) next to the docker one.
- **Steps:** (1) with the lane up, Docker 29.8.2 and Compose 5.6.0 from the host module's `startup.sh` (its Docker part) and the release's images pulled; (2) `systemctl disable --now` the five caravel units and the host's Caddy (still installed), `/opt/caravel` config, data, keys and run given to uid 10001, Caddy's certificate copied into the web container's data dir so HTTPS didn't wait for a new one; (3) `caravel apply` with main's release (4c2839e; no node, relayer or web code changed since c7532cc) wrote the compose file, Caddyfile and configs and started the containers.
- **Downtime:** 10:24:21 to about 10:28:35 UTC (4 min), most of it two IAP ssh round trips and the apply's per-node health waits; the plan had said 1 to 2 minutes.
- **Checked:** blocks at 0.5 s from height 403,001, checkpoints 6551 to 6553 accepted on Stellar after the switch, oracle prices moving, `/validators/1..3` and the web app over HTTPS with the same Let's Encrypt certificate, six containers using about 105 MB together (the relayer 57 MB, each node about 10 MB), and `caravel plan` with the `caravel_vars` file said **No changes**.
- **Rollback:** RUNBOOK §3.4 (compose down, ownership back to `caravel`, Caddy on, runtime line removed, apply). | The human chose one way to run a lane on a laptop and on a VM; lane #1 now runs the same images CI pushes | Removing the disabled units and the host's Caddy once the containers have run for a while (done the same day, D-09) |
| DEC-116 | **M0.8 (D-08).** `scripts/build-linux-release.sh <out>` makes a Linux release on any machine with Docker: `cargo build --release --locked` of `caravel-cli` and each template's node in `rust:1.93.0-trixie` (pinned by digest, `versions.json` `images.rust_builder`; `check-versions` requires the toolchain of record), as the calling user, with the target and cargo's cache under `target/linux-<arch>/`, and `RUSTUP_TOOLCHAIN` set so rustup doesn't install `rust-toolchain.toml`'s Wasm target into the image. The architecture is the Docker daemon's (arm64 on Apple silicon, native speed). Everything else comes from this checkout's builds, which don't depend on the platform: the contracts, and the relayer, feeds and web app (their production dependencies are plain JS). `assemble-release.sh` takes the binaries from `CARAVEL_BIN_DIR`, and now resolves its output to a real path, because `npm ci` refused a prefix reached through macOS's `/var` symlink. `E2E_RUNTIME=docker` uses the builder off Linux instead of requiring `E2E_RELEASE_DIR`. **Checked 2026-10-04 on macOS (arm64):** a first build in 3 min 41 s; `E2E_TEMPLATE=payments E2E_RUNTIME=docker` passed every step from a clean checkout build. | A Mac developer can try their own changes in containers without CI | Cross-building amd64 on arm64 (slow under emulation) |
| DEC-117 | **M0.8 (D-11).** CI's first job, `changes`, diffs the change (a pull request against its base, a push against `before`; a manual run or a new branch counts as everything) and sets three outputs. **code** is false only when every file is docs-like (`docs/`, `docs-site/`, `site/`, `infra/opentofu/`, `.claude/`, `LICENSE*`, any `.md` outside `lanes/perps/engine`); `rust`, `e2e`, `quickstart` and `release` need it. **apps** lists the npm apps whose directories changed (the relayer brings its feed module along), and feeds the `apps` matrix, skipped when empty. **infra** gates the OpenTofu job. A change to `versions.json` or `ci.yml` runs everything. `versions` (pins, frozen engine, dependency rule, copy rules) always runs. `main` has no branch protection, so skipped jobs block nothing. GitHub's own `[skip ci]` in a commit message skips the workflow; `CONTRIBUTING.md` limits it to changes that can't break anything. | A docs or site change waited about 10 minutes for the Rust job on a pull request, and a docs merge ran four e2e runs and a release | Required status checks, if branch protection comes (skipped jobs count as passing) |
| DEC-118 | **M0.8 (D-15).** `install.sh` puts `$PREFIX/bin` on PATH itself, the way rustup does, instead of asking the user to `export` it. Unless the directory is already on PATH, it writes `$PREFIX/env` (a `case` guard that prepends the directory once) and appends `. "$PREFIX/env"` to `~/.profile`, to `~/.bashrc` and `~/.bash_profile` when they exist, and to zsh's `${ZDOTDIR:-~}/.zshenv` when the user has zsh; for fish it writes `~/.config/fish/conf.d/caravel.fish` (`fish_add_path --prepend`). Each line is added once, so a second install changes nothing. A piped installer can't change the shell that ran it, so it ends with `. "$PREFIX/env"` for that one. `--no-modify-path` or `CARAVEL_NO_MODIFY_PATH=1` touches nothing outside `PREFIX` and prints the `export` line; CI's installs into temp prefixes pass it. **Checked 2026-10-04** with a scratch `HOME` under macOS's bash 3.2: a new zsh and a bash login shell find `caravel`, a second run adds nothing, the opt-out and an already-set PATH leave `HOME` alone. | Every widely used installer (rustup, bun, deno, foundryup) sets PATH; an extra manual step loses people at the first command | — |
| DEC-119 | **M0.9 (F-04).** CI runs the Rust gates as four parallel jobs, `lint`, `test`, `engine` and `parity`, through one composite setup (`.github/actions/rust-setup`: toolchain, cargo cache, Node, the Stellar CLI and cargo-nextest when asked).
- **Cache:** the cache covers both workspaces (`.` and `lanes/perps/engine`, whose `target/` was never cached before and was rebuilt from scratch each run), keyed per job role, and only `main` saves; release.yml never saves (its tag and pull-request caches can't be restored elsewhere and pushed the repo past the 10 GB cache limit).
- **Disk cleanup:** the step is gone. The runner had 87 GB free before it ran, and debug info is off.
- **Parity gates:** both run at once (`cargo nextest run --run-ignored only -E 'binary(parity)'`). The perps gate spreads its 50 independent lanes over every core, one `Dual` per thread, the same 10,050 blocks: 5.2 s instead of serial locally (15 cores). The payments gate's one 10,000-block lane becomes 16 seeded lanes of 625 blocks, on every core: 5.1 s locally (82 s serial in CI).
- **Tests:** they run under nextest 0.9.146 (pinned in `versions.json`, checked), and doc tests run separately.
- **Local builds:** the root `[profile.dev]` keeps line tables only (the contracts build with the release profile, and their hashes are unchanged).
- **Main:** the release job's build is the e2e's input (`E2E_RELEASE_DIR`; the e2e then builds nothing), instead of four separate LTO builds. `cancel-in-progress` applies to pull requests only, so every main merge gets a finished run. | A code PR took ~10 min with the Rust gates in one serial job; the gates are unchanged, only how they run | Skipping the frozen engine's tests when nothing it depends on changed (a gate policy change for the human) |
| DEC-120 | **M0.9 (F-07): a lazy head.** A node writes its head (the full state) at every `CHECKPOINT_END`, where the snapshot already holds it, and every `10,000 / block_time_ms` blocks (20 at 500 ms, 50 at 200 ms), instead of with every block. Block rows are still written per block. On open, the sequencer core and the validator follower re-execute the blocks after the head through their executor, check each against its `state_hash_after`, and write the head at the tip (`resume`). A mismatch is a corrupt store and the node stops. `check-store` re-executes to the last block and checks the head on the way. **Durability:** validators commit blocks with `synchronous=NORMAL`, which in WAL mode survives a crash of the node and may lose the last blocks to a power loss; they fetch those again. Their `signed` row, the equivocation guard, is written with FULL before the signature leaves. **The sequencer keeps FULL for blocks**, unlike the F-07 plan: a block it has served must survive a power loss, or after one it could produce another block at the same height, and validators that followed the first would see a fork. Checkpoint status changes happen on the sequencer, so they stay FULL too | The state blob was rewritten every block, which is ~4.8 KB on lane #1 and up to ~85 KB at full caps, or ~1.7 MB/s of WAL at 200 ms. The cost of a restart is at most ~10 s of blocks re-executed (~11 ms each for the perps engine), and nothing that left a node can be forgotten | — |
| DEC-121 | **M0.9 (F-08): block history.** `Store::prune_history` runs after `prune` on every pass (8 checkpoints a pass; `compact` does the backlog at once). It handles accepted checkpoints up to and including the oldest one whose snapshot `prune` keeps (last accepted − 3), so the blocks left always start right after a kept snapshot.
- **The sequencer archives:** each checkpoint's blocks become one row of `block_archive (seq, first_height, last_height, blob)`. The blob is a version byte (1), then a deflate stream (`miniz_oxide` 0.9.1, level 6, pure Rust, MIT OR Zlib OR Apache-2.0) of each block as `u32 LE` input length, input, `state_hash_after`, `u32 LE` receipts length, receipts. It is a node-local format, never hashed or signed. `Store::block` and `Store::blocks` read rows and archives alike, with the last unpacked archive cached, so `/v1/blocks/{h}`, validator catch-up, `check-store`, the perps history and every other reader are unchanged.
- **Validators drop:** `/v1/blocks/{h}` below `Store::first_block` answers `410 PRUNED` ("ask the sequencer, or replay from Stellar"). `check-store` starts from the kept snapshot the first block follows (`from_height` in its report). Escape and withdrawal proofs read snapshots and checkpoint rows, which are unchanged.
- **zstd was the plan; deflate it is:** on soak blocks zstd -3 gave 2.44× and deflate 2.4×, because signatures and keys do not compress. The `zstd` crate binds C under BSD-3-Clause, and `miniz_oxide` is pure Rust (checked with `cargo info`, 2026-10-04).
- Amends §15 and DEC-105 ("blocks are never pruned"), by the human's call (2026-10-04). | Four full copies of every block on one disk, ~485 MB a day at 200 ms. Three of the four stores now stay flat, and the sequencer's blocks take ~40% of their size | — |
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
| OQ-009 | C-23: once nodes run on several hosts, what protects the sequencer's call to each validator's `/v1/sign`, and who may run a validator? | **Answered 2026-10-03.** Every call is authenticated, as the zero-trust tools do; the network isn't the boundary. **(1) Signed requests:** the sequencer gets its own identity (`[env.X.sequencer] key`), signs each `/v1/sign` request (ed25519 over the time and the body's hash), and validators check it against that key from the lane file, within a time window. No token, no CA and no state file. **(2) A private network is optional:** `private_address` per host carries node-to-node traffic when declared; otherwise traffic goes over public HTTPS. It's defence in depth, not the boundary. **(3) External validators are in C-23:** a validator run elsewhere is a reference (its URL and key), put in the signer set and called by the sequencer, never deployed. `/internal/*` stays on localhost (the relayer shares the sequencer's host) |
| OQ-010 | H-06: how should a contracts lane use Groundhog? H-05 found (2026-10-03, sources in `docs/SOURCES.md`):<br>- **What it is:** Groundhog is a research execution engine by Geoffrey Ramseyer and David Mazières (Stanford, arXiv 2404.03201, April 2024). It is about 30k lines of C++ running arbitrary Wasm with its own host API, not Soroban. Transactions in a block are unordered, and typed commutative state (int64 adds, byte strings, sets) resolves conflicts deterministically. It is consensus-agnostic and claims more than 500k payments/s on 96 cores. The OSDI '25 DeCl work makes contracts native code.<br>- **Its code:** `scslab/smart-contract-scalability` (Apache-2.0) has an empty README, no releases or docs, and no activity after July 2025. Two submodules have no detected license.<br>- **Mentors:** only its authors appear able to mentor it.<br>- **The conflict:** using it as a lane's executor breaks DEC-002 (consensus runs the engine Wasm through `soroban-env-host`) and INV-P5. Cross-architecture agreement on one state root isn't shown, and hackers' Soroban contracts wouldn't run on it. A lane's throughput is bound by the data posted to Stellar, so its multicore speed may not show.<br>**Questions:**<br>(1) Will Tyler or the authors commit to mentoring it, and to a supported branch?<br>(2) Is there a newer, non-public version at SDF?<br>(3) May the contracts template take an exception to DEC-002, or should Groundhog's model be emulated inside a Soroban engine?<br>(4) Which architectures must validators agree across? | **Default:** H-07 builds the contracts lane on Soroban, under DEC-002, so hackers deploy ordinary Soroban contracts. Groundhog becomes an experimental, clearly labelled executor (H-08) only if its authors commit to it. The copy never claims Groundhog support before then **On hold (the human, 2026-10-03):** Groundhog and the contracts lane (H-06 to H-08) wait; nothing is built until the human picks them up again. |

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
