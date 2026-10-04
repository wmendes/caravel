---
title: Machines on Google Cloud
sidebar_position: 3
description: "Make the VM a lane runs on with OpenTofu, then hand its address to the lane file."
---

Caravel deploys a lane onto machines; it doesn't make them. The repository ships infrastructure as code for that part, written for [OpenTofu](https://opentofu.org), under `infra/opentofu/`. You describe the machine there and the lane in its lane file. The machine's outputs become the lane file's vars.

| Layer | Tool | Owns |
|---|---|---|
| Machines | OpenTofu | VM, static address, firewall, Docker, budget |
| Containers | Docker | how each node runs (`runtime = "docker"`) |
| Lane | `caravel` | contracts on Stellar, keys, signers, node configs, releases |

## What's there

- **`modules/caravel-host-gcp`**: one VM (Ubuntu 24.04, e2-small by default) with a static address, ports 80 and 443 open to the web, and ssh only through IAP. Its startup script installs Docker and the Compose plugin at pinned versions, makes `/opt/caravel` for the containers' user and adds swap.
- **`modules/billing-cap`**: a monthly budget with alerts, and a function that detaches billing from the project when the budget runs out, which stops everything in it. Budget alerts lag real costs, so treat it as a backstop, not an exact limit.
- **`envs/caravel-testnet`**: lane #1's project. Copy it as the start of your own.

## A new project

You need [OpenTofu](https://opentofu.org/docs/intro/install/) 1.13, the gcloud CLI, logged in with `gcloud auth application-default login`, and a project with billing.

1. Copy `envs/caravel-testnet` to `envs/<yours>`. Set the project, its number, the billing account and the zone in `main.tf`, and the state bucket's name in `versions.tf`. Delete `imports.tf`, which adopts lane #1's existing resources, and the module arguments that keep lane #1's old names (`address_name`, `web_rule_name`, `ssh_rule_name`, `ssh_rule_on_tag`, `install_docker`, `existing_source`). Give `billing_cap` a `source_bucket` to upload the function's code to.
2. Make the state bucket once:

   ```sh
   PROJECT=<project> BUCKET=<bucket> ./infra/opentofu/bootstrap-state.sh
   ```

3. Plan, read it, apply:

   ```sh
   tofu -chdir=infra/opentofu/envs/<yours> init
   tofu -chdir=infra/opentofu/envs/<yours> plan -out machines.plan
   tofu -chdir=infra/opentofu/envs/<yours> apply machines.plan
   ```

## Hand the machine to the lane

The env's `caravel_vars` output is a vars file:

```sh
tofu -chdir=infra/opentofu/envs/<yours> output -raw caravel_vars > hosts.vars.toml
```

```toml
host_address = "caravel-1"
host_project = "caravel-testnet"
host_zone = "us-central1-a"
public_url = "https://35-224-76-64.sslip.io"
```

Declare those vars in the lane file and use them in the host (see [Vars](../reference/lane-file/composition.md#vars)):

```toml
[env.testnet.host]
provider = "ssh"
transport = "gcloud-iap"
runtime = "docker"
address = "${var.host_address}"
project = "${var.host_project}"
zone = "${var.host_zone}"
public_url = "${var.public_url}"
```

Then deploy the lane as usual, with the file:

```sh
caravel plan lane.toml --env testnet --var-file hosts.vars.toml
caravel apply lane.toml --env testnet --var-file hosts.vars.toml
```

The `public_url` is the address's [sslip.io](https://sslip.io) name, so Caddy can get a certificate without any DNS to set up. Use your own domain by pointing it at the address and changing the var.

## Changing the machine

- A bigger machine type is an in-place change: OpenTofu stops the VM, changes it and starts it again. The lane's stores live on the disk, so the nodes pick up where they stopped.
- The boot disk is never replaced from here, since a new disk would mean a new VM without the stores. A newer Ubuntu image is ignored once the VM exists. To grow the disk, resize it with `gcloud compute disks resize`, then set `disk_gb` to match.
- Removing the env (`tofu destroy`) deletes the VM and its disk, stores included. Wind the lane down first ([Wind a lane down](wind-down.md)). The state bucket can't be destroyed from the plan.
