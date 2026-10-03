---
title: Modules
sidebar_label: Modules
sidebar_position: 6
description: "Reusable groups of accounts, tokens and contracts, with inputs and outputs."
---


A module packages accounts, tokens and contracts behind inputs and outputs, so a deployment can use it once or many times. It is a local `.toml` file (no remote sources):

```toml
# modules/feed.toml
[inputs.symbol]
type = "string"                    # string, integer, boolean, list, map or any

[inputs.decimals]
type = "integer"
default = 7

[locals]
code = "${upper(input.symbol)}"

[accounts.feeder]                  # identity: <instance>-feeder, unless given

[tokens.coin]
code = "${local.code}"
issuer = "feeder"                  # its own account, by its short name

[contracts.feed]
wasm = "feed.wasm"                 # next to the module file
deployer = "feeder"
args = { admin = "${account.feeder.public_key}", asset = "${token.coin.address}", decimals = "${input.decimals}" }

[outputs]
feed = "${contract.feed.address}"
```

```toml
# the lane file
[env.local.modules.oracle]
source = "modules/feed.toml"       # relative to the lane file
for_each = "${['btc', 'eth']}"     # optional: one instance per item, oracle-btc and oracle-eth
inputs = { symbol = "${each.value}" }

[outputs]
btc_feed = "${module.oracle-btc.feed}"
```

- **What a module file holds:** `[inputs]`, `[locals]`, `[accounts]`, `[tokens]`, `[contracts]` and `[outputs]`, nothing else.
- **What its expressions read:** `input`, its own `local`, `lane` and `env`, and the deployment's `account`, `token`, `contract`, `network` and the other names known later. Its own resources answer to their short names. It doesn't see the lane file's `var`; what it needs comes in as inputs.
- **Addresses:** its resources join the deployment as `module.<instance>.<kind>.<name>`, so `--target 'module.oracle-btc.*'` plans one instance. Inside the module, `depends_on` may name its own (`token.coin`) and the deployment's (`account.admin`).
- **Outputs:** an instance's outputs are `module.<instance>.<name>` in the deployment's `[outputs]`.
- **Names:** a `.` in a declared name is reserved for modules.

