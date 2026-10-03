---
title: Several hosts and outside validators
sidebar_position: 2
description: "Spread a lane's validators over several hosts, or have others run them."
---

A deployment can place each validator on its own host, and include validators that someone else runs.

## Validators on their own hosts

Declare the hosts under `hosts` and place each node:

```toml
[env.testnet.sequencer]
host = "a"                         # the sequencer's host also runs the relayer
key = "acme-sequencer"             # signs its requests to the validators

[[env.testnet.validators]]
name = "3"
key = "acme-v3"
host = "b"

[env.testnet.hosts.a]
provider = "ssh"
address = "ops@a.example"
public_url = "https://lane.example"
private_address = "10.0.0.2"       # optional: a private network between the hosts

[env.testnet.hosts.b]
provider = "ssh"
address = "ops@b.example"
public_url = "https://b.example"
private_address = "10.0.0.3"
```

- **Each host gets only what it runs:** its validators' configs, units and keys.
- **Signed requests.** Once a validator runs on another host, `[sequencer] key` is required. The sequencer signs every request it sends to a validator's `/v1/sign`, and validators answer only those, within 60 seconds of signing.
- **Reaching each other.** With a `private_address`, hosts reach each other's nodes there. Without one, they go through each host's `public_url`.
- **Moving a validator.** Change its `host` and apply: it starts on the new host, then stops on the old one, and its key leaves.

## Validators run by others

A validator someone else operates is a reference, never deployed:

```toml
[[env.testnet.validators]]
name = "partner"
key = "partner-v"                  # stellar keys add partner-v --public-key G…
url = "https://validator.partner.example"
```

It joins the signer set, and the sequencer asks it to sign. Its operator runs the template's node as a validator with `sequencer_key` set to your sequencer's public key; `${node.sequencer.key}` gives it to them as an output. See [Run a validator](./run-a-validator.md).

The [hosts reference](../reference/lane-file/hosts.md) has every rule.
