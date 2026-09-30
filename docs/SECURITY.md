# Security review (T-016)

The spec §24 checklist, walked on 2026-09-29 against the M0 code. Each item names its evidence, or what was fixed or filed. This is an internal review of a testnet demo, not an audit.

## Checklist

| # | Item (§24) | Result | Evidence |
|---|---|---|---|
| 1 | Every settlement check in §13.3 has a negative test | ✅ | `platform/contracts/settlement/src/test/checkpoint.rs`: `check_1_*` to `check_8_*`, at least one rejection each (check 9 is the effects, covered by `happy_path_deposit_checkpoint_claim` and `checkpoints_chain`) |
| 2 | `claim_withdrawal` and `escape_claim` pay only the owner's `G...` address | ✅ | `claims.rs` `wrong_recipient_rejected`, `both_claims_pay_their_owners`; `freeze.rs` `escape_claim_rejections` (wrong recipient); the `ACCOUNT_XDR_PREFIX` test in `misc.rs` |
| 3 | No path lets the relayer or sequencer alter a signed header or its batch | ✅ | Validators sign `H(header)` and the header commits to `H(batch)`; the contract rejects a changed batch (`check_5_rejects_a_batch_that_does_not_match`) and a bad signature (`check_7_bad_signature_traps`). The relayer only forwards bytes |
| 4 | Validators persist `(seq, header_hash)` before replying and refuse a different header for a signed seq | ✅ | `lanes/perps/node/tests/validator.rs` `never_two_headers_for_one_seq`; `Follower::sign` stores before it returns |
| 5 | Inbox accumulator matches between contract, engine and relayer | ✅ | `test-vectors/inbox_msg.json` checked by `caravel-types`, the relayer (`codec.test.ts`) and the engine; the contract's own chain in `misc.rs`; the local and testnet end-to-end runs deposit through the real contract |
| 6 | Integer overflow never wraps | ✅ | `overflow-checks = true` in the release profile (the engine Wasm too); `fixed` helpers return `Err`, which the engine turns into a rejection or `Fatal`; no `wrapping_*`/`overflowing_*` in the consensus crates; property tests (§19.3) |
| 7 | The engine rejects oracle keys not in config; the sequencer never includes an unverified signature | ✅ | `fatal_cases` scenario (`UNKNOWN_ORACLE_KEY`); `mempool::prevalidate` checks user signatures with `verify_strict` (`prevalidation_rejects_what_the_engine_would_not_take`, API test `BAD_SIGNATURE`); **added** `oracle_updates_need_a_configured_key_and_a_valid_signature` for the oracle path |
| 8 | Session keys cannot withdraw or manage keys; expiry ≤ 7 days | ✅ | `session_key_permissions` scenario; `MAX_SESSION_MS` in `lanes/perps/engine/crates/caravel-perps/src/user.rs` |
| 9 | Engine Wasm hash checked at node startup against config and on-chain `engine_wasm_hash` | ✅ fixed | Every node checked the file against its config. **Added:** a validator with `rpc_url` reads the contract's `config()` at startup and refuses to start unless `lane_id`, `engine_wasm_hash`, `config_hash` and `genesis_state_hash` match (`a_validator_refuses_a_contract_committed_to_another_engine`). The sequencer has no RPC access; a wrong engine there gives headers the contract rejects (check 3) and validators refuse |
| 10 | Admin functions emit events and are listed on `/about` as testnet powers | ✅ | `upgrade` emits `UpgradeEvent`, `admin_rotate_signers` emits `admin_rotate`; the About page says the admin can upgrade the contract and rotate validators |
| 11 | TTL extension on every persistent entry the contract reads | ✅ | `storage::get` extends the TTL on each read, `bump_instance` on each call; TTL test in `misc.rs` |
| 12 | Freeze drill executed on testnet; replay OK after freeze | ✅ | `E2E_NETWORK=testnet ./scripts/e2e-local.sh` on a throwaway contract with the settlement Wasm of record: freeze, two escapes, then `replay` `OK seq=1..6` from testnet data (`docs/RESULTS.md`, freeze drill). The demo lane's contract was not frozen: that ends the lane |
| 13 | Secrets only via env or files outside git | ✅ | `.gitignore` covers `*.key`, `keys/`, `.env*`, `*.sqlite`; no tracked key, env or database file; no `S...` secret in the git history (`git log --all -p`); keys reach the VM from the operator's keystore over the ssh connection (`caravel apply`, DEC-069), mode 600 |
| 14 | Dependency audit; soroban-sdk not affected by known advisories | ✅, 3 filed (#20, #21, #22) | below |

## Dependency audit (2026-09-29)

- `cargo audit` (RustSec, 1,277 advisories, 410 crates): **no vulnerabilities**. One warning: `paste` 1.0.15 is unmaintained (RUSTSEC-2024-0436), a build-time proc-macro reached through `ark-ff` in `soroban-env-host`. Filed as #22: nothing to do until upstream moves.
- GitHub advisories for the Stellar crates:
  - `soroban-sdk` GHSA-x2hw-px52-wp4m, GHSA-4chv-4c6w-w254, GHSA-96xm-fv9w-pf3f: the affected ranges end at 25.x, 23.x and 22.x. **28.0.0 is not affected.**
  - `soroban-env-host` GHSA-pm4j-7r4q-ccg8: below 26.0.0. **28.0.2 is not affected.**
  - `stellar-xdr` GHSA-3xhm-p452-9wjh (2026-09-29): `VecM` serde deserialization does not enforce its maximum length, up to 28.0.0; fixed in 28.0.1. **Filed, not reachable here:** the advisory states that decoding with `ReadXdr` is not affected, and Caravel decodes XDR only with `ReadXdr` (the `serde` feature is on only because the Soroban crates enable it). The pin cannot move yet: `soroban-sdk` 28.0.0 and `soroban-env-common` 28.0.2, the latest, both require `stellar-xdr = "=28.0.0"`. Upgrade when they allow 28.0.1 (#20).
- `npm audit`: `platform/relayer` and `lanes/perps/web` have **no vulnerabilities**. `infra/gcp/billing-cap` has 3 moderate reports for one issue: `uuid` < 11.1.1 (GHSA-w5hq-g745-h8pq, missing bounds check when a `buf` argument is passed to v3/v5/v6), through `cloudevents` in `@google-cloud/functions-framework` 5.0.5, the latest. **Filed, not reachable:** our code never calls `uuid`, and `cloudevents` only calls `v4()` without a buffer. The function takes internal traffic only. The suggested fix downgrades the framework to 2.0.0. Filed as #21.

## Other hardening in this pass

- XDR read from Stellar RPC (`getLedgerEntries`, `getTransaction`) is decoded with bounded limits (depth 500, 1 MiB) instead of none, since nodes do not trust their RPC.
- After a signer rotation, the sequencer has checkpoints signed by the old set signed again by the new one (found while writing the rotation runbook; `a_rotation_sends_old_epoch_signatures_back_for_signing`, and step 4b of the end-to-end run).

## The deploy tool (M0.5 P-18, 2026-09-30)

A pass over `platform/crates/caravel-deploy` (plan, apply, status, destroy and the local and ssh providers), with the same rule: evidence, or what was fixed.

| # | Item | Result | Evidence |
|---|---|---|---|
| D1 | No secret in a lane file | ✅ | Key fields must name Stellar CLI identities; a secret key or a seed phrase anywhere in `[env.*]` is refused without being echoed (`tests/manifest.rs` `secrets_are_refused_wherever_they_are`, `key_fields_name_identities_not_keys`) |
| D2 | No secret in a generated file | ✅ | Rendered configs, units and the Caddyfile hold none (`deploy_render.rs` `the_nodes_read_every_rendered_file`); validator keys are separate files, and the sequencer's token and the relayer's keys come from `keys/env` |
| D3 | Secrets never touch this machine's disk or a command line (ssh hosts) | ✅ | `Cli::secret` reads `stellar keys secret` through a pipe; `Ssh::write_keys` sends each key on the connection's stdin to `sudo install -m 600 /dev/stdin`; `CARAVEL_DEBUG` prints commands, which never carry a secret. A local host keeps its keys under `.caravel/<lane>/<env>/keys` (mode 600 in a mode-700 directory, gitignored) |
| D4 | Nothing from the lane file is run as shell code on the host | ✅ fixed | Validator names and identities are checked names; every value in a remote script is single-quoted; units, the Caddyfile and configs travel on stdin. **Fixed:** `host.root` was only required to be absolute, and it is used in `rm -rf <root>/data/*` during wipe. It is now a plain absolute path (letters, digits, `/ _ - .`), with no `..`, and never `/` itself (`every_rule_is_checked`) |
| D5 | Mainnet is refused | ✅ | `network` is `local` or `testnet`; `mainnet`, `pubnet` and `public` are refused by name; the nodes refuse the mainnet passphrase (`check_network`) |
| D6 | Irreversible steps are guarded | ✅ | Freezing happens only in `destroy`, which without `--yes` makes the operator type the lane's name; apply refuses a plan with problems; a change the constructor fixed is a problem, never a redeploy; a signer set that was ever installed is refused before the contract would (`plan-signers-reused.txt`) |
| D7 | What apply does is what the plan said | ✅ | Apply runs the plan it just printed; each Stellar result is checked against it (the uploaded Wasm hash, the deployed address), and apply ends by reading everything back; an interrupted apply converges (P-13, `kill -9` twice) |
| D8 | A plan can't be fooled by another process answering on a node's port | ✅ fixed in P-13 | Health checks compare the validator's key as well as lane, config, settlement and engine |
| D9 | `destroy --pay-out` can pay only owners | ✅ | The contract pays `escape_claim` and `claim_withdrawal` only to the lane account's owner (item 2); the tool passes the owner and never a recipient of its choice |

## Known limits of M0 (by design, spec §2 and §4)

- The Caravel team runs the sequencer and all three validators, so a collusion of the team can sign a bad checkpoint; the escape hatch and public replay are the recourse.
- The settlement admin can upgrade the contract and rotate validators without delay on testnet (§4.3).
- Oracle prices are signed by one team key (DEC-021, DEC-041).
- The public API has no rate limiting beyond per-account mempool limits.
