# Caravel: agent instructions

Caravel deploys and runs configurable appchains ("lanes") that settle to Stellar in a token of their choosing (DEC-072), declared in one lane file (`caravel plan / apply / status / destroy`). The first lane is Caravel Perps, an on-chain perpetual futures exchange; Payments is the second template. Current milestone: **M0.8, containers and machines as code** (spec §20.7): images per release, a `docker` runtime for `caravel apply`, OpenTofu for machines. Before it, **M0.7, ready for HackMeridian** (spec §20.6): the landing page says who it is for, prebuilt binaries and a short path to a first lane, a contracts lane template that can run Groundhog, and a starter kit for Frankenstack teams. Before it came M0.6 (a real CLI and the lane file language, §20.5, done 2026-10-03) and M0.5 (the platform split and declarative lanes, §20.3).

## Read first

- `docs/CARAVEL_SPEC.md` is the source of truth. Before any task, read §0 (rules), §1, §4 and the task's `Reads:` sections in §20.
- If the spec is silent or wrong, stop and ask (§0.4). Do not guess on consensus code.

## Non-negotiables

1. Deterministic replay is the product invariant. No floats, time, randomness or I/O in consensus code (§8).
2. Consensus runs the exact engine Wasm through `soroban-env-host` (DEC-002). The native build is for tests and must match byte for byte (INV-P5).
3. Never invent APIs. Check docs.rs / npm docs for the exact versions in `versions.json`. `[VERIFY]` items in the spec must be checked before use. Check Stellar facts (protocol version, passphrases, RPC URLs, contract IDs, resource limits, SEPs/CAPs) through the stellar-raven MCP, and log the source and date in `docs/SOURCES.md`.
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

## Commands

Toolchain: Rust 1.93.0 + `wasm32v1-none` (from `rust-toolchain.toml`), Stellar CLI 28.1.0 (`cargo install --locked stellar-cli@28.1.0`), Node ≥ 22.

