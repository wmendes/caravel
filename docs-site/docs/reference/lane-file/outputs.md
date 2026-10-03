---
title: Outputs and the web config
sidebar_label: Outputs
sidebar_position: 9
description: "What a deployment publishes, and the config the web app reads."
---

## Outputs

Values a deployment publishes, for scripts and other tools:

```toml
[outputs]
api = "${node.sequencer.url}"
settlement = { value = "${contract.settlement.address}", description = "the settlement contract" }

[env.testnet.outputs]              # per deployment, over the top-level ones
explorer = "https://stellar.expert/explorer/testnet/contract/${contract.settlement.address}"
```

`caravel output` lists them with the built-in ones (`settlement`, `token`, `api`, `lane_id`, …), and `caravel output NAME` prints one. `status --json` has them under `outputs`. An output computed from a sensitive var is sensitive: masked in listings, printed only when asked for by name.

## The web app's config

A template's web app can read its deployment from the lane file instead of build-time variables:

```toml
[env.testnet.web]
# host = "a"                       # default: the sequencer's host

[env.testnet.web.config]
sequencerUrl = "${node.sequencer.url}"
settlementContract = "${contract.settlement.address}"
networkPassphrase = "${network.passphrase}"
validatorUrls = ["${node.validator-1.url}", "${node.validator-2.url}"]
```

- `config` is rendered as `<root>/config/web.json` on the web host. That host's Caddy serves it at `/config.json`, and nothing else from `config/`.
- The perps web app fetches `/config.json` at boot and lays its known keys over its defaults. Without the file, the defaults are the `VITE_*` variables, else the testnet lane.
- A change rewrites the file and restarts nothing.
- On an ssh host, the web host needs a `public_url`. On a `local` host the file is written but not served; the dev server still uses `VITE_*`.

