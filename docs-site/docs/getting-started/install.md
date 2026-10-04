---
title: Install
sidebar_position: 1
description: "Install the caravel CLI: a prebuilt release in one line, or built from a clone."
---

`caravel` is one CLI for every template. It installs into `~/.caravel` (or `$CARAVEL_HOME`). Outside that directory it only adds one line to your shell's startup files, so `caravel` is on your PATH ([below](#your-path)).

## A prebuilt release

On x86_64 or arm64 Linux and arm64 macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/wmendes/caravel/main/scripts/install.sh | bash
```

The script downloads the release for your machine and checks it against the release's `SHA256SUMS`. It also brings the pinned Stellar CLI (28.1.0, checked against GitHub's published digest) unless the `stellar` on your PATH is already that version. `--version vX.Y.Z` picks a release; `--no-stellar-cli` skips the CLI.

### Your PATH

Like rustup, the installer writes `~/.caravel/env` and sources it from your shell's startup files (`~/.profile`, `~/.bashrc` and `~/.bash_profile` when they exist, zsh's `.zshenv`, and fish's `conf.d`), so new terminals find `caravel`. In the terminal you installed from, run `. ~/.caravel/env` once. `--no-modify-path` (or `CARAVEL_NO_MODIFY_PATH=1`) leaves those files alone and prints the line to add yourself. Running the installer again adds nothing twice.

## From a clone

You need Rust 1.93 with the `wasm32v1-none` target (both pinned in `rust-toolchain.toml`), the Stellar CLI 28.1.0 (`cargo install --locked stellar-cli@28.1.0`) and Node.js 22.

```bash
git clone https://github.com/wmendes/caravel && cd caravel
./scripts/install.sh
```

This builds the CLI, every template's node, the contracts and the relayer, and installs them as one release.

## What a lane needs to run

| To run... | You need |
|---|---|
| A lane on your machine | Docker with Compose: the local Stellar network, and the lane's nodes in containers. Node.js 22 only when the nodes run as processes (`caravel init --runtime process`, or a release without images such as 0.1.0) |
| A lane on your servers | Linux hosts over ssh with passwordless sudo, and either Docker with Compose (`runtime = "docker"`, nothing else needed) or systemd with Node.js 22, rsync and curl, plus Caddy for a public URL. See [Deploy to testnet](../guides/deploy-to-testnet.md) and [Lanes in containers](../guides/lanes-in-docker.md) |

`caravel doctor` checks this machine against a lane file and says what's missing.

```bash
caravel version     # this CLI, the templates it finds and the Stellar CLI it needs
caravel doctor
```
