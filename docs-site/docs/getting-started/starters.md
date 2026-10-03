---
title: Starters
sidebar_position: 3
description: "Small, complete lanes to build on: agents paying each other, and a lane with an oracle."
---

Each starter is a script and a short guide in the repository's `examples/` folder. It runs end to end with `caravel` alone, on your machine. Clone the repository, put `caravel` on your PATH, and run it.

## Agents paying each other

`examples/agent-payments`: a few agents, each funded on Stellar with test USDC, pay each other small amounts on a Payments lane, all at once. Every payment is a lane transaction confirmed in a block. The script then prints the balances, sends one agent's money back to Stellar and winds the lane down.

```bash
./examples/agent-payments/run.sh                   # 3 agents, 30 payments of 0.05
AGENTS=5 PAYMENTS=100 ./examples/agent-payments/run.sh
```

## A lane with an oracle

`examples/oracle-trading`: the Perps template, the app behind Caravel Perps on testnet, with fixed prices. Two traders deposit collateral, one rests a bid and the other sells into it. The script prints the trade and the position from the lane's API.

```bash
./examples/oracle-trading/run.sh
KEEP=1 ./examples/oracle-trading/run.sh            # keep the lane running afterwards
```

To use live prices, give a market sources such as `[{ coinbaseStream = "BTC-USD" }, { coinbase = "BTC-USD" }]` in `lane.toml` and run `caravel apply`. It restarts the relayer and nothing else.

## Build your own

The Payments template is built on the app SDK (`platform/crates/caravel-app-sdk`), a `no_std` SDK for lane engines. A new template is a Soroban Wasm engine on that SDK, plus a node binary that links `caravel-node`.