```sh
node scripts/check-versions.mjs         # versions.json pins, Cargo.lock/package-lock, placeholders (§7)
./scripts/build-contracts.sh            # builds Wasm with the pinned CLI, checks size and recorded hashes (DEC-020; hashes of record are x86_64 Linux builds from CI, DEC-033); run before the tests
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace --locked
cargo test --locked -p caravel-perps-node --test parity -- --ignored      # 10,000-block native/Wasm parity gate (INV-P5)
cargo test --locked -p caravel-payments-node --test parity -- --ignored   # the same gate for the payments engine (DEC-064)
./scripts/check-frozen.sh               # the perps engine of record is frozen under lanes/perps/engine (DEC-051)
node scripts/check-deps.mjs             # platform/ never depends on lanes/ (M0.5; listed exceptions shrink to none)
cargo test --workspace --locked --manifest-path lanes/perps/engine/Cargo.toml   # its vectors, scenarios and properties
cargo build --locked --manifest-path lanes/perps/engine/Cargo.toml -p caravel-types -p caravel-merkle -p caravel-perps --target wasm32v1-none   # consensus crates stay no_std
cargo build --locked -p caravel-core -p caravel-app-sdk -p caravel-payments --target wasm32v1-none   # and the platform's and payments'
BENCH_ACCOUNTS=256 BENCH_ORDERS_PER_SIDE=128 BENCH_BLOCK_BYTES=12000 cargo run --release -p caravel-perps-node --example bench_full_caps   # docs/BENCHMARKS.md
CARAVEL_INTERNAL_TOKEN=$(openssl rand -hex 16) cargo run --release -p caravel-perps-node -- sequencer --config lanes/perps/config/sequencer.local.toml   # local sequencer (local lane, DEC-037)
cargo run --release -p caravel-perps-node -- validator --config lanes/perps/config/validator-1.local.toml   # a validator (key in keys/, never in git)
cargo run --release -p caravel-perps-node -- check-store --config lanes/perps/config/sequencer.local.toml   # replay a node's store through the Wasm
cargo run --release -p caravel-perps-node -- compact --config <sequencer or validator toml>   # prune a stopped node's store and VACUUM it (DEC-105); nodes also prune as they run
cargo run --release -p caravel-perps-node -- export-proofs --config <validator.toml> [--out exit.json]   # every escape and withdrawal proof a validator holds (DEC-065)
DURATION=3600 TPS=50 ./scripts/soak-sequencer.sh   # T-007 soak: 1 s blocks, 50 tx/s, restart halfway
cargo run --release -p caravel-perps-node -- replay --rpc <url> --network-passphrase <p> --settlement C... --genesis-config lanes/perps/config/lane.<lane>.toml --engine-wasm target/contracts/perps_engine.wasm [--prove-escape G...]   # replay from Stellar only
cargo build --release -p caravel-cli -p caravel-payments-node && ./target/release/caravel plan -f lanes/payments/config/lane.caravel-payments.local.toml   # the CLI (DEC-066 to DEC-068, DEC-075): plan, apply, status, destroy, validate, env list, output, version, doctor; --json on all
./scripts/e2e-local.sh                  # the whole lifecycle with the caravel CLI alone, on a local network (DEC-068, DEC-086); E2E_TEMPLATE=payments, E2E_NETWORK=testnet
./scripts/check-e2e.sh                  # the e2e calls no stellar/curl/node -e/kill/sleep outside its cleanup (DEC-086)
./scripts/build-images.sh <release-dir> [--push --registry ghcr.io/<owner>]   # the release's container images and its IMAGES file (DEC-111); arch=dir pairs for multi-arch
./scripts/check-quickstart.sh           # install from the checkout, then the README quickstart as written (SKIP_BUILD=1 reuses builds)
cargo run --release -p caravel-payments-node -- genesis --config lanes/payments/config/lane.caravel-payments.local.toml   # any template's genesis hashes
TPS=20 DURATION=600 ./scripts/measure-testnet.sh   # §19.6 numbers on the live testnet lane (docs/RESULTS.md)
npm --prefix platform/relayer ci && npm --prefix platform/relayer test
npm --prefix lanes/perps/relayer-feeds ci && npm --prefix lanes/perps/relayer-feeds test   # the perps oracle feed module (DEC-053)
npm --prefix lanes/perps/relayer-feeds run build && npm --prefix platform/relayer run build && node platform/relayer/dist/main.js --config lanes/perps/config/relayer.local.json   # needs CARAVEL_INTERNAL_TOKEN, CARAVEL_RELAYER_SECRET, CARAVEL_ORACLE_SECRET (read by the feed module)
npm --prefix lanes/perps/web ci && npm --prefix lanes/perps/web test && npm --prefix lanes/perps/web run build
WASM_DIR=<CI contracts-wasm artifact> ./scripts/deploy-testnet.sh   # T-012: deploy contracts + witness (refuses to redeploy)
./target/release/caravel apply lanes/perps/config/lane.caravel-perps.testnet.toml --env testnet --release-dir <CI release artifact>   # lane #1's VM (DEC-070); plan --diff first
(cd site && vercel deploy --prod)       # landing page only, never from the repo root (DEC-019)
npm --prefix docs-site ci && npm --prefix docs-site test   # the docs site (DEC-100): typecheck and build, which fails on any broken link
npm --prefix docs-site start            # the docs with live reload; `npm --prefix docs-site run serve` serves the build
node scripts/gen-cli-docs.mjs && ./scripts/check-cli-docs.sh && ./scripts/check-docs.sh   # the CLI reference from `caravel help`, and the docs' copy rules
```

Git: one branch and PR per task (`t-0xx-short-name`, `p-0x-short-name` for M0.5, `c-0x-short-name` for M0.6, `h-0x-short-name` for M0.7). Inside a phase, PRs stack on the previous task's branch, and the human reviews at the phase gates (M0: T-003, T-006, T-011, T-016; M0.5: P-07, P-10, P-14, P-18; M0.6: C-05, C-10, C-14, C-17, C-21, C-25; M0.7: H-01, H-04, H-06, H-08, H-11).

