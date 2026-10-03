---
title: Your first lane
sidebar_position: 2
description: "A Payments lane on a local Stellar network: create it, use it, and wind it down."
---

This brings up a Payments lane on your machine, against a local Stellar network that `caravel apply` starts in Docker, and takes it through its whole life. It takes about two minutes once Caravel is [installed](./install.md).

## Create the lane

```bash
caravel init payments my-lane && cd my-lane
```

`init` writes `my-lane/lane.toml` from the Payments template's example and creates the Stellar CLI identities it names (`my-lane-admin`, `my-lane-relayer`, `my-lane-v1`...). Keys stay in the Stellar CLI's keystore: a lane file names identities, never keys.

## Plan and apply

```bash
caravel plan     # what apply would do, on Stellar and on this machine
caravel apply    # starts the local network, deploys the settlement contract, runs the nodes
```

`plan` reads Stellar and the host and lists every step. `apply` shows the same plan, asks, and makes it so. Run `caravel plan` again and it says `No changes.`

## Use it

```bash
caravel account create alice --amount 100      # XLM from friendbot, a trustline, 100 test USDC
caravel account create bob --amount 10
caravel deposit alice 50                       # returns once the lane has credited it
caravel deposit bob 5
caravel tx --from alice transfer --to @bob --amount 5
caravel balance bob
caravel withdraw alice 10                      # waits for its checkpoint on Stellar, then claims it
caravel status
```

`@bob` is bob's Stellar address. Each `caravel tx` is signed with SEP-53 through the Stellar CLI keystore and returns once a block has it.

## Wind it down

```bash
caravel destroy --yes      # drain, export every exit to exit.json, freeze
caravel escape alice       # take the rest back on Stellar
```

`destroy` waits until every signed checkpoint is accepted, stops the sequencer and relayer, writes every account's exit proof to `exit.json`, and freezes the settlement contract. After that, each account takes its last checkpointed balance back on Stellar with `caravel escape`.

## Next

- Try a [starter](./starters.md), or read [how a lane settles](../concepts/settlement.md).
- Take the same lane to your servers: [Deploy to testnet](../guides/deploy-to-testnet.md).
