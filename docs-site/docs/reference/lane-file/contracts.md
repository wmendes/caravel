---
title: Contracts
sidebar_label: Contracts
sidebar_position: 5
description: "Any Wasm contract deployed with the lane, at an address derived from the lane."
---

## Declared contracts

Any Soroban contract the deployment needs (an oracle, a registry, a vault of its own), deployed once at an address derived from its deployer and a salt:

```toml
[env.local.contracts.oracle]
wasm = "wasm/oracle.wasm"          # relative to the lane file, or the sha256 of Wasm already uploaded
deployer = "admin"                 # or a declared account (default: the admin)
salt = "v1"                        # part of its address: change it for a new contract
depends_on = ["token.usd"]
lifecycle = { prevent_destroy = true }

[env.local.contracts.oracle.args]  # its constructor's arguments, by name
admin = "${account.admin.public_key}"
asset = "${token.settlement.address}"
decimals = 7
feeds = ["BTC", "ETH"]             # lists and tables go as JSON
```

- **One address per name and salt.** The address is `contract_id(deployer, sha256("caravel/contract" ‖ lane id ‖ name ‖ 0 ‖ salt))`, which `plan` knows before anything is sent. `apply` uploads the Wasm when the network lacks it, then deploys. A deployed contract running other Wasm is a problem, not an upgrade.
- **Arguments are set once.** Stellar keeps no record of a constructor's arguments, so they are used when the contract is deployed and never checked again. `plan` notes this for a deployed contract that has some. For other arguments, deploy a new contract with a new `salt`. `--replace contract.<name>` is refused for the same reason.
- **Order:** a contract follows its deployer, every resource whose address one of its arguments names (an account, a token, another contract), and its `depends_on`.
- **Expressions** read `contract.<name>.address` and `.deployer`, in arguments, relayer feeds (`reflectorContract = "${contract.oracle.address}"`) and outputs.
- **`lifecycle.prevent_destroy`** on a contract or on the deployment makes `caravel destroy` refuse.

