---
title: Use a lane
sidebar_position: 6
description: "Accounts, deposits, lane transactions, withdrawals, claims, forced withdrawals and escapes."
---

Every user flow is a `caravel` command that waits for what it needs.

| Command | What it does |
|---|---|
| `caravel account create alice --amount 100` | An identity with XLM, a trustline to the settlement token and some of it (minted on a local network; bought on the DEX for Circle's testnet USDC) |
| `caravel deposit alice 50` | Deposits into the lane and returns once the lane has credited it |
| `caravel tx --from alice transfer --to @bob --amount 5` | A lane transaction in the template's syntax, signed with SEP-53 through the keystore, returned once a block has it |
| `caravel balance alice` | The account on Stellar and on the lane |
| `caravel withdraw alice 10` | The withdrawal, then its leaf in an accepted checkpoint, then the claim on Stellar |
| `caravel claim alice` | Claims every unclaimed withdrawal |
| `caravel force-withdraw alice 10` | Asks the settlement contract for a withdrawal the lane must include, then claims it |
| `caravel escape alice` | After a freeze: the account's withdrawals and its share of the last checkpoint |
| `caravel api /v1/accounts/G…` | Any path of the lane's API, as JSON |

`@name` stands for an identity's Stellar address. Every command takes `--json` for scripts.

For an app, talk to the lane's API directly: `caravel tx` is a convenience, and each call signs, sends and waits.
