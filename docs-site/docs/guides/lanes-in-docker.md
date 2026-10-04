---
title: Lanes in containers
sidebar_position: 2
description: "Run a lane's nodes as containers from the release's images, on your machine or on a server."
---

A lane's sequencer, validators and relayer can run as containers instead of processes or systemd units. The lane file is the same; one line in the host says how the nodes run:

```toml
[env.local.host]
provider = "local"
runtime = "docker"
```

`caravel init` writes it for you when your installed release ships images. `caravel init --runtime process` keeps processes.

## What you need

Docker with the Compose plugin. On a server, also ssh access with passwordless `sudo`. Nothing else: no Node.js, no Caddy, no files copied by hand. `caravel doctor` checks.

## What apply does

`caravel apply` writes the lane's files under its root as usual (`.caravel/<lane>/<env>/` here, `/opt/caravel` on a server), plus two more:

- `config/compose.yml`: one service per node, from the release's images, pinned by digest;
- `config/Caddyfile`, when the deployment has a `public_url`: the `web` service serves the lane's API, each validator's public API and the web app on ports 80 and 443, with its own certificate.

`caravel render` shows both before anything runs. Then it pulls the images, writes the keys (each validator sees only its own) and starts the containers one by one, waiting for each node to report the right lane, config, settlement and engine, as it does for processes.

Every other command works the same: `plan` restarts only the nodes whose configs or release changed, `logs` reads the container's output, and `destroy` and `escape` export the exits from the validators' stores.

## How the containers run

- Read-only, with a private `/tmp`, without Linux capabilities, and unable to gain privileges. The `web` container keeps one capability, to bind ports 80 and 443.
- As uid 10001 on a server, which owns the lane's files there; as you on your machine.
- Nodes reach each other by service name (`http://sequencer:8080`), and each node's port is published on `127.0.0.1` only, or on the host's private address when a validator on another host calls it.
- A local Stellar network is reached at `host.docker.internal`; caravel writes that into the configs.

## Where the images come from

A release names its images in an `IMAGES` file: one per node template (`caravel-perps-node`, `caravel-payments-node`), the relayer (`caravel-relayer`) and each web app (`caravel-perps-web`), on `ghcr.io`. Releases from CI and from `install.sh` carry it. On Linux, `scripts/build-images.sh <release>` builds the images of a release assembled from your checkout and writes the file.
