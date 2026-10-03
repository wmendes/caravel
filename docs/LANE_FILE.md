# The lane file

A lane file declares one lane and its deployments. `caravel` reads it for every command: `plan`, `apply`, `status`, `destroy`, and the commands users run against a lane. This page covers the language: what a lane file holds, how deployments compose, and what expressions can compute.

Decisions behind it: DEC-066 to DEC-068 (deployments), DEC-078 to DEC-082 (the language), in [`CARAVEL_SPEC.md`](CARAVEL_SPEC.md) §22.

## Contents

- [Two halves: genesis and deployments](#two-halves-genesis-and-deployments)
- [Finding the file and the deployment](#finding-the-file-and-the-deployment)
- [A deployment](#a-deployment)
- [Composition: include and extends](#composition-include-and-extends)
- [Vars](#vars)
- [Locals](#locals)
- [Expressions](#expressions)
- [for_each](#for_each)
- [Attributes: values known after reading Stellar](#attributes-values-known-after-reading-stellar)
- [Outputs](#outputs)
- [Checking what a file means](#checking-what-a-file-means)
- [A worked example](#a-worked-example)

## Two halves: genesis and deployments

| Part | Sections | Rules |
|---|---|---|
| **Genesis** | `[lane]`, `[app]`, `[node]`, `[access]`, `[limits]` and the template's own table (`[perps]`, `[payments]`) | Consensus config. These sections are hashed into the lane's `config_hash`, so they stay literal: a `${` in them is an error. Changing them makes a different lane |
| **Deployments** | `[env.<name>]`, plus the top-level `include`, `vars`, `locals` and `outputs` | Where and how the lane runs: network, keys, token, validators, host. Expressions, inheritance and inputs live here |

`[node]` is the one genesis-side section that isn't consensus. A deployment can override it with `[env.<name>.node]`, which reaches the hosts but never genesis.

There is no state file. The lane file, Stellar and the host are the whole truth, and `plan` reads all three.

## Finding the file and the deployment

**The lane file**, in order:
1. `-f <file or directory>`;
2. `CARAVEL_FILE`;
3. `./lane.toml`;
4. the one `lane*.toml` in the directory;
5. the same, walking up to the repository root or your home directory.

**The deployment**, in order:
1. `--env <name>`;
2. `CARAVEL_ENV`;
3. the deployment marked `default = true`;
4. the only deployment.

`caravel env list` shows them and marks the one picked.

## A deployment

```toml
[env.local]
default = true                     # picked when no --env is given (at most one)
network = "local"                  # "local" or "testnet"; mainnet is refused
admin = "acme-admin"               # a Stellar CLI identity; keys never go in the file
token = { local = "USDC" }         # the settlement token (below)
threshold = 2                      # signature weight a checkpoint needs

[env.local.settlement_params]      # fixed at deploy
force_inclusion_window_secs = 20
escape_timeout_secs = 30
min_rotation_delay_secs = 3600
signer_retention_epochs = 2

[[env.local.validators]]
name = "1"                         # the node is validator-1
key = "acme-v1"                    # identity whose key signs checkpoints
# weight = 1, port = sequencer port + 1 by default

[env.local.sequencer]
port = 18080

[env.local.relayer]
account = "acme-relayer"           # pays for checkpoint transactions

[env.local.host]
provider = "local"                 # or "ssh"
```

**Other keys:**
- `rpc_url` overrides the network's RPC.
- `settlement_wasm` pins a settlement build by its sha256.
- `settlement` names a contract deployed before this tool (lane #1). New lanes derive the address from the admin and the lane's name.
- `validator_polling = { sequencer_ms, stellar_secs }`.
- `sequencer = { port, cors_origins, production }`.
- `relayer = { account, feed_keys, feeds, intervals_ms }`. `feeds` are the template's feed modules; `feed_keys` maps an env var to an identity whose secret a feed reads.
- `host = { provider, address, transport, project, zone, public_url, root }`:
  - `address` is `user@host` for ssh, or the VM name with `transport = "gcloud-iap"`;
  - `public_url` is where the lane's API is served, and the user commands use it.

**Settlement tokens:**

| `token =` | Settles in |
|---|---|
| `"circle-usdc"` | Circle's testnet USDC |
| `{ asset = "CODE:G…" }` | A Stellar asset, through its Stellar Asset Contract |
| `{ contract = "C…" }` | A SEP-41 token contract |
| `{ local = "CODE" }` | A test asset issued by the admin, on a local network |

**What a change does:** a validator swap becomes a signer rotation, and a `[node]` change becomes a restart. Anything the settlement contract fixed at deploy is refused: the lane's rules, the engine, the admin, the token and the settlement params.

## Composition: include and extends

**`include`** loads deployment tables from other files, relative to the including one. TOML requires it before the first table header:

```toml
include = ["envs/testnet.toml"]

[lane]
name = "acme-pay"
```

An included file holds only `[env.*]`, `vars`, `locals`, `outputs` and its own `include`. Each name is defined once across all files, and a cycle is an error.

**`extends`** makes a deployment start from others:

```toml
[env.base]
abstract = true                    # only for extending: not listed, planned or picked
admin = "acme-admin"
threshold = 2

[env.local]
extends = "base"                   # or ["a", "b"]: applied in order, then this table
network = "local"
```

Tables merge key by key. Anything else replaces what it inherits: a value, an array, or an array of tables such as `validators`. `default` isn't inherited. A value of `"${null}"` drops an inherited key.

## Vars

Inputs to a deployment, declared at the top level (or in an included file):

```toml
[vars.validators]
type = "list"                      # string, integer, boolean, list, map or any
default = ["1", "2", "3"]          # literal: no expressions
description = "the validators' names"

[vars.host]
type = "string"
sensitive = true                   # kept out of messages, masked in the plan
validation = [{ condition = "${length(var.host) > 0}", message = "set the ssh host" }]
```

**Where a value comes from**, the last one winning:
1. `default`;
2. `CARAVEL_VAR_<name>`;
3. each `--var-file <file>` (TOML, `name = value`);
4. each `--var name=value`.

A `--var` is read by the var's type: a string as written, anything else as a TOML value (`--var 'validators=["1","2","4"]'`).

A value for an undeclared var is an error with a did-you-mean. A var with no value is an error only in a deployment that uses it. When a file declares vars, `plan` prints them in its header.

## Locals

Named values, computed per deployment in the order they need each other:

```toml
[locals]
prefix = "${lane.name}-${env.name}"
```

They can use `var`, `lane` (`name`, `template`), `env` (`name`) and other locals. A cycle is an error.

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

## for_each

A table with `for_each` becomes a list of tables, one per item, with `each.key` (the index, or the map key) and `each.value` in scope:

```toml
[env.base.validators]
for_each = "${var.validators}"
name = "${each.value}"
key = "acme-v${each.value}"
```

It works for a key (as above) and for an element of an array of tables. It makes at most 1,024 instances, and `for_each` must be known before anything is read from Stellar.

## Attributes: values known after reading Stellar

Some values exist only once the keys, the chain and the release are read. Expressions name them by these roots:

| Root | Fields |
|---|---|
| `lane` | `name`, `template`, `id`, `engine_wasm_hash`, `config_hash`, `genesis_state_hash` |
| `network` | `name`, `passphrase`, `rpc_url` |
| `account.admin`, `account.relayer` | `identity`, `public_key` |
| `token.settlement` | `address`, `asset` |
| `contract.settlement` | `address`, `pinned` |
| `node.sequencer` | `url`, `port` |
| `node.validator-<name>` | `name`, `key`, `url`, `port`, `weight` |
| `validators` | The list of validator nodes |
| `signers.settlement` | `threshold`, `count` |
| `release.commit` | The release the hosts run |

These are read in a second pass, after `lane.name`, `lane.template`, `lane.id` and `lane.engine_wasm_hash`, which are known from the start. Inside a deployment they may only be used under `relayer.feeds` (a feed that needs a contract id, say). Anywhere else an error points at the line and says why. Outputs can use them all.

## Outputs

Values a deployment publishes, for scripts and other tools:

```toml
[outputs]
api = "${node.sequencer.url}"
settlement = { value = "${contract.settlement.address}", description = "the settlement contract" }

[env.testnet.outputs]              # per deployment, over the top-level ones
explorer = "https://stellar.expert/explorer/testnet/contract/${contract.settlement.address}"
```

`caravel output` lists them with the built-in ones (`settlement`, `token`, `api`, `lane_id`, …), and `caravel output NAME` prints one. `status --json` has them under `outputs`. An output computed from a sensitive var is sensitive: masked in listings, printed only when asked for by name.

## Checking what a file means

| Command | What it does |
|---|---|
| `caravel validate` | Reads the file and every deployment offline, and reports every problem with `file:line:col` |
| `caravel render` | The deployment as `plan` reads it: includes, inheritance, vars and expressions resolved |
| `caravel render --genesis` | The exact document the template hashes and the hosts get as `lane.toml` |
| `caravel plan` | What `apply` would change, on Stellar and on the host. `--exit-code` exits 3 when there are changes |

Errors point into the file: the line and caret, where an inherited value came from (`from [env.base]`), and a did-you-mean for names, fields, functions, deployments and vars.

## A worked example

One lane, a local deployment and a testnet one, sharing everything they can:

```toml
[vars.validators]
type = "list"
default = ["1", "2", "3"]
description = "the validators' names; a rotation is --var 'validators=[\"1\",\"2\",\"4\"]'"

[vars.host]
type = "string"
default = "deploy@lane.example"
description = "the testnet host, for ssh"

[locals]
ids = "${lane.name}"

[outputs]
api = "${node.sequencer.url}"
settlement = "${contract.settlement.address}"

[lane]
name = "acme-pay"
# … [app], [node], [access], [limits] and [payments], as `caravel init payments` writes them

[env.base]
abstract = true
admin = "${local.ids}-admin"
threshold = "${length(var.validators) / 2 + 1}"      # a majority: 2 of 3, 3 of 4
relayer = { account = "${local.ids}-relayer" }

[env.base.settlement_params]
force_inclusion_window_secs = 20
escape_timeout_secs = 30
min_rotation_delay_secs = 3600
signer_retention_epochs = 2

[env.base.validators]
for_each = "${var.validators}"
name = "${each.value}"
key = "${local.ids}-v${each.value}"

[env.local]
extends = "base"
default = true
network = "local"
token = { local = "USDC" }
host = { provider = "local" }
node = { checkpoint_every_blocks = 5 }      # this deployment's [node]: not consensus

[env.testnet]
extends = "base"
network = "testnet"
token = "circle-usdc"
sequencer = { port = 8080 }
host = { provider = "ssh", address = "${var.host}", public_url = "https://lane.example" }
```

```sh
caravel plan                                       # the local deployment (default = true)
caravel plan --env testnet --var host=me@my-vm     # the testnet one, on another host
caravel apply --var 'validators=["1","2","4"]'     # replace validator 3: a signer rotation
caravel output settlement
```
