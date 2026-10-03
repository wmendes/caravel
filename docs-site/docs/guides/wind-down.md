---
title: Wind a lane down
sidebar_position: 8
description: "destroy: drain, export every exit, freeze. And how to drill a freeze without ending a lane."
---

```bash
caravel destroy --env testnet
```

1. **Drain** until every signed checkpoint is accepted, and at least one is.
2. **Stop** the sequencer and the relayer.
3. **Export** every account's exit proof to `exit.json`.
4. **Trigger, then wait** out the forced-withdrawal window, so the contract allows a freeze.
5. **Freeze**, so users withdraw on Stellar with their proofs.

Without `--yes`, destroy asks you to type the lane's name. **A frozen lane can't be restarted.** The validators keep serving proofs unless you pass `--stop-validators`, and `--pay-out` claims every exit for its owner.

After the freeze, each user runs `caravel escape <identity>`. It needs only the lane file and the admin's public key, and takes the proof from `exit.json`, a validator, or a replay from Stellar.

## Drill a freeze

Practice on a lane you can lose, never on one with users:

```bash
caravel stop sequencer relayer        # checkpoints stop; it prints when a freeze becomes possible
stellar contract invoke --id <settlement> --source-account <anyone> --network testnet -- freeze
caravel wait frozen
caravel escape alice
```
