---
title: Hosts
sidebar_label: Hosts
sidebar_position: 3
description: "Spreading a lane's nodes over several hosts, and running several lanes on one host."
---

## Several hosts

A deployment can spread its nodes over several hosts. Declare each one under `hosts` instead of a single `host`, and say where each node runs:

```toml
[env.testnet.sequencer]
host = "a"                         # the sequencer's host also runs the relayer
key = "acme-sequencer"             # signs its requests to the validators

[[env.testnet.validators]]
name = "3"
key = "acme-v3"
host = "b"                         # default: the sequencer's host

[env.testnet.hosts.a]
provider = "ssh"
address = "ops@a.example"
public_url = "https://lane.example"
private_address = "10.0.0.2"       # where the other hosts reach this one's nodes

[env.testnet.hosts.b]
provider = "ssh"
address = "ops@b.example"
public_url = "https://b.example"   # optional: validator 3's public API
private_address = "10.0.0.3"
```

- **What each host gets:**
  - The sequencer's host runs the sequencer, the relayer and the validators placed there.
  - Every other host gets `lane.toml` and its validators' configs and unit. With a `public_url`, it also gets a Caddyfile serving their public API.
  - A validator's key goes only to its own host.
- **Signed requests:** once a validator runs on another host or elsewhere, `[sequencer] key` is required. It names an identity whose key signs every request the sequencer sends to a validator's `/v1/sign`. Validators answer only those requests, and refuse any request signed more than 60 s ago. The key is written only to the sequencer's host.
- **Reaching across hosts:**
  - With a `private_address`, another host's nodes are reached there, and the nodes they call listen there.
  - Without one, they go through the host's `public_url`: `<url>` for the sequencer, `<url>/validators/<name>` for a validator. That host's proxy then passes the validator's `/v1/sign`, which answers only signed requests. Everything else stays on 127.0.0.1.
  - A validator on another host must reach the sequencer, and the sequencer must reach it. If neither address works, `caravel validate` says what is missing.
  - Two `local` hosts are the same machine, so they need no address. Their state is under `.caravel/<lane>/<env>` and `.caravel/<lane>/<env>@<host>`.
- **A validator someone else runs:** give it a `url` instead of a host. `key` names an identity added from its operator's public key:

  ```toml
  [[env.testnet.validators]]
  name = "partner"
  key = "partner-v"                  # stellar keys add partner-v --public-key G…
  url = "https://validator.partner.example"
  ```

  It joins the signer set and the sequencer asks it to sign; nothing is deployed for it. Its operator runs `caravel-<template>-node validator` with `sequencer_key` set to the sequencer's public key (`${node.sequencer.key}`, for an output). See the RUNBOOK.
- **Addresses:** the sequencer's host keeps the one-host addresses (`release`, `file.<path>`). Another host's resources are `host.<h>.release`, `host.<h>.file.<path>` and `host.<h>.data`.
- **Moving a node:** change its `host` and apply. The plan starts it on its new host, then stops it where it ran (`node.validator-3@a`), and its key leaves that host. Take a host out of the file only after moving its nodes off, because a host that isn't in the file isn't read.
- **One host:** `host = { … }` is still one host. Nothing about it changes.

## Several lanes on one host

Give each lane that shares an ssh host a `namespace`:

```toml
[env.testnet.host]
provider = "ssh"
address = "ops@lanes.example"
public_url = "https://pay.lanes.example"
namespace = "pay"                  # units caravel-pay-*, root /opt/caravel-pay
```

- **What it sets apart:**
  - Its units are `caravel-pay-sequencer`, `caravel-pay-relayer` and `caravel-pay-validator@<n>`.
  - Its root is `/opt/caravel-pay` unless you give `root`.
  - Its Caddy site goes in `/etc/caddy/caravel.d/pay.caddy`. The host's `/etc/caddy/Caddyfile` must contain `import /etc/caddy/caravel.d/*.caddy`, or the plan says the host lacks it.
- **Each lane needs its own ports and its own `public_url`.**
- **Without a namespace,** a lane has the host to itself: units `caravel-*`, the whole Caddyfile, and `/opt/caravel`.
- **Checked on every plan:**
  - If the root holds another lane, or the unit names run another root, the plan says the host is taken (`HostTaken`).
  - If something else answers on a node's port, the plan says the port is taken (`PortInUse`).
  - Either one refuses apply.
- **`local` hosts** need no namespace, because each lane already has its own `.caravel/<lane>/<env>`. They still check ports.