## Layout

M0.5 splits the repo into the platform (Caravel) and its lanes (Caravel Perps first, then the Payments template), then turns the platform into a declarative deploy tool for lanes: one lane file with `[env.<name>]` tables; plan, apply, destroy (spec §20.3). Never write the name of the well-known IaC tool in copy or docs; say "declarative" or "infrastructure as code". Current state:
- `lanes/perps/engine/`: **frozen** nested workspace, the perps engine of record (DEC-051). Never edit it. It holds caravel-types, caravel-merkle, caravel-perps (engine logic), caravel-testkit (test-only: lane simulator, scenarios, `cargo gen-vectors`), contracts/perps-engine and test-vectors/.
- `platform/` (Caravel, app-agnostic; `scripts/check-deps.mjs` keeps it off `lanes/`):
  - `crates/caravel-core` (the formats every lane shares, read without the app, DEC-052);
  - `crates/caravel-harness` (test-only: a native lane on the SDK's test app, for platform tests such as settlement's, DEC-063);
  - `crates/caravel-app-sdk` (no_std: `AppGenesisV1`, the SDK state layout and the standard pipeline for any app engine, DEC-060, DEC-062; `testapp` feature);
  - `crates/caravel-runtime` (executor, store, sequencer and validator cores, checkpoints; any app through the `LaneApp` trait, DEC-053; formerly caravel-lane);
  - `crates/caravel-node` (library: sequencer / validator / replay / genesis / lane files for any app through `NodeApp`, DEC-053, DEC-054; each app's binary links it);
  - `crates/caravel-deploy` (the deploy library: a lane file's `[env.<name>]` deployments, derived addresses, the pure plan, the chain reader, node configs, the `local` and `ssh` providers, `plan`/`apply`/`status`/`destroy`, and templates in process or through their binary's plugin protocol; DEC-066 to DEC-069, DEC-073, DEC-074);
  - `crates/caravel-cli` (`caravel`, the one CLI for any template: lane-file and env discovery, `--json`, exit codes; DEC-075);
  - `contracts/settlement`;
  - `relayer/` (TS: inbox, checkpoints, metrics, and the host for an app's feed modules, DEC-053);
  - `test-vectors/` (the platform formats' golden vectors).
- `lanes/perps/` (Caravel Perps, lane #1): `engine/` (frozen, above), `node/` (caravel-perps-node, the lane #1 binary: `PerpsApp` and its `NodeApp`, the perps lane file, views, `tx`, `loadgen`, the golden trace and fixture store, the sequencer/validator/parity tests, format compatibility tests, `bench_full_caps`), `config/` (lane TOML files and local node configs), `deploy/testnet/` (the VM), `relayer-feeds/` (the oracle feed module the relayer loads), `web/` (the trading app).
- `lanes/payments/` (the Payments template, on the app SDK, DEC-064): `app/` (caravel-payments, no_std), `contracts/payments-engine`, `node/` (caravel-payments-node: `PaymentsApp`, the `[payments]` lane file, `tx`, scenarios, parity, API and vector tests), `config/` (the local lane file), `test-vectors/`.
- `scripts/`: shared build, check, deploy and e2e scripts; `infra/gcp/`: the billing cap.
- `docker/`: the container images a release ships (node per template, relayer, web), built from the assembled release, base images pinned by digest (M0.8, DEC-111).
- `site/`: the landing page (Vercel project `caravel`, https://caravel-tau.vercel.app).
- `docs-site/`: the docs (Docusaurus, its own Vercel project, DEC-100). Users' docs live here: the lane-file reference in `docs/reference/lane-file/`, the CLI reference generated from `caravel help` (never edit it by hand). `docs/` in the repo root stays the internal record (spec, sources, results, lane #1's runbook).
