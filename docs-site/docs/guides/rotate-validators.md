---
title: Rotate validators
sidebar_position: 4
description: "Replace a validator, or change the threshold, with a lane-file edit."
---

Changing the validators is an edit to the lane file:

```diff
 [[env.testnet.validators]]
-name = "3"
-key = "acme-v3"
+name = "4"
+key = "acme-v4"
```

```bash
caravel plan --env testnet
caravel apply --env testnet
```

`apply` does the rotation in order. It starts the new validator, installs the new signer set on the settlement contract, restarts the sequencer on the new epoch, and stops the old validator. Checkpoints the old set signed but Stellar hadn't accepted go back for signatures under the new set, so nothing is lost. If the run is interrupted, the next one finishes the job.

With vars, a rotation is one flag: `caravel apply --var 'validators=["1","2","4"]'`.

:::caution Testnet admin power
This rotation uses the settlement admin's `admin_rotate_signers`, a testnet power that installs the new set at once. A set that was ever installed can't be installed again: the plan refuses it.
:::
