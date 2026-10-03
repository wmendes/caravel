# Caravel

[![CI](https://github.com/wmendes/caravel/actions/workflows/ci.yml/badge.svg)](https://github.com/wmendes/caravel/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
![Network: Stellar testnet](https://img.shields.io/badge/network-Stellar%20testnet-26958F.svg)

**Infrastructure as code for Stellar appchains.** Caravel deploys and runs appchains, called *lanes*, that settle to Stellar in the token each one chooses. You declare a lane in one file, and four commands manage it:

| Command | What it does |
|---|---|
| `caravel plan` | Shows every change on Stellar and on your host before anything is sent, down to the settlement contract's address |
| `caravel apply` | Makes the lane match the file. A second run changes nothing, and an interrupted run finishes on the next one |
| `caravel status` | Height, checkpoints accepted on Stellar, when a freeze would become possible, and drift from the file |
| `caravel destroy` | Drains the lane, writes every account's exit proof to `exit.json`, and freezes its contract |

> **Status:** testnet software, not audited. See [What this is not](#what-this-is-not).

## Contents

- [How a lane works](#how-a-lane-works)
- [Live on testnet](#live-on-testnet)
- [Quickstart](#quickstart)
- [The lane file](#the-lane-file)
- [Settlement tokens](#settlement-tokens)
- [Deploying to testnet](#deploying-to-testnet)
- [What this is not](#what-this-is-not)
- [Repository layout](#repository-layout)
- [Documentation](#documentation)
- [Contributing](#contributing)
- [License](#license)

## How a lane works

- **Nodes.** A lane is a sequencer and its validators. They run the lane's engine, a Soroban Wasm, through `soroban-env-host`, and every validator runs every block again itself.
- **Checkpoints.** Every checkpoint goes to the lane's settlement contract on Stellar with its block data. The contract accepts it only with the validators' threshold signature.
- **Funds.** The lane's token stays in the settlement contract. Withdrawals are Merkle proofs against an accepted checkpoint.
- **Exit.** If the lane stops checkpointing, or ignores deposits and forced withdrawals sent through Stellar, anyone can freeze the contract. Users then take their last checkpointed balance back on Stellar.
- **Replay.** Anyone can rebuild a lane from Stellar data alone and check every state hash.

Two templates exist:
- **Caravel Perps:** perpetual futures on an order book.
- **Payments:** transfers between lane accounts, with a flat fee.

New templates build on the app SDK (`platform/crates/caravel-app-sdk`).

## Live on testnet

**Lane #1, Caravel Perps**, has run on Stellar testnet since 2026-09-29, in Circle's testnet USDC. Its whole deployment is the `[env.testnet]` table in [`lanes/perps/config/lane.caravel-perps.testnet.toml`](lanes/perps/config/lane.caravel-perps.testnet.toml).

- **Trading app:** https://35-224-76-64.sslip.io
- **Lane status:** https://35-224-76-64.sslip.io/v1/status
- **Settlement contract:** [`CBIHBEUZYFZQZEQPBJH2ID6CDRDZFEDI6XHAXVOCHG6FO5XWUIGPONWO`](https://stellar.expert/explorer/testnet/contract/CBIHBEUZYFZQZEQPBJH2ID6CDRDZFEDI6XHAXVOCHG6FO5XWUIGPONWO)

Measured on that lane, with 1 s blocks, on one e2-small VM ([`docs/RESULTS.md`](docs/RESULTS.md), 2026-09-29):

| At 45 tx/s for 5 minutes | Result |
|---|---|
| Soft confirmation (receipt on the WebSocket), p50 / p99 | 648 ms / 1,131 ms |
| Hard settlement (checkpoint accepted on Stellar), p50 / p99 | 12.4 s / 19.1 s |
| Stellar fee per 1,000 lane transactions | 0.842 XLM |

A Payments lane was also run through its whole life on testnet from a lane file: apply, deposits, a transfer, a validator rotation, a forced withdrawal, destroy, escapes from `exit.json` and replay. The run took 215 s (2026-09-30, [`docs/RESULTS.md`](docs/RESULTS.md)).

## Quickstart

This brings up a Payments lane on your machine, against a local Stellar network, and uses it.

**Requirements:**
- Rust 1.93 with the `wasm32v1-none` target (both pinned in `rust-toolchain.toml`);
- the Stellar CLI 28.1.0: `cargo install --locked stellar-cli@28.1.0`;
- Node.js 22;
- Docker, for the local Stellar network.

**Install** from a clone. This builds the CLI, the templates' nodes, the contracts and the relayer into `~/.caravel`:

```sh
./scripts/install.sh
export PATH="$HOME/.caravel/bin:$PATH"
```

Once a release is published, the same script can install it prebuilt instead, on x86_64 or arm64 Linux and arm64 macOS. It needs no Rust, and it brings the pinned Stellar CLI if yours isn't 28.1.0. Docker and Node.js 22 are still needed to run a lane. There is no published release yet.

```sh
curl -fsSL https://raw.githubusercontent.com/wmendes/caravel/main/scripts/install.sh | bash
```

**Run a lane and use it:**

<!-- quickstart: scripts/check-quickstart.sh runs this block as written -->
```sh
caravel init payments my-lane && cd my-lane    # lane.toml, and Stellar CLI identities my-lane-<role>
caravel apply                                  # starts a local network, deploys, runs the nodes
caravel account create alice --amount 100      # XLM from friendbot, a trustline, 100 test USDC
caravel account create bob --amount 10
caravel deposit alice 50                       # returns once the lane has credited it
caravel deposit bob 5
caravel tx --from alice transfer --to @bob --amount 5
caravel balance bob
caravel withdraw alice 10                      # waits for its checkpoint on Stellar, then claims it
caravel status
caravel destroy --yes                          # drain, export every exit to exit.json, freeze
caravel escape alice                           # take the rest back on Stellar
```

Each command finds `lane.toml` in the current directory (or `-f`) and uses its default deployment (or `--env`). Every command takes `--json`.

**The commands:**

| For | Commands |
|---|---|
| A lane file | `init`, `validate`, `render`, `env list`, `keys list\|ensure\|show`, `output` |
| Running a lane | `plan`, `apply` (both with `--target` and `--replace`; `plan --out FILE`, then `apply FILE`), `graph`, `status`, `destroy`, `stop`, `start`, `restart`, `logs`, `wait`, `api`, `replay`, `doctor` |
| Using a lane | `account create\|fund`, `balance`, `deposit`, `tx`, `withdraw`, `claim`, `force-withdraw`, `escape` |

Lane transactions are signed through the Stellar CLI's keystore (SEP-53 message signing), so no key is ever written out.

**Exit codes:** 0 done, 1 error, 2 usage, 3 changes pending (`plan --exit-code`), 4 a wait timed out.

**The full lifecycle:** `./scripts/e2e-local.sh` runs every step of a lane's life with these commands: user flows, a signer rotation, a forced withdrawal, destroy, escapes and replay.
- `E2E_TEMPLATE=perps` or `payments` picks the template;
- `E2E_NETWORK=testnet` runs it on testnet.

**As a Stellar CLI plugin:** the installer also links `stellar-caravel`, so `stellar caravel plan` works too.

## The lane file

The lane's own sections define its genesis: `[lane]`, `[app]`, `[node]`, `[access]`, `[limits]` and the template's table. They are consensus config and stay literal. Each `[env.<name>]` table is one deployment of the lane, and deployments are where the language is: vars, locals, `${…}` expressions, `extends`, `include`, `for_each` and outputs.

```toml
[vars.validators]
type = "list"
default = ["1", "2", "3"]

[env.base]
abstract = true                                # only for extending
admin = "acme-admin"                           # a Stellar CLI identity; keys never go in the file
threshold = "${length(var.validators) / 2 + 1}"
relayer = { account = "acme-relayer" }

[env.base.validators]                          # one validator per name
for_each = "${var.validators}"
name = "${each.value}"
key = "acme-v${each.value}"

[env.local]
extends = "base"
default = true
network = "local"                              # or "testnet"; mainnet is refused
token = { local = "USDC" }
host = { provider = "local" }

[env.testnet]
extends = "base"
network = "testnet"
token = "circle-usdc"
host = { provider = "ssh", address = "deploy@lane.example", public_url = "https://lane.example" }

[outputs]
settlement = "${contract.settlement.address}"
```

`caravel apply --var 'validators=["1","2","4"]'` then replaces validator 3, and `caravel output settlement` prints the contract's address.

**What a change does:**
- **Applied as a step:** a validator swap becomes a signer rotation, and a `[node]` setting becomes a restart.
- **Refused:** anything the settlement contract fixed at deploy (the lane's rules, the engine, the admin, the token, the settlement params). For those, destroy the lane or give it a new name.

There is no state file. The lane file and the chain are the whole truth, and `plan` reads both. The full reference is [`docs/LANE_FILE.md`](docs/LANE_FILE.md).

## Settlement tokens

A lane settles in the token its `[env]` names:

| `token =` | Settles in |
|---|---|
| `"circle-usdc"` | Circle's testnet USDC |
| `{ asset = "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5" }` | Any Stellar asset, through its Stellar Asset Contract. `apply` deploys that contract when the network has none |
| `{ contract = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA" }` | Any SEP-41 token contract |
| `{ local = "USDC" }` | A test asset issued by the admin, on a local network |
| `"usd"` | A token the deployment declares (`[env.<name>.tokens.usd]`, [`docs/LANE_FILE.md`](docs/LANE_FILE.md#declared-tokens)) |

The two examples above are the same token, Circle's testnet USDC, written both ways.

**What a token must do:**
- **Move exact amounts.** A token that charges a fee on transfer, or rebases, would break the vault's accounting, and the tool can't check that.
- **Its issuer's rules still apply.** A freeze or clawback reaches the contract's balance too.
- **Decimals.** Perps arithmetic is in 10^-7 units, so a Perps lane needs a 7-decimal token (every Stellar asset has 7). Payments takes any decimals.

## Deploying to testnet

- **Network:** set `network = "testnet"` in the `[env]` table.
- **Wasm:** pass `--wasm-dir` with the `contracts-wasm` artifact that CI builds for each commit, so the lane deploys the Wasm of record. Hashes of record are x86_64 Linux builds.
- **Host:** `provider = "ssh"` runs the nodes as systemd units on a Linux host prepared with [`lanes/perps/deploy/testnet/provision.sh`](lanes/perps/deploy/testnet/provision.sh). The host is reached over ssh, or over `gcloud compute ssh --tunnel-through-iap`, and keys stream over the connection into mode-600 files.
- **Operations:** releases, rotations and recovery are in [`docs/RUNBOOK.md`](docs/RUNBOOK.md).

## What this is not

- **Not trustless.** If a threshold of validators collude with the sequencer, they can sign a wrong state. Replay detects this but can't prevent it.
- **Stellar validators don't run lane blocks.** They check signatures and store the data. Lane blocks aren't Stellar transactions, and a lane's block time is its own.
- **Not audited, not production, not mainnet:**
  - on testnet, a lane's admin can still upgrade its settlement contract and rotate its validators;
  - lane #1's three validators run on one machine operated by the Caravel team.

## Repository layout

| Path | Contents |
|---|---|
| `platform/crates/caravel-core` | The formats every lane shares |
| `platform/crates/caravel-app-sdk` | The `no_std` SDK for lane engines |
| `platform/crates/caravel-runtime` | Executor, store, sequencer and validator cores |
| `platform/crates/caravel-node` | Sequencer, validator, replay and genesis for any app |
| `platform/crates/caravel-lanefile` | The lane file language: include, extends, vars, expressions |
| `platform/crates/caravel-deploy` | The deploy library: plans, providers, chain reads, user flows |
| `platform/crates/caravel-cli` | `caravel`, the one CLI for every template |
| `platform/contracts/settlement` | The settlement contract (vault, inbox, checkpoints, freeze, escape) |
| `platform/relayer` | Posts inbox entries and checkpoints to Stellar, and hosts feed modules |
| `lanes/perps` | Caravel Perps: its frozen engine, node, trading app and oracle feed |
| `lanes/payments` | The Payments template |
| `scripts` | Build, check, deploy and end-to-end scripts |

## Documentation

- [`docs/CARAVEL_SPEC.md`](docs/CARAVEL_SPEC.md): the specification, the source of truth, with every design decision in §22.
- [`docs/LANE_FILE.md`](docs/LANE_FILE.md): the lane file language.
- [`docs/RUNBOOK.md`](docs/RUNBOOK.md): operating a lane.
- [`docs/RESULTS.md`](docs/RESULTS.md): measured results on testnet.
- [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md): engine benchmarks at full caps.
- [`docs/SECURITY.md`](docs/SECURITY.md): the security review and its findings.
- [`docs/SOURCES.md`](docs/SOURCES.md): where each Stellar fact and prior-art idea comes from.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the workflow and the checks every change must pass.

## License

Licensed under either of:
- [Apache License, Version 2.0](LICENSE-APACHE);
- [MIT license](LICENSE-MIT).

You may choose either. Unless you state otherwise, any contribution you intentionally submit for inclusion in this work, as defined in the Apache-2.0 license, is dual licensed as above, without any additional terms or conditions.
