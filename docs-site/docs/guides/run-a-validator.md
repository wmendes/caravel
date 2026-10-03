---
title: Run a validator
sidebar_position: 5
description: "Follow a lane, re-execute every block, and, once in its signer set, sign its checkpoints."
---

A validator follows the sequencer's public block API, re-executes every block with the engine Wasm, rebuilds every checkpoint itself, and serves blocks, checkpoints and proofs from its own store. Following a lane needs no permission. Signing needs your key in the lane's signer set.

## A key

```bash
mkdir -p keys && chmod 700 keys
stellar keys generate my-validator
stellar keys secret my-validator > keys/my-validator.key && chmod 600 keys/my-validator.key
```

## A config

```toml
[validator]
listen = "127.0.0.1:8091"
sequencer_url = "https://lane.example"              # the lane's public API
key_file = "keys/my-validator.key"
lane = "lane.toml"                                  # the lane's genesis
engine_wasm = "contracts/payments_engine.wasm"
engine_wasm_sha256 = "<the lane's engine_wasm_sha256>"
db = "data/my-validator.sqlite"
network_passphrase = "Test SDF Network ; September 2015"
settlement_contract = "C…"                          # the lane's settlement contract
rpc_url = "https://soroban-testnet.stellar.org"     # to learn which checkpoints Stellar accepted
sequencer_key = "G…"                                # once you sign: only the sequencer's requests
```

```bash
caravel-payments-node validator --config my-validator.toml
```

It checks that the settlement contract commits to this lane, engine and genesis before it starts. Then it catches up from block 1 and follows live.

## Signing

To sign for a lane, your public key goes into its lane file as a validator run by others (`url` and `key`), and the lane rotates its signer set. Serve `/v1/sign` at that `url` over HTTPS. With `sequencer_key` set, it answers only requests the sequencer signed.

## When it stops or refuses

- **Halted:** a block didn't re-execute to the sequencer's state. It stops and refuses to sign. Keep the store and the log, and compare with another validator.
- **Suspicious block:** a live block failed a policy check, such as a timestamp too far from the validator's clock. The validator keeps following but won't sign that checkpoint until an operator clears it.
- **Never two headers for one checkpoint.** Every signature is stored before it's sent. Never delete a signing validator's database.
