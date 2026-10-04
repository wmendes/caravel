---
title: Deploy to testnet
sidebar_position: 1
description: "Take a lane from your machine to your own servers over ssh, against Stellar testnet."
---

The lane file you try locally is the one you deploy. Add a testnet deployment with an ssh host, and the same `plan` and `apply` run it there.

## Prepare a host

A host is a Linux machine you can reach over ssh (or Google Cloud's IAP) with passwordless `sudo`. How its nodes run decides what else it needs:

- **Containers** (`runtime = "docker"` in the host, see [Lanes in containers](lanes-in-docker.md)): Docker Engine with the Compose plugin, and `curl`. The release's images bring the nodes, the relayer and Caddy.
- **systemd** (the default for ssh hosts): an unprivileged `caravel` user and the lane's root, `/opt/caravel` by default; Node.js 22 or later, `rsync` and `curl`; Caddy, if the deployment has a `public_url`.

`caravel plan` checks all of this and lists what's missing. To make a Google Cloud VM with Docker on it, use the repository's infrastructure as code ([Machines on Google Cloud](machines-on-gcp.md)). For a systemd host, `lanes/perps/deploy/testnet/provision.sh` sets up Ubuntu: it installs Caddy and Node.js, creates the user and root, and adds swap.

## Add a testnet deployment

```toml
[env.testnet]
network = "testnet"
admin = "acme-admin"                 # an identity in your Stellar CLI keystore
token = "circle-usdc"                # Circle's testnet USDC
threshold = 2

[env.testnet.settlement_params]
force_inclusion_window_secs = 3600
escape_timeout_secs = 21600
min_rotation_delay_secs = 3600
signer_retention_epochs = 2

[[env.testnet.validators]]
name = "1"
key = "acme-v1"                      # and 2, 3

[env.testnet.relayer]
account = "acme-relayer"

[env.testnet.host]
provider = "ssh"
address = "deploy@lane.example"      # or a VM name with transport = "gcloud-iap"
public_url = "https://lane.example"  # served by Caddy: the lane's API and the validators' public API
```

The identities must exist in your keystore and hold testnet XLM: `caravel keys list --env testnet` shows them, and friendbot funds them.

## Plan and apply

```bash
caravel plan --env testnet --diff     # every step, and each file it would write as a diff
caravel apply --env testnet
caravel status --env testnet
```

`apply` deploys the settlement contract on testnet, installs the release on the host, writes the node configs and the systemd units and Caddyfile (or the compose file, for containers), streams the validators' and relayer's keys over the ssh connection into the host's `keys/` (mode 600), and starts the nodes. Keys never touch your disk or a command line.

On a machine that isn't x86_64 Linux, install the release built by CI for the host (`--release-dir`). The plan refuses a binary built for another platform.

## Upgrade

A new release is another `apply` with `--release-dir`. It installs the release and restarts the nodes; the stores stay. To review first and then apply exactly what you reviewed, use `caravel plan --out` and `caravel apply <plan file>`.
