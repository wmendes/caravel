---
title: Expressions and attributes
sidebar_label: Expressions
sidebar_position: 8
description: "The ${...} language, and the values known once keys and addresses are."
---

## Expressions

A string holding `${…}` is computed:
- **Types:** a string that is exactly one `${…}` takes the expression's type (`threshold = "${length(var.validators) / 2 + 1}"` is a number). Any other string with `${…}` in it is a string.
- **Escaping:** `$${` writes a literal `${`.
- **Values:** null, booleans, 64-bit integers, strings, lists and maps. TOML floats and dates pass through but can't be computed with.

| Kind | What there is |
|---|---|
| Operators | `?:`, `\|\|` and `&&` (both short-circuit), `== != < <= > >=`, `+ -`, `* / %`, unary `! -` |
| Access | `a.b`, `a["b"]`, `list[0]`, calls |
| Literals | `"…"` and `'…'` strings (they interpolate too), numbers, `true`/`false`/`null`, `[…]` lists, `{ k = v }` maps |
| Comprehensions | `[for v in xs : e]`, `[for k, v in m : e if cond]` |

**Rules:**
- Arithmetic is checked: overflow and division by zero are errors, and `/` truncates.
- `+` adds numbers only; join strings with a template or `format()`.
- `==` compares values of one kind, or anything with `null`. Orderings compare two numbers or two strings.
- Names may contain `-` (`node.validator-1`), so subtraction needs spaces: `a - 1`.

**Functions:**

| Function | Returns |
|---|---|
| `range(end)`, `range(start, end[, step])` | A list of integers, `end` excluded |
| `length(x)` | The size of a list, map or string |
| `concat(l1, l2, …)` | The lists joined |
| `merge(m1, m2, …)` | The maps merged, later keys winning |
| `lookup(map, key[, default])` | A map's value, else the default |
| `keys(map)` | The map's keys |
| `contains(list or string, x)` | Whether it holds `x` |
| `join(sep, list)`, `split(sep, string)` | Strings and lists |
| `replace(s, from, to)`, `upper(s)`, `lower(s)` | Strings |
| `tostring(x)`, `tonumber(s)` | Conversions |
| `min(…)`, `max(…)` | Of numbers |
| `coalesce(a, b, …)` | The first value that isn't null |
| `format("{}-v{}", a, b)` | `{}` filled in order; `{{` and `}}` for braces |

No function reads the clock, randomness, the environment or files. A lane file and its inputs always give the same deployment.

## Attributes: values known after reading Stellar

Some values exist only once the keys, the chain and the release are read. Expressions name them by these roots:

| Root | Fields |
|---|---|
| `lane` | `name`, `template`, `id`, `engine_wasm_hash`, `config_hash`, `genesis_state_hash` |
| `network` | `name`, `passphrase`, `rpc_url` |
| `account.admin`, `account.relayer`, `account.<name>` | `identity`, `public_key` |
| `token.settlement` | `address`, `asset` |
| `token.<name>` | `address`, `asset`, `code`, `issuer` (declared tokens) |
| `contract.settlement` | `address`, `pinned` |
| `contract.<name>` | `address`, `deployer` (declared contracts) |
| `node.sequencer` | `url`, `port`, `key` (the request key validators check, or null) |
| `node.validator-<name>` | `name`, `key`, `url`, `port`, `weight` |
| `validators` | The list of validator nodes |
| `signers.settlement` | `threshold`, `count` |
| `release.commit` | The release the hosts run |

These are read in a second pass, after `lane.name`, `lane.template`, `lane.id` and `lane.engine_wasm_hash`, which are known from the start. Inside a deployment they may only be used under `relayer.feeds` (a feed that needs a contract id, say), in a contract's `args` and in `web.config`. Anywhere else an error points at the line and says why. Outputs can use them all.

