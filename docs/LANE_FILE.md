# The lane file

A lane file declares one lane and its deployments. `caravel` reads it for every command: `plan`, `apply`, `status`, `destroy`, and the commands users run against a lane. This page covers the language: what a lane file holds, how deployments compose, and what expressions can compute.

Decisions behind it: DEC-066 to DEC-068 (deployments), DEC-078 to DEC-082 (the language), in [`CARAVEL_SPEC.md`](CARAVEL_SPEC.md) §22.

## Contents

- [Two halves: genesis and deployments](#two-halves-genesis-and-deployments)
- [Finding the file and the deployment](#finding-the-file-and-the-deployment)
- [A deployment](#a-deployment)
- [Several hosts](#several-hosts)
- [Several lanes on one host](#several-lanes-on-one-host)
- [Declared accounts](#declared-accounts)
- [Declared tokens](#declared-tokens)
- [Declared contracts](#declared-contracts)
- [Modules](#modules)
- [Composition: include and extends](#composition-include-and-extends)
- [Vars](#vars)
- [Locals](#locals)
- [Expressions](#expressions)
- [for_each](#for_each)
- [Attributes: values known after reading Stellar](#attributes-values-known-after-reading-stellar)
- [Outputs](#outputs)
- [Checking what a file means](#checking-what-a-file-means)
- [Resources, targets and replacements](#resources-targets-and-replacements)
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
- `host = { provider, address, transport, project, zone, public_url, root, private_address, namespace }` (or `hosts`, [below](#several-hosts)):
  - `address` is `user@host` for ssh, or the VM name with `transport = "gcloud-iap"`;
  - `public_url` is where the lane's API is served, and the user commands use it.

**Settlement tokens:**

| `token =` | Settles in |
|---|---|
| `"circle-usdc"` | Circle's testnet USDC |
| `{ asset = "CODE:G…" }` | A Stellar asset, through its Stellar Asset Contract |
| `{ contract = "C…" }` | A SEP-41 token contract |
| `{ local = "CODE" }` | A test asset issued by the admin, on a local network |
| `"usd"` | A token the deployment declares (`[env.<name>.tokens.usd]`), issued by the admin or a `G…` address |

**What a change does:** a validator swap becomes a signer rotation, and a `[node]` change becomes a restart. Anything the settlement contract fixed at deploy is refused: the lane's rules, the engine, the admin, the token and the settlement params.

## Several hosts

A deployment can spread its nodes over several hosts. Declare each one under `hosts` instead of a single `host`, and say where each node runs:

```toml
[env.testnet.sequencer]
host = "a"                         # the sequencer's host also runs the relayer
key = "acme-sequencer"             # signs its requests to the validators

[[env.testnet.validators]]
name = "3"
key = "acme-v3"
host = "b"                         # default: the sequencer's host

[env.testnet.hosts.a]
provider = "ssh"
address = "ops@a.example"
public_url = "https://lane.example"
private_address = "10.0.0.2"       # where the other hosts reach this one's nodes

[env.testnet.hosts.b]
provider = "ssh"
address = "ops@b.example"
public_url = "https://b.example"   # optional: validator 3's public API
private_address = "10.0.0.3"
```

- **What each host gets:**
  - The sequencer's host runs the sequencer, the relayer and the validators placed there.
  - Every other host gets `lane.toml` and its validators' configs and unit. With a `public_url`, it also gets a Caddyfile serving their public API.
  - A validator's key goes only to its own host.
- **Signed requests:** once a validator runs on another host or elsewhere, `[sequencer] key` is required. It names an identity whose key signs every request the sequencer sends to a validator's `/v1/sign`. Validators answer only those requests, and refuse any request signed more than 60 s ago. The key is written only to the sequencer's host.
- **Reaching across hosts:**
  - With a `private_address`, another host's nodes are reached there, and the nodes they call listen there.
  - Without one, they go through the host's `public_url`: `<url>` for the sequencer, `<url>/validators/<name>` for a validator. That host's proxy then passes the validator's `/v1/sign`, which answers only signed requests. Everything else stays on 127.0.0.1.
  - A validator on another host must reach the sequencer, and the sequencer must reach it. If neither address works, `caravel validate` says what is missing.
  - Two `local` hosts are the same machine, so they need no address. Their state is under `.caravel/<lane>/<env>` and `.caravel/<lane>/<env>@<host>`.
- **A validator someone else runs:** give it a `url` instead of a host. `key` names an identity added from its operator's public key:

  ```toml
  [[env.testnet.validators]]
  name = "partner"
  key = "partner-v"                  # stellar keys add partner-v --public-key G…
  url = "https://validator.partner.example"
  ```

  It joins the signer set and the sequencer asks it to sign; nothing is deployed for it. Its operator runs `caravel-<template>-node validator` with `sequencer_key` set to the sequencer's public key (`${node.sequencer.key}`, for an output). See the RUNBOOK.
- **Addresses:** the sequencer's host keeps the one-host addresses (`release`, `file.<path>`). Another host's resources are `host.<h>.release`, `host.<h>.file.<path>` and `host.<h>.data`.
- **Moving a node:** change its `host` and apply. The plan starts it on its new host, then stops it where it ran (`node.validator-3@a`), and its key leaves that host. Take a host out of the file only after moving its nodes off, because a host that isn't in the file isn't read.
- **One host:** `host = { … }` is still one host. Nothing about it changes.

## Several lanes on one host

Give each lane that shares an ssh host a `namespace`:

```toml
[env.testnet.host]
provider = "ssh"
address = "ops@lanes.example"
public_url = "https://pay.lanes.example"
namespace = "pay"                  # units caravel-pay-*, root /opt/caravel-pay
```

- **What it sets apart:**
  - Its units are `caravel-pay-sequencer`, `caravel-pay-relayer` and `caravel-pay-validator@<n>`.
  - Its root is `/opt/caravel-pay` unless you give `root`.
  - Its Caddy site goes in `/etc/caddy/caravel.d/pay.caddy`. The host's `/etc/caddy/Caddyfile` must contain `import /etc/caddy/caravel.d/*.caddy`, or the plan says the host lacks it.
- **Each lane needs its own ports and its own `public_url`.**
- **Without a namespace,** a lane has the host to itself: units `caravel-*`, the whole Caddyfile, and `/opt/caravel`.
- **Checked on every plan:**
  - If the root holds another lane, or the unit names run another root, the plan says the host is taken (`HostTaken`).
  - If something else answers on a node's port, the plan says the port is taken (`PortInUse`).
  - Either one refuses apply.
- **`local` hosts** need no namespace, because each lane already has its own `.caravel/<lane>/<env>`. They still check ports.

## Declared accounts

A deployment can declare the Stellar accounts it needs beyond its admin and relayer: test users, a market maker, a treasury. `apply` makes each one exist, trust its assets and hold at least its balance:

```toml
[env.local.accounts.alice]
identity = "acme-alice"            # a Stellar CLI identity; the account's name when not given
fund = true                        # friendbot funds it when it's missing (the default)
trustlines = ["settlement"]        # "settlement", a declared token's name, or "CODE:G…"
balances = { settlement = "100" }  # top up to at least this, in token units (settlement or a declared token)
depends_on = ["account.bob"]       # applied after these addresses

[env.local.accounts.bob]
trustlines = ["settlement"]
```

- **Balances** are only ever topped up, never taken away. A top-up is minted by the token's issuer, so the lane file must have that issuer: the admin (who issues a `{ local = "USDC" }` token) or a declared account that issues a declared token. If it doesn't (Circle's USDC, say), the plan reports a problem and the account needs funding by hand (`caravel account fund`).
- **No float:** amounts are decimal strings in token units.
- **Each account is a resource,** `account.<name>`. Its plan lines are `fund`, `trust` and `mint`. It runs after the token when it trusts or holds it, and after `depends_on`. `--target account.alice` plans just it and what it needs.
- **Expressions** read `account.<name>.identity` and `account.<name>.public_key`.
- **Identities:** on a local network, `apply` creates missing ones, as for the deployment's own. On testnet, `caravel keys ensure` creates them first.
- **Names:** `admin` and `relayer` are taken, and a key in place of an identity is refused.

## Declared tokens

A deployment can declare Stellar assets: a second token for its users, or its own settlement token on testnet. `apply` deploys each one's Stellar Asset Contract when the network has none (anyone may deploy one; the admin pays):

```toml
[env.testnet.tokens.usd]
code = "USD"                       # 1–12 letters or digits
issuer = "admin"                   # "admin", a declared account's name, or a G… address

[env.testnet.tokens.eur]
code = "EUR"
issuer = "treasury"                # [env.testnet.accounts.treasury]

[env.testnet]
token = "usd"                      # settle in a declared token
```

- **Each token is a resource,** `token.<name>`, after its issuer's account. Accounts that trust or hold it come after it. The settlement token stays `token.settlement`, even when it is a declared one.
- **Settling in a declared token:** the admin or a `G…` address must issue it, since the lane's addresses derive from the admin alone. Issued by the admin, it is `CODE:<admin>`, which a top-up can mint, on testnet too.
- **Minting:** a declared account that issues a token mints the top-ups of the accounts that hold it. A token issued by a `G…` address can't be minted from the file.
- **Expressions** read `token.<name>.address`, `.asset`, `.code` and `.issuer`.

## Declared contracts

Any Soroban contract the deployment needs (an oracle, a registry, a vault of its own), deployed once at an address derived from its deployer and a salt:

```toml
[env.local.contracts.oracle]
wasm = "wasm/oracle.wasm"          # relative to the lane file, or the sha256 of Wasm already uploaded
deployer = "admin"                 # or a declared account (default: the admin)
salt = "v1"                        # part of its address: change it for a new contract
depends_on = ["token.usd"]
lifecycle = { prevent_destroy = true }

[env.local.contracts.oracle.args]  # its constructor's arguments, by name
admin = "${account.admin.public_key}"
asset = "${token.settlement.address}"
decimals = 7
feeds = ["BTC", "ETH"]             # lists and tables go as JSON
```

- **One address per name and salt.** The address is `contract_id(deployer, sha256("caravel/contract" ‖ lane id ‖ name ‖ 0 ‖ salt))`, which `plan` knows before anything is sent. `apply` uploads the Wasm when the network lacks it, then deploys. A deployed contract running other Wasm is a problem, not an upgrade.
- **Arguments are set once.** Stellar keeps no record of a constructor's arguments, so they are used when the contract is deployed and never checked again. `plan` notes this for a deployed contract that has some. For other arguments, deploy a new contract with a new `salt`. `--replace contract.<name>` is refused for the same reason.
- **Order:** a contract follows its deployer, every resource whose address one of its arguments names (an account, a token, another contract), and its `depends_on`.
- **Expressions** read `contract.<name>.address` and `.deployer`, in arguments, relayer feeds (`reflectorContract = "${contract.oracle.address}"`) and outputs.
- **`lifecycle.prevent_destroy`** on a contract or on the deployment makes `caravel destroy` refuse.

## Modules

A module packages accounts, tokens and contracts behind inputs and outputs, so a deployment can use it once or many times. It is a local `.toml` file (no remote sources):

```toml
# modules/feed.toml
[inputs.symbol]
type = "string"                    # string, integer, boolean, list, map or any

[inputs.decimals]
type = "integer"
default = 7

[locals]
code = "${upper(input.symbol)}"

[accounts.feeder]                  # identity: <instance>-feeder, unless given

[tokens.coin]
code = "${local.code}"
issuer = "feeder"                  # its own account, by its short name

[contracts.feed]
wasm = "feed.wasm"                 # next to the module file
deployer = "feeder"
args = { admin = "${account.feeder.public_key}", asset = "${token.coin.address}", decimals = "${input.decimals}" }

[outputs]
feed = "${contract.feed.address}"
```

```toml
# the lane file
[env.local.modules.oracle]
source = "modules/feed.toml"       # relative to the lane file
for_each = "${['btc', 'eth']}"     # optional: one instance per item, oracle-btc and oracle-eth
inputs = { symbol = "${each.value}" }

[outputs]
btc_feed = "${module.oracle-btc.feed}"
```

- **What a module file holds:** `[inputs]`, `[locals]`, `[accounts]`, `[tokens]`, `[contracts]` and `[outputs]`, nothing else.
- **What its expressions read:** `input`, its own `local`, `lane` and `env`, and the deployment's `account`, `token`, `contract`, `network` and the other names known later. Its own resources answer to their short names. It doesn't see the lane file's `var`; what it needs comes in as inputs.
- **Addresses:** its resources join the deployment as `module.<instance>.<kind>.<name>`, so `--target 'module.oracle-btc.*'` plans one instance. Inside the module, `depends_on` may name its own (`token.coin`) and the deployment's (`account.admin`).
- **Outputs:** an instance's outputs are `module.<instance>.<name>` in the deployment's `[outputs]`.
- **Names:** a `.` in a declared name is reserved for modules.

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

These are read in a second pass, after `lane.name`, `lane.template`, `lane.id` and `lane.engine_wasm_hash`, which are known from the start. Inside a deployment they may only be used under `relayer.feeds` (a feed that needs a contract id, say) and in a contract's `args`. Anywhere else an error points at the line and says why. Outputs can use them all.

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
