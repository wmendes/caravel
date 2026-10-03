---
title: Resources, targets and replacements
sidebar_label: Resources and targets
sidebar_position: 10
description: "Addresses, --target, --replace, saved plans, and checking what a file means."
---

## Checking what a file means

| Command | What it does |
|---|---|
| `caravel validate` | Reads the file and every deployment offline, and reports every problem with `file:line:col` |
| `caravel render` | The deployment as `plan` reads it: includes, inheritance, vars and expressions resolved |
| `caravel render --genesis` | The exact document the template hashes and the hosts get as `lane.toml` |
| `caravel plan` | What `apply` would change, on Stellar and on the host, each step with its resource's address. `--exit-code` exits 3 when there are changes; `--json` is `caravel-plan/1` |
| `caravel graph` | The deployment's resources and what each depends on, as Graphviz DOT (or `--json`) |

Errors point into the file: the line and caret, where an inherited value came from (`from [env.base]`), and a did-you-mean for names, fields, functions, deployments and vars.

## Resources, targets and replacements

`plan` compares a graph of resources, each with an address:

| Address | What |
|---|---|
| `module.<instance>.<kind>.<name>` | A module's account, token or contract |
| `account.admin`, `account.relayer`, `account.<name>` | The accounts the deployment pays from, and the ones it declares |
| `token.settlement`, `token.<name>` | The settlement token, and the tokens the deployment declares |
| `wasm.settlement`, `contract.settlement`, `contract.<name>` | The settlement contract and its Wasm, and the contracts the deployment declares |
| `signers.settlement` | The signer set the contract checks |
| `host`, `host.data`, `release` | The host's readiness, the lane's stores, the release it runs |
| `file.<path>` | A generated file (`file.sequencer.toml`, `file.validator-2.toml`) |
| `node.<name>` | A node (`node.sequencer`, `node.validator-2`, `node.relayer`) |

Edges say what comes first (the contract before the nodes, the validators before a signer rotation, the rotation before the sequencer) and what restarts what (a file restarts the nodes that read it). `caravel graph | dot -Tsvg > lane.svg` draws it.

| Option | Effect |
|---|---|
| `--target ADDR` | Plan and apply only that resource and what it depends on. `*` matches any characters: `--target 'node.validator-*'` |
| `--replace ADDR` | Replace a node, a file or the release even when it matches. A node restarts; a file is written again and its nodes restart; the release is installed again and every node restarts. The settlement contract, accounts, the token and the signer set can't be replaced, and the error says why |

Both are repeatable and work with `plan` and `apply`. After a targeted apply, the check that follows is targeted too: the rest of the deployment may still differ.

**Saved plans.** `caravel plan --out plan.json` saves the plan; `caravel apply plan.json` applies exactly it, or refuses and says what moved:
- a file the lane file loaded, or a var file;
- a var's value (the plan's vars come back by themselves; a sensitive one must be given again, as when planning);
- what the lane file resolves to (a setting, a key, an address or the release);
- Stellar, or the host, since the plan;
- the steps a plan computed now would take.

A saved plan holds hashes and public data only: no key, no file's content, and a sensitive var only as a salted hash. It keeps its `--target` and `--replace`, records the caravel version that made it (another version refuses it), and is marked applied once it is, so it can't run twice.

