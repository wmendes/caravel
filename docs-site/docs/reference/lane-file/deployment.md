---
title: A deployment
sidebar_label: Deployment
sidebar_position: 2
description: "An [env.<name>] table: network, admin, token, validators, sequencer, relayer and host."
---


```toml
[env.local]
default = true                     # picked when no --env is given (at most one)
network = "local"                  # "local" or "testnet"; mainnet is refused
admin = "acme-admin"               # a Stellar CLI identity; keys never go in the file
token = { local = "USDC" }         # the settlement token (below)
threshold = 2                      # signature weight a checkpoint needs

[env.local.settlement_params]      # fixed at deploy
force_inclusion_window_secs = 20
escape_timeout_secs = 30
min_rotation_delay_secs = 3600
signer_retention_epochs = 2

[[env.local.validators]]
name = "1"                         # the node is validator-1
key = "acme-v1"                    # identity whose key signs checkpoints
# weight = 1, port = sequencer port + 1 by default

[env.local.sequencer]
port = 18080

[env.local.relayer]
account = "acme-relayer"           # pays for checkpoint transactions

[env.local.host]
provider = "local"                 # or "ssh"
```

**Other keys:**
- `rpc_url` overrides the network's RPC.
- `settlement_wasm` pins a settlement build by its sha256.
- `settlement` names a contract deployed before this tool (lane #1). New lanes derive the address from the admin and the lane's name.
- `validator_polling = { sequencer_ms, stellar_secs }`.
- `sequencer = { port, cors_origins, production }`.
- `relayer = { account, feed_keys, feeds, intervals_ms }`. `feeds` are the template's feed modules; `feed_keys` maps an env var to an identity whose secret a feed reads.
- `host = { provider, address, transport, project, zone, public_url, root, private_address, namespace }` (or `hosts`, [below](./hosts.md#several-hosts)):
  - `address` is `user@host` for ssh, or the VM name with `transport = "gcloud-iap"`;
  - `public_url` is where the lane's API is served, and the user commands use it.

**Settlement tokens:**

| `token =` | Settles in |
|---|---|
| `"circle-usdc"` | Circle's testnet USDC |
| `{ asset = "CODE:G…" }` | A Stellar asset, through its Stellar Asset Contract |
| `{ contract = "C…" }` | A SEP-41 token contract |
| `{ local = "CODE" }` | A test asset issued by the admin, on a local network |
| `"usd"` | A token the deployment declares (`[env.<name>.tokens.usd]`), issued by the admin or a `G…` address |

**What a change does:** a validator swap becomes a signer rotation, and a `[node]` change becomes a restart. Anything the settlement contract fixed at deploy is refused: the lane's rules, the engine, the admin, the token and the settlement params.

