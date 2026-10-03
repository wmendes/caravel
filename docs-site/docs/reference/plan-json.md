---
title: Plan JSON
sidebar_position: 4
description: "caravel plan --json: the caravel-plan/1 format."
---

`caravel plan --json` prints the plan as one JSON object, format `caravel-plan/1`:

```json
{
  "format": "caravel-plan/1",
  "lane": "acme-pay",
  "env": "testnet",
  "network": "testnet",
  "lane_id": "<hex>",
  "config_hash": "<hex>",
  "settlement": "C…",
  "settlement_pinned": false,
  "token": "C…",
  "target_epoch": 1,
  "targets": [],
  "replaced": [],
  "notes": [],
  "steps": [
    { "address": "account.admin", "change": "+", "action": "fund", "line": "+ fund admin G… (friendbot)" }
  ],
  "problems": [
    { "address": "contract.settlement", "message": "…" }
  ]
}
```

| Field | Meaning |
|---|---|
| `steps[].address` | The resource the step changes, as `--target` and `--replace` take it: `account.<name>`, `token.<name>`, `contract.<name>`, `signers.settlement`, `release`, `file.<path>`, `node.<name>`, `host.<h>.…` on another host, `module.<instance>.<kind>.<name>` |
| `steps[].change` | `+` creates, `~` changes, `-` removes |
| `steps[].action` | What the step does (`fund`, `deploy`, `write`, `start`, `rotate`…) |
| `steps[].line` | The step as `plan` prints it |
| `problems[]` | What blocks apply, with the resource it's about. Apply refuses while there is one |
| `targets`, `replaced` | The `--target` and `--replace` addresses this plan was narrowed to or forced with |
| `target_epoch` | The signer-set epoch the lane will be on after apply |

A saved plan (`--out`) is a different format, `caravel-saved-plan/1`. It records what the plan was computed from, so `apply` can refuse when anything moved.
