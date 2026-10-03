---
title: Install
sidebar_position: 1
description: "Install the caravel CLI: a prebuilt release in one line, or built from a clone."
---

`caravel` is one CLI for every template. It installs into `~/.caravel` (or `$CARAVEL_HOME`) and touches nothing outside it.

## A prebuilt release

On x86_64 or arm64 Linux and arm64 macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/wmendes/caravel/main/scripts/install.sh | bash
export PATH="$HOME/.caravel/bin:$PATH"
```

The script downloads the release for your machine and checks it against the release's `SHA256SUMS`. It also brings the pinned Stellar CLI (28.1.0, checked against GitHub's published digest) unless the `stellar` on your PATH is already that version. `--version vX.Y.Z` picks a release; `--no-stellar-cli` skips the CLI.

:::note
There is no published release yet. Until there is, the one-liner says so and stops: build from a clone instead.
:::

## From a clone

You need Rust 1.93 with the `wasm32v1-none` target (both pinned in `rust-toolchain.toml`), the Stellar CLI 28.1.0 (`cargo install --locked stellar-cli@28.1.0`) and Node.js 22.

```bash
git clone https://github.com/wmendes/caravel && cd caravel
./scripts/install.sh
export PATH="$HOME/.caravel/bin:$PATH"
```

This builds the CLI, every template's node, the contracts and the relayer, and installs them as one release.

## What a lane needs to run

| To run... | You need |
|---|---|
| A lane on your machine | Docker (the local Stellar network) and Node.js 22 (the lane's relayer) |
| A lane on your servers | ssh access to Linux hosts with systemd, passwordless sudo, Node.js 22, rsync and curl, plus Caddy for a public URL. See [Deploy to testnet](../guides/deploy-to-testnet.md) |

`caravel doctor` checks this machine against a lane file and says what's missing.

```bash
caravel version     # this CLI, the templates it finds and the Stellar CLI it needs
caravel doctor
```
