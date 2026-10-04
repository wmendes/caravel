# Contributing to Caravel

Thanks for helping. Caravel's product invariant is deterministic replay: anyone must be able to rebuild a lane from Stellar data and get the same state hashes. Most of the rules below protect that.

## Before you start

- **Read the spec first.** [`docs/CARAVEL_SPEC.md`](docs/CARAVEL_SPEC.md) is the source of truth. Read §0 (rules), §1, §4, and the sections your change touches.
- **Ask when the spec is silent.** If the spec is silent or wrong, open an issue before writing consensus code.

## Rules

- **No non-determinism in consensus code.** No floats, time, randomness or I/O (spec §8).
- **The perps engine is frozen.** Never edit anything under `lanes/perps/engine` (`scripts/check-frozen.sh` enforces this).
- **Frozen binary formats need a version bump.** A change to a format in spec §9 also needs regenerated test vectors and a new decision in §22.
- **The platform stays app-agnostic.** `platform/` never depends on `lanes/` (`scripts/check-deps.mjs` enforces this).
- **Honest claims only.** Copy and docs follow spec §2. They never say "trustless", "audited", "production" or "mainnet".
- **Testnet only.** No mainnet passphrases or contract IDs in defaults.
- **No secrets in git.** Keys live in the Stellar CLI keystore, and lane files name them only as identities.

## Checks

Build the contracts first. Then run these before opening a pull request:

```sh
./scripts/build-contracts.sh
node scripts/check-versions.mjs
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
./scripts/check-frozen.sh
node scripts/check-deps.mjs
./scripts/check-e2e.sh
npm --prefix platform/relayer ci && npm --prefix platform/relayer test
```

**CLI or deploy changes:** also run `./scripts/e2e-local.sh` (both templates, `E2E_TEMPLATE=payments`) and `./scripts/check-quickstart.sh`. The e2e drives a lane with `caravel` and `jq` only.

**Engine changes:** also run the native/Wasm parity gates:

```sh
cargo test --locked -p caravel-perps-node --test parity -- --ignored
cargo test --locked -p caravel-payments-node --test parity -- --ignored
```

**Deploy tool changes:** also run the whole lifecycle through the CLI:

```sh
./scripts/e2e-local.sh
```

## What CI runs

CI looks at the files a change touches and runs only what they can affect:
- **Docs only** (`docs/`, `docs-site/`, `site/`, any `.md` outside the frozen engine): the version and copy checks, plus the docs site build when it changed. About a minute.
- **An npm app** (`platform/relayer`, `lanes/perps/relayer-feeds`, `lanes/perps/web`, `docs-site`): that app's tests and build. A relayer change tests the feed module too.
- **OpenTofu** (`infra/opentofu/`): its format and validate job.
- **Anything else is code:** the Rust build and tests, and on `main` the e2e runs, the quickstart and the release. A change to `versions.json` or the workflow runs everything.

For a commit that can't break anything, such as a typo in a comment or a spec wording fix, put `[skip ci]` in its message and CI doesn't start. Never use it for a change to code, scripts, versions or the workflow.

## Pull requests

Keep each pull request to one change. In its description, say:
- what changed;
- how you tested it;
- the open risks.

If a change departs from the spec, update the spec in the same pull request: record a decision in §22, and replace any `[VERIFY]` you checked with the checked value and its date.

## License

By contributing, you agree that your contributions are dual licensed under the MIT license and the Apache License 2.0, as described in the [README](README.md#license).
