---
title: Settlement, checkpoints and exits
sidebar_position: 2
description: "How money enters a lane, how its state reaches Stellar, and how every account gets out."
---

A lane's money never leaves Stellar's settlement contract. The lane moves balances between its accounts; the contract pays out only against state it has checked.

## Deposits

A user calls the settlement contract on Stellar, which locks the tokens and adds the deposit to its **inbox**. The relayer brings each inbox message into the lane, and the next block credits the account. `caravel deposit` returns once the lane has credited it.

## Checkpoints

When something is waiting for Stellar (within seconds of a deposit or withdrawal, within a minute of a trade, and every so often when the lane is idle; see [Block time and checkpoints](../reference/lane-file/index.md#block-time-and-checkpoints)), the sequencer seals a **checkpoint**: a header that commits to the lane's state, its withdrawals, the inbox it has processed and the batch of blocks since the last one.

1. Each validator re-executes the blocks itself, rebuilds the header, and signs it only if it is byte for byte the same. It never signs two different headers for one checkpoint.
2. The relayer submits the header, its batch and the signatures to the settlement contract.
3. The contract checks the signatures against the current signer set and threshold, that the header follows the last one, and that the batch matches. Then it accepts it.

Anyone can replay a lane from Stellar data alone and check every checkpoint: see [Replay a lane](../guides/replay.md).

## Withdrawals

A withdrawal is a lane transaction. It leaves a leaf in the next checkpoint, and once that checkpoint is accepted on Stellar, the owner claims it from the contract with a proof. `caravel withdraw` does all three steps and waits between them.

If the lane won't take a withdrawal, the user can ask the contract for one directly (`caravel force-withdraw`). The lane must include it within `force_inclusion_window_secs`.

## When a lane stops

If no checkpoint is accepted for `escape_timeout_secs`, or a deposit or forced withdrawal isn't processed within `force_inclusion_window_secs`, **anyone can freeze the lane**. After a freeze the contract takes nothing more from the lane, and every account claims its equity from the last accepted checkpoint (`caravel escape`). Deposits the lane never processed are refunded.

`caravel destroy` is the planned version of the same ending: it drains the lane, exports every account's exit proof to `exit.json`, and freezes the contract. See [Wind a lane down](../guides/wind-down.md).

:::note
The settlement token's own rules still apply. An issuer's freeze or clawback reaches the contract's balance too.
:::
