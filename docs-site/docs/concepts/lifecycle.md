---
title: Plan, apply, destroy
sidebar_position: 5
description: "How a change goes from the lane file to Stellar and the hosts, safely."
---

## Plan

`caravel plan` compares the lane file with what Stellar and the hosts have, and lists every step `apply` would take. Each step is a resource with an address (`contract.settlement`, `signers.settlement`, `file.sequencer.toml`, `node.validator-2`), ordered by what depends on what. Problems that would block apply are listed too.

What a change does depends on what it touches:

| A change to... | Does |
|---|---|
| `[node]` (block time, checkpoint cadence) | Restarts the nodes that read it |
| The validators or the threshold | A signer rotation: the new validator starts, the contract gets the new set, the sequencer restarts on the new epoch, the old validator stops |
| Anything the constructor fixed (the token, fees, access, limits, exit timers) | Refused: that's a different lane. Destroy this one, or give the new one another name |

## Apply

`caravel apply` prints the plan, asks (or `--yes`), and makes it so: it funds accounts, deploys contracts, installs the release, writes files and starts nodes. Run it again and nothing changes. If it's interrupted, the next run finishes the job.

To apply exactly what was reviewed, save the plan and apply that:

```bash
caravel plan --out release.plan.json
caravel apply release.plan.json      # refuses, and says why, if anything moved since
```

`--target node.relayer` plans one resource and what it needs; `--replace file.sequencer.toml` redoes one that matches. `caravel graph` shows every resource and edge.

## Status

`caravel status` reports the lane's height, the last checkpoint accepted on Stellar, when a freeze would become possible, the relayer's XLM, and whether the deployment still matches the lane file.

## Destroy

`caravel destroy` winds a lane down for good: it drains, exports every exit, and freezes the contract. A frozen lane can't be restarted. See [Wind a lane down](../guides/wind-down.md).
