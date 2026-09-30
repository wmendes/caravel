# Caravel

Declarative appchains on Stellar. A lane file says what a lane is (its app, rules and limits) and where it runs (its network, keys, validators and host). Four commands manage it:
- `caravel plan` shows what would change on Stellar and on the host;
- `caravel apply` makes it so;
- `caravel status` shows how it is doing;
- `caravel destroy` winds it down and leaves every user an exit proof.

**Testnet only.**

## What a lane is

- **Nodes.** A lane is a sequencer and its validators. They run the lane's engine, a Soroban Wasm, through `soroban-env-host`, and each validator re-executes every block itself.
- **Checkpoints.** Every checkpoint, with its block data, goes to the lane's settlement contract on Stellar. The contract accepts it only with the validators' threshold signature.
- **Funds.** USDC stays in that contract. Withdrawals are Merkle proofs against an accepted checkpoint.
- **Exit.** If the lane stops checkpointing, or ignores what users send through Stellar, anyone can freeze the contract. Users then take their last checkpointed balance back on Stellar.
- **Replay.** Anyone can rebuild the lane from Stellar data alone.

Two templates exist: **Caravel Perps**, a perpetual futures exchange (lane #1, live on testnet), and **Payments**, USDC transfers with a flat fee.

## Quickstart: a payments lane on your machine

You need:
- Rust 1.93 with `wasm32v1-none` (from `rust-toolchain.toml`);
- the Stellar CLI 28.1.0 (`cargo install --locked stellar-cli@28.1.0`);
- Node 22;
- Docker, for the local Stellar network.

```sh
./scripts/build-contracts.sh
cargo build --release -p caravel-deploy -p caravel-payments-node
npm --prefix platform/relayer ci && npm --prefix platform/relayer run build

# The lane file names keys as Stellar CLI identities; create them once.
for k in pay-local-admin pay-local-v1 pay-local-v2 pay-local-v3 pay-local-relayer; do stellar keys generate "$k"; done

L=lanes/payments/config/lane.caravel-payments.local.toml
./target/release/caravel plan    $L --env local      # changes nothing
./target/release/caravel apply   $L --env local      # starts a local network, deploys, runs the nodes
./target/release/caravel status  $L --env local
./target/release/caravel destroy $L --env local      # drain, export every exit, freeze
```

`plan` names the settlement contract's address before anything is sent: it is derived from the admin and the lane. A second `apply` changes nothing. After an interruption, running `apply` again finishes the job.

## The lane file

The lane's own sections (`[lane]`, `[app]`, `[limits]`, and the app's own table) define its genesis. Each `[env.<name>]` table is one deployment of it:

```toml
[env.local]
network = "local"                  # or "testnet"; mainnet is refused
admin = "pay-local-admin"          # a Stellar CLI identity; keys are never written here
usdc = "local"                     # or "circle" on testnet
threshold = 2

[env.local.settlement_params]
force_inclusion_window_secs = 20
escape_timeout_secs = 30
min_rotation_delay_secs = 3600
signer_retention_epochs = 2

[[env.local.validators]]
name = "1"
key = "pay-local-v1"
# ... one table per validator

[env.local.relayer]
account = "pay-local-relayer"

[env.local.host]
provider = "local"                 # or "ssh", for a Linux host you already have
```

- **Changes the plan applies:** a validator swap becomes a signer rotation, and a node setting becomes a restart.
- **Changes it refuses:** anything the settlement contract fixed at deploy time (the lane's rules, the engine, the admin, the params). For those, destroy the lane or give it a new name.

## On testnet

- **USDC:** `usdc = "circle"` uses Circle's testnet USDC.
- **Wasm:** pass `--wasm-dir <CI contracts-wasm artifact>` so the lane deploys the Wasm of record.
- **Hosts:** `provider = "ssh"` runs the nodes as systemd units on a host prepared with `lanes/perps/deploy/testnet/provision.sh`, reached over ssh or over `gcloud compute ssh --tunnel-through-iap`. Keys stream over the connection into mode-600 files.
- **Lane #1:** its deployment is `[env.testnet]` in `lanes/perps/config/lane.caravel-perps.testnet.toml`.
- **The full lifecycle:** `./scripts/e2e-local.sh` runs apply, user flows, a rotation from the file, destroy, escapes and replay, locally or with `E2E_NETWORK=testnet`.

## As a Stellar CLI plugin

```sh
ln -s "$PWD/target/release/caravel" ~/.local/bin/stellar-caravel
stellar caravel plan lanes/payments/config/lane.caravel-payments.local.toml --env local
```

## What this is not

- **It is not trustless.** If a threshold of validators collude with the sequencer, they can sign a wrong state. Replay detects this but can't prevent it.
- **Stellar validators don't run lane blocks.** They check signatures and store the data.
- **Lane blocks aren't Stellar transactions,** and a lane's block time is its own.
- **Not audited, not production, not mainnet.** On testnet the lane admin can still upgrade the settlement contract and rotate validators. Lane #1's three validators all run on one machine, operated by the Caravel team.

## Repository

- `platform/`: the formats, runtime, nodes, settlement contract, relayer and the deploy tool (`crates/caravel-deploy`).
- `lanes/perps/`: Caravel Perps (its frozen engine, node, web app and oracle feed).
- `lanes/payments/`: the Payments template.
- `docs/CARAVEL_SPEC.md`: the spec, including every decision (§22).
- `docs/RUNBOOK.md`: operating a lane.
- `docs/RESULTS.md`: measured results.
- `docs/SECURITY.md`: the security review.
