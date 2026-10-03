---
title: Composition, vars and for_each
sidebar_label: Composition and vars
sidebar_position: 7
description: "include, extends, vars, locals and for_each: one file for every deployment."
---

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

## for_each

A table with `for_each` becomes a list of tables, one per item, with `each.key` (the index, or the map key) and `each.value` in scope:

```toml
[env.base.validators]
for_each = "${var.validators}"
name = "${each.value}"
key = "acme-v${each.value}"
```

It works for a key (as above) and for an element of an array of tables. It makes at most 1,024 instances, and `for_each` must be known before anything is read from Stellar.

