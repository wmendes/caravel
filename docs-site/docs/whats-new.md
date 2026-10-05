---
title: What's new
sidebar_position: 6
description: "Changes to Caravel, newest first."
---

## October 2026

- **Caravel 0.4.1.** `caravel withdraw` goes from the command to claimed on Stellar in about 18 s instead of up to two minutes.
- **Caravel 0.4.0: checkpoints when needed.** A lane checkpoints within seconds of a deposit or withdrawal, within a minute of a trade, and only every few minutes when idle, set by `checkpoint_urgent_ms`, `checkpoint_busy_ms` and `checkpoint_idle_ms` in `[node]`. The settlement contract for new lanes keeps a record only for checkpoints with withdrawals, so a checkpoint without them costs about 0.002 XLM on testnet instead of 0.34. `caravel validate` says when checkpoints come, and refuses times the contract's windows can't allow. Every checkpoint's fee, with its rent, is in the relayer's metrics.
- **Caravel 0.3.0**, a performance release. Validators keep only recent blocks and the sequencer compresses old ones, so three of a lane's four stores stay flat and the fourth grows about 2.4 times slower. Store files give space back as they prune. Checkpoints get signed in milliseconds instead of after a 2 s retry, and the relayer picks them up at once: about 2.5 s less from a transaction to its checkpoint on Stellar. Every node reports its timings under `perf` in `/v1/status` and `caravel status --json`. Logs have a size cap, and `caravel validate` points out a checkpoint cadence faster than every 30 s.
- **The installer sets up your PATH**, like rustup: new terminals find `caravel` with no `export` to add (`--no-modify-path` to opt out).
- **Caravel 0.2.0.** Lanes in containers: `runtime = "docker"` runs the nodes, the relayer and Caddy from the release's images, on your machine or on a server, so a first lane needs Docker and the `caravel` CLI. Images for x86_64 and arm64 on `ghcr.io`. Infrastructure as code for the machine a lane runs on ([Machines on Google Cloud](guides/machines-on-gcp.md)). Caravel Perps runs this way on testnet.
- **Docs.** This site.
- **Caravel 0.1.0**, the first prebuilt release, with a one-line install for x86_64 and arm64 Linux and arm64 macOS.
- **Topology.** Several hosts per deployment, validators run by others, signed requests to validators, several lanes on one host, and the web app's config from the lane file.
- **Chain resources.** Declared accounts, tokens, contracts and local modules.
- **The resource graph.** Every plan step has an address: `--target`, `--replace`, `caravel graph`, saved plans.
- **The lane file language.** `include`, `extends`, vars, locals, `for_each`, expressions, attributes and outputs.
- **A real CLI.** `caravel` for every template, with users' flows that wait for what they need.

## September 2026

- **Caravel Perps** live on testnet, and the **Payments** template.
- Lanes from one file: `plan`, `apply`, `status`, `destroy`.
