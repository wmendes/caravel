---
title: Accounts and tokens
sidebar_label: Accounts and tokens
sidebar_position: 4
description: "Accounts a deployment funds, trusts and tops up, and the tokens it issues."
---

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

