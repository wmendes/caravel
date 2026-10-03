---
title: Lanes and templates
sidebar_position: 1
description: "What a lane is made of: an app engine, a sequencer, validators, a relayer and a settlement contract on Stellar."
---

A **lane** is an appchain with its own rules that settles on Stellar. It is made of:

| Part | Where it runs | What it does |
|---|---|---|
| **The engine** | inside every node | The lane's app, a Soroban Wasm. It runs every block. Consensus executes this exact Wasm, so every node computes the same state |
| **The sequencer** | your host | Orders transactions into blocks, at the lane's block time, and serves the lane's API |
| **The validators** | your hosts, or run by others | Re-execute every block from the sequencer, rebuild each checkpoint themselves, and sign it only if it matches |
| **The relayer** | the sequencer's host | Posts checkpoints to Stellar and brings deposits and forced withdrawals from Stellar into the lane |
| **The settlement contract** | Stellar | Holds the lane's token, checks every checkpoint's signatures, and pays withdrawals and exits |

## Templates

A **template** is an engine plus the node binary that runs it. Caravel has two:

- **Perps:** perpetual futures on an order book, priced by an oracle feed. Caravel Perps, the first lane, runs it on testnet in Circle's testnet USDC.
- **Payments:** transfers with a flat fee or none, in the token you choose.

Both are built from the same platform, so they share the CLI, the lane file, settlement and exits. The app SDK (`caravel-app-sdk`, `no_std`) is how a new template is written: Payments is built on it.

## One CLI for every template

`caravel` reads `[app] template` from the lane file and asks that template's node binary for what only it knows, such as the genesis hashes, through a small plugin protocol. Everything else is the platform's: plans, deployments, hosts, keys and the users' flows.
