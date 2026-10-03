---
title: Who you trust
sidebar_position: 3
description: "What a lane's users trust today, what Stellar checks, and what is planned."
---

| When | You trust | Because |
|---|---|---|
| **Between checkpoints** | The validators | They run every block again and sign only what they checked. A threshold of them signing a wrong state could pass it |
| **At each checkpoint** | Their threshold, checked on Stellar | The contract verifies the signatures, and anyone can replay the lane from Stellar to check them |
| **If the lane stops** | No one | Anyone can freeze it, and every account withdraws its last checkpointed balance on Stellar |

## What is true today

- **Testnet only, not audited.** Caravel refuses mainnet passphrases, and its security review is an internal one ([`docs/SECURITY.md`](https://github.com/wmendes/caravel/blob/main/docs/SECURITY.md)).
- **A lane's admin has testnet powers.** It can upgrade the settlement contract and rotate the validators without delay. Both emit events.
- **Caravel Perps' three validators run on one machine**, operated by the Caravel team. A lane you deploy has the validators you declare, on the hosts you choose, or run by others.
- **Requests to validators are signed.** When validators run on other hosts, the sequencer signs every request for a signature, and validators answer only those.
- **Prices are as good as the feed.** On Caravel Perps, one team key signs the oracle prices.

## Planned, not built

- **Validity proofs:** a zero-knowledge proof of each checkpoint would replace the signature check, so the contract checks the state itself.
- **Bonded validators:** anyone could validate, with a bond at stake.

Neither exists yet. Until they do, the trust above is the trust there is.
