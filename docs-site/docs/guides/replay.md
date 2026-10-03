---
title: Replay a lane from Stellar
sidebar_position: 7
description: "Rebuild a lane from Stellar data alone, check every checkpoint, and build an escape proof."
---

Anyone can check a lane without trusting its operators: `replay` rebuilds it from the checkpoints on Stellar and checks every hash.

```bash
caravel replay --env testnet
caravel replay --env testnet --prove-escape G…        # and that account's escape proof
```

It takes the network, the settlement address, the genesis and the engine from the lane file and the release. The first line of output is the report: `ok`, the checkpoints replayed and the final state hash, or the first mismatch. `--prove-escape` and `--prove-withdrawals` add that account's proofs, so a user can claim with nothing but Stellar RPC and this CLI.

:::note
Stellar RPC keeps transactions for about 7 days on testnet, and replay needs every checkpoint since genesis. For an older lane, the validators' stores are its history.
:::
