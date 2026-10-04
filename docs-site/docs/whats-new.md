---
title: What's new
sidebar_position: 6
description: "Changes to Caravel, newest first."
---

## October 2026

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
