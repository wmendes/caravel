---
title: The lane file
sidebar_position: 4
description: "Genesis versus deployments, identities instead of keys, and why there is no state file."
---

A lane file declares one lane and the places it runs. It has two halves:

- **Genesis:** `[lane]`, `[app]`, `[node]`, `[access]`, `[limits]` and the template's own table (`[payments]`, `[perps]`). These are the lane's rules. They are hashed into its `config_hash`, which the settlement contract commits to, so they stay literal, and changing one makes a different lane.
- **Deployments:** `[env.<name>]` tables. Each says where and how the lane runs: the network, the admin, the token, the validators, the hosts. Deployments compose with `include` and `extends`, take vars, and compute values with expressions.

`[node]` is the one genesis-side table that isn't consensus (block time, checkpoint cadence). A deployment can override it with `[env.<name>.node]`.

## Identities, never keys

Every key field names a Stellar CLI identity (`admin = "acme-admin"`). Caravel refuses a secret key or a seed phrase anywhere in a lane file. Keys reach a host over the ssh connection when it needs them, and never enter git, a release or your disk.

## No state file

There is nothing to keep in sync. The lane file, Stellar and the hosts are the whole truth, and `caravel plan` reads all three every time. Addresses are derived: the settlement contract's address comes from the admin's key and the lane's name, a declared contract's from its deployer and name. So a plan can always say what exists and what would change.

The [lane file reference](../reference/lane-file/index.md) covers every table and key.
