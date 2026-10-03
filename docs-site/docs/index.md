---
title: What is Caravel
slug: /
sidebar_label: What is Caravel
sidebar_position: 1
description: "Appchains with their own rules, settled on Stellar: declared in one lane file, planned, applied, and wound down to an exit proof for every account."
---

Caravel runs appchains that settle on Stellar. We call them **lanes**. A lane is declared in one file, the **lane file**, and the `caravel` CLI turns that file into a running chain:

- `caravel plan` shows every change on Stellar and on your hosts before it's made;
- `caravel apply` makes it;
- `caravel destroy` winds the lane down to an exit proof for every account.

## Who it's for

On Stellar, every app shares the network's fees and rules. A lane sets its own, one line each:

| An app that is... | ...might set |
|---|---|
| A trading venue (the Perps template) | zero maker fees, its own leverage limits, its own price feed, settlement in Circle's USDC |
| A payment network (the Payments template) | a flat fee per transfer, members only, its own validators, its own settlement token |
| A game economy (your engine, on the app SDK) | 200 ms blocks, players only, a cap on moves per block, an in-game token |
| An agent marketplace (Payments) | no fee between agents, session keys for each agent, a cap on accounts |

Deposits, withdrawals and exits stay on Stellar. If Stellar's shared rules already fit your app, build on Stellar directly: a lane adds operators, validators and a relayer to run.

## How it works

1. **Deposit.** A user locks a token in the lane's settlement contract on Stellar. The same amount appears on the lane.
2. **Use it.** Every block runs the lane's app, a Soroban Wasm engine: an order book on Perps, transfers on Payments.
3. **Checkpoint.** The validators re-execute every block and sign the new state. It goes to Stellar with its block data, and the settlement contract checks the signatures.
4. **Withdraw.** The token goes back to the user's wallet, only against an accepted checkpoint.

If the lane stops, anyone can freeze it, and every account withdraws its last checkpointed balance on Stellar. [Who you trust](./concepts/trust.md) has the details.

:::caution Testnet only, not audited
Caravel runs on Stellar testnet and has not been audited. Between checkpoints you trust the lane's validators. Validity proofs and bonded validators are planned, not built.
:::

## Next

- [Install Caravel](./getting-started/install.md), then [run your first lane](./getting-started/first-lane.md) on your machine.
- Read the [concepts](./concepts/lanes-and-templates.md), or go straight to the [lane file reference](./reference/lane-file/index.md).
