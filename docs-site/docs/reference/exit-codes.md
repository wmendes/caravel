---
title: Exit codes
sidebar_position: 3
description: "How caravel exits, for scripts and CI."
---

| Code | Meaning |
|---|---|
| `0` | Done. With `--exit-code`, also: the deployment matches the lane file |
| `1` | An error, or with `plan --exit-code`, problems that block apply |
| `2` | The command line was refused (an unknown flag, a missing argument) |
| `3` | `plan --exit-code` or `status --exit-code`: the deployment differs from the lane file |
| `4` | `caravel wait`: the time ran out |

```bash
caravel plan --exit-code --env testnet        # in CI: 0 means nothing to apply
```

Every command takes `--json`: JSON on stdout, progress and notes on stderr.
