---
title: Several lanes on one host
sidebar_position: 3
description: "Give each lane a namespace so several share an ssh host."
---

Each lane that shares an ssh host needs a `namespace`:

```toml
[env.testnet.host]
provider = "ssh"
address = "ops@lanes.example"
public_url = "https://pay.lanes.example"
namespace = "pay"
```

A namespace gives the lane:

- its own systemd units (`caravel-pay-sequencer`, `caravel-pay-relayer`, `caravel-pay-validator@<n>`);
- its own root (`/opt/caravel-pay`, unless `root` is given);
- its own Caddy site, in `/etc/caddy/caravel.d/pay.caddy`.

The host's `/etc/caddy/Caddyfile` must `import /etc/caddy/caravel.d/*.caddy`.

Each lane needs its own ports and its own `public_url`. Every plan checks for collisions: if another lane holds the root or the unit names, or something else listens on a node's port, the plan says so and apply refuses.
