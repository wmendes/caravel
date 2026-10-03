---
title: The lane file
sidebar_label: Overview
sidebar_position: 1
description: "What a lane file holds, how caravel finds it and picks a deployment, and a complete example."
---

A lane file declares one lane and its deployments. `caravel` reads it for every command: `plan`, `apply`, `status`, `destroy`, and the commands users run against a lane. These pages cover the language: what a lane file holds, how deployments compose, and what expressions can compute.

## Two halves: genesis and deployments

| Part | Sections | Rules |
|---|---|---|
| **Genesis** | `[lane]`, `[app]`, `[node]`, `[access]`, `[limits]` and the template's own table (`[perps]`, `[payments]`) | Consensus config. These sections are hashed into the lane's `config_hash`, so they stay literal: a `${` in them is an error. Changing them makes a different lane |
| **Deployments** | `[env.<name>]`, plus the top-level `include`, `vars`, `locals` and `outputs` | Where and how the lane runs: network, keys, token, validators, host. Expressions, inheritance and inputs live here |

`[node]` is the one genesis-side section that isn't consensus. A deployment can override it with `[env.<name>.node]`, which reaches the hosts but never genesis.

There is no state file. The lane file, Stellar and the host are the whole truth, and `plan` reads all three.

## Finding the file and the deployment

**The lane file**, in order:
1. `-f <file or directory>`;
2. `CARAVEL_FILE`;
3. `./lane.toml`;
4. the one `lane*.toml` in the directory;
5. the same, walking up to the repository root or your home directory.

**The deployment**, in order:
1. `--env <name>`;
2. `CARAVEL_ENV`;
3. the deployment marked `default = true`;
4. the only deployment.

`caravel env list` shows them and marks the one picked.

## A worked example

One lane, a local deployment and a testnet one, sharing everything they can:

```toml
[vars.validators]
type = "list"
default = ["1", "2", "3"]
description = "the validators' names; a rotation is --var 'validators=[\"1\",\"2\",\"4\"]'"

[vars.host]
type = "string"
default = "deploy@lane.example"
description = "the testnet host, for ssh"

[locals]
ids = "${lane.name}"

[outputs]
api = "${node.sequencer.url}"
settlement = "${contract.settlement.address}"

[lane]
name = "acme-pay"
# … [app], [node], [access], [limits] and [payments], as `caravel init payments` writes them

[env.base]
abstract = true
admin = "${local.ids}-admin"
threshold = "${length(var.validators) / 2 + 1}"      # a majority: 2 of 3, 3 of 4
relayer = { account = "${local.ids}-relayer" }

[env.base.settlement_params]
force_inclusion_window_secs = 20
escape_timeout_secs = 30
min_rotation_delay_secs = 3600
signer_retention_epochs = 2

[env.base.validators]
for_each = "${var.validators}"
name = "${each.value}"
key = "${local.ids}-v${each.value}"

[env.local]
extends = "base"
default = true
network = "local"
token = { local = "USDC" }
host = { provider = "local" }
node = { checkpoint_every_blocks = 5 }      # this deployment's [node]: not consensus

[env.testnet]
extends = "base"
network = "testnet"
token = "circle-usdc"
sequencer = { port = 8080 }
host = { provider = "ssh", address = "${var.host}", public_url = "https://lane.example" }
```

```sh
caravel plan                                       # the local deployment (default = true)
caravel plan --env testnet --var host=me@my-vm     # the testnet one, on another host
caravel apply --var 'validators=["1","2","4"]'     # replace validator 3: a signer rotation
caravel output settlement
```

