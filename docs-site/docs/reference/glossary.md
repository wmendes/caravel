---
title: Glossary
sidebar_position: 5
description: "The words Caravel uses."
---

**Lane.** An appchain with its own rules that settles on Stellar.

**Lane file.** The TOML file that declares a lane: its genesis and its deployments.

**Genesis.** The lane's rules (`[lane]`, `[app]`, `[node]`, `[access]`, `[limits]`, the template's table), hashed into the `config_hash` the settlement contract commits to.

**Deployment.** An `[env.<name>]` table: where and how the lane runs.

**Template.** An engine and the node binary that runs it: Perps, Payments.

**Engine.** The lane's app, a Soroban Wasm that runs every block.

**Sequencer.** The node that orders transactions into blocks and serves the lane's API.

**Validator.** A node that re-executes every block and signs checkpoints it rebuilt itself.

**Relayer.** The process that posts checkpoints to Stellar and brings deposits into the lane.

**Settlement contract.** The contract on Stellar that holds the lane's token, checks checkpoints, and pays withdrawals and exits.

**Checkpoint.** A signed header committing to the lane's state and withdrawals, accepted by the settlement contract.

**Signer set.** The validators' keys and weights the settlement contract accepts, with a threshold. Each rotation starts a new epoch.

**Inbox.** The contract's queue of deposits and forced withdrawals for the lane to process.

**Freeze.** The end of a lane: after it, the contract takes nothing more from the lane and accounts exit from the last checkpoint.

**Escape.** An account's claim after a freeze: its withdrawals and its share of the last checkpoint.

**Identity.** A named key in the Stellar CLI's keystore. Lane files name identities, never keys.

**Namespace.** What sets a lane apart from others on the same ssh host.
