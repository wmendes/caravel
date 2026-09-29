# Caravel: agent instructions

Caravel is a framework for configurable appchains ("lanes") that settle to Stellar in USDC. The first lane is Caravel Perps, an on-chain perpetual futures exchange. Current milestone: **M0, a testnet demo**.

## Read first

- `docs/CARAVEL_SPEC.md` is the source of truth. Before any task, read §0 (rules), §1, §4 and the task's `Reads:` sections in §20.
- If the spec is silent or wrong, stop and ask (§0.4). Do not guess on consensus code.

## Non-negotiables

1. Deterministic replay is the product invariant. No floats, time, randomness or I/O in consensus code (§8).
2. Consensus runs the exact engine Wasm through `soroban-env-host` (DEC-002). The native build is for tests and must match byte for byte (INV-P5).
3. Never invent APIs. Check docs.rs / npm docs for the exact versions in `versions.json`. `[VERIFY]` items in the spec must be checked before use.
4. Frozen binary formats (§9) change only with a version bump, regenerated `test-vectors/` and a new DEC in §22.
5. Honest claims only (§2). No "trustless", "audited", "mainnet", "validators on Stellar execute trades".
6. Testnet only. No mainnet passphrases or contract IDs in defaults.
7. Copy prior-art code only from MIT/Apache-2.0 files, keep headers, and log it in `docs/SOURCES.md`. Never copy SoroDOOM's GPL (PureDOOM) files.
8. No secrets in git.

## Workflow per task

1. Pick the lowest-numbered `todo` task in §20.1 whose dependencies are `done`. Set it to `doing`.
2. Write tests first (scenarios, vectors, negative cases) from the acceptance criteria.
3. Implement until the §19.1 gates pass locally.
4. Update the spec where reality differed:
   - implementation-level choices (libraries, API substitutes, module layout) get a new DEC in §22;
   - changes to frozen formats, invariants, settlement checks, the trust model or claims need the human first (§0.4);
   - replace `[VERIFY]` with the checked value and date.
5. Set the task to `review` and summarize in the PR: what changed, how it was tested, open risks.

## Commands (fill in during T-000)

```sh
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace --locked
./scripts/build-contracts.sh            # builds Wasm, checks size, prints sha256
./scripts/e2e-local.sh                  # quickstart + sequencer + 3 validators + relayer (after T-011)
npm --prefix apps/relayer test
npm --prefix apps/web test && npm --prefix apps/web run build
```

## Layout

- `crates/`: caravel-types, caravel-merkle, caravel-perps (engine logic), caravel-lane (executor, store), caravel-node (sequencer / validator / replay).
- `contracts/`: perps-engine, settlement.
- `apps/`: relayer (TS), web (React).
- `config/`: lane TOML files.
- `test-vectors/`: golden vectors shared by Rust and TS.
