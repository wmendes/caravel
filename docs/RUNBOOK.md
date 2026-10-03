# Runbook

How to run, deploy and operate Caravel M0, the testnet demo. The spec is `docs/CARAVEL_SPEC.md`; dated numbers are in `docs/RESULTS.md`.

**Who runs what in M0.** The Caravel team runs the sequencer, all three validators and the relayer, on one GCP VM (DEC-046). The settlement contract admin has testnet-only powers: upgrading the contract and rotating validators without delay (spec §4.3). Anyone can follow the lane with their own validator (§2 below), rebuild it from Stellar data with `caravel replay` (§6), and use the escape hatch if the lane stops (§5).

Everything here goes through `caravel`, the one CLI (install it with `./scripts/install.sh`, README). It finds the lane file in the current directory or takes `-f`, picks the deployment with `--env`, and takes `--json` everywhere. The lane file language is in `docs/LANE_FILE.md`.

Every command runs from the repository root unless it says otherwise.

## 0. Tools

| Tool | Version | Install |
|---|---|---|
| Rust | 1.93.0 + `wasm32v1-none` | `rustup` picks both up from `rust-toolchain.toml` |
| Stellar CLI | 28.1.0 | `cargo install --locked stellar-cli@28.1.0` |
| Node.js | 22 or later | nodejs.org |
| Docker | any recent | for the local Stellar network (§1.1) |
| jq | any | for `scripts/e2e-local.sh` |
| curl, openssl | any | package manager; only for the by-hand steps below |
| gcloud | any recent | only to deploy or reach the testnet VM (§3) |

Check the pins with `node scripts/check-versions.mjs`. Build the contracts first; every node checks the engine Wasm hash when it starts:

```sh
./scripts/build-contracts.sh               # target/contracts/{perps_engine,settlement}.wasm, sizes and hashes
cargo build --release --locked -p caravel-perps-node
```

The engine hash is the same on every host. The settlement Wasm of record is the x86_64 Linux build from CI (DEC-033), so on macOS `build-contracts.sh` warns that the settlement hash differs; local runs are unaffected.

## 1. Run locally

### 1.1 The whole lane, one command

The README's quickstart is the short version: `caravel init`, `caravel apply`, a few users, `caravel destroy`. `./scripts/check-quickstart.sh` runs it exactly as written. The end-to-end script is the long version:

```sh
./scripts/e2e-local.sh          # KEEP=1 leaves everything running afterwards
```

Outside its cleanup it uses only `caravel` and `jq` (`scripts/check-e2e.sh` keeps it so). It makes a lane with `caravel init <template> --prefix e2e`, includes the e2e's deployment (`scripts/e2e/env.toml`, with its vars in a `--var-file`), and runs `caravel apply`. That starts a local Stellar network in Docker, deploys a local USDC asset and the settlement contract (30 s escape timeout, DEC-043), and runs the sequencer, three validators and the relayer as local processes. Then it goes through spec §19.5:

1. `caravel account create` and `caravel deposit` 1,000 USDC for A and B;
2. `caravel tx`: A rests a bid and B sells into it (perps), or A pays B (payments);
3. `caravel wait checkpoint`: a checkpoint is accepted on Stellar;
4. `caravel withdraw`: A withdraws 100 USDC and claims it on Stellar;
   - 4b. `caravel stop relayer`, then `caravel apply --var 'validators=["1","2","4"]'` rotates validator 3 out (the procedure in §4.1); a checkpoint signed by the old set is signed again by the new one;
   - 4c. `caravel force-withdraw`: B's forced withdrawal through Stellar is processed and claimed;
5. `caravel destroy` drains the lane, writes `exit.json` and freezes it; `caravel escape` pays A and B pro rata;
6. `caravel replay` rebuilds the lane from Stellar data and reports OK.

It takes about 4 minutes from a clean clone. Logs and state go to a temporary work directory, printed at the end (and on failure, with the tail of each log). The Stellar CLI keys it creates live in that directory, not in your keystore. With `E2E_NETWORK=testnet` the same run goes to Stellar testnet with a settlement contract of its own (§5).

### 1.2 A sequencer alone, no Stellar

For engine or API work. Checkpoints are sealed but never signed or submitted.

```sh
CARAVEL_INTERNAL_TOKEN=$(openssl rand -hex 16) \
  ./target/release/caravel-perps-node sequencer --config lanes/perps/config/sequencer.local.toml
# in another shell: accounts funded through the internal API, prices, orders
CARAVEL_INTERNAL_TOKEN=<same> cargo run --release -p caravel-perps-node --example loadgen -- \
  --url http://127.0.0.1:8080 --lane lanes/perps/config/lane.caravel-perps.local.toml --tps 20 --duration-secs 60
```

State goes to `data/sequencer.sqlite`; delete it to start again from genesis. `caravel-perps-node check-store --config lanes/perps/config/sequencer.local.toml` re-executes the whole store through the engine Wasm and checks every block and checkpoint header.

The web app runs against it with `npm --prefix lanes/perps/web run dev` (see `lanes/perps/web/env.local.example`).

## 2. Run a validator

A validator follows the sequencer's public block API, re-executes every block with the engine Wasm, rebuilds every checkpoint header itself, and serves blocks, checkpoints and escape and withdrawal proofs from its own store (spec §15). It needs no permission to follow the testnet lane. The sequencer only asks the validators in the contract's signer set to sign, and in M0 those are the team's three (§2.6).

### 2.1 Build

```sh
git clone <this repository> && cd caravel
./scripts/build-contracts.sh
cargo build --release --locked -p caravel-perps-node
shasum -a 256 target/contracts/perps_engine.wasm   # must be engine_wasm_sha256 in versions.json
```

Or download the `caravel-release-linux-x86_64` artifact of a CI run on `main` (binary, contracts, relayer and web builds, with `SHA256SUMS`).

### 2.2 A key

The validator signs with an ed25519 key. It only signs when the sequencer asks, but the key file is required.

```sh
mkdir -p keys && chmod 700 keys
stellar keys generate my-validator
stellar keys secret my-validator > keys/my-validator.key && chmod 600 keys/my-validator.key
```

`*.key` and `keys/` are ignored by git. Never commit a key.

### 2.3 Config

`lanes/perps/config/my-validator.toml` (paths relative to the file):

```toml
[validator]
listen = "127.0.0.1:8091"
sequencer_url = "https://35-224-76-64.sslip.io"      # the testnet lane's public API
key_file = "../keys/my-validator.key"
lane = "lane.caravel-perps.testnet.toml"
engine_wasm = "../target/contracts/perps_engine.wasm"
engine_wasm_sha256 = "4571cd252d4b78d523d01fc4a31ab763a1aff77aecafbb9d7302cfb879abbf0a"
db = "../data/my-validator.sqlite"
network_passphrase = "Test SDF Network ; September 2015"
settlement_contract = "CBIHBEUZYFZQZEQPBJH2ID6CDRDZFEDI6XHAXVOCHG6FO5XWUIGPONWO"
rpc_url = "https://soroban-testnet.stellar.org"      # to learn which checkpoint Stellar accepted
stellar_poll_secs = 30
```

The node refuses a mainnet passphrase. Take `engine_wasm_sha256` and the contract ID from `versions.json`.

### 2.4 Run and check

```sh
RUST_LOG=info ./target/release/caravel-perps-node validator --config lanes/perps/config/my-validator.toml
```

It catches up from block 1, then follows live. On 2026-09-29, a laptop following the testnet lane with this exact config caught up about 6,150 blocks in 14 minutes. Compare with the sequencer at the same height:

```sh
curl -s http://127.0.0.1:8091/v1/status | jq '{height, state_hash, halted, checkpoints}'
curl -s https://35-224-76-64.sslip.io/v1/blocks/<height> | jq -r .state_hash_after
```

`state_hash` must equal the block's `state_hash_after`, and `halted` must be `null`. Its `/v1/proofs/escape?account=G...` is an escape proof source independent of the sequencer.

### 2.5 When it stops or refuses

- **Halted** (`halted` is a reason): a block did not continue its chain or did not re-execute to the sequencer's `state_hash_after`. It stops following and refuses to sign. This is the check doing its job: keep the store and the log, compare with another validator (`/validators/N/v1/blocks/<height>` on the testnet host), and report it. A restart re-follows and halts again at the same block if the data is really wrong.
- **Suspicious blocks** (`suspicious_blocks` is not empty): a live block failed a policy check (its timestamp is too far from the validator's clock, or an oracle entry is too old). The validator keeps following but refuses to sign a checkpoint that contains it (`SUSPICIOUS_BLOCK`). After looking at the reason, clear it and restart:

  ```sh
  ./target/release/caravel-perps-node validator-clear --config lanes/perps/config/my-validator.toml --through <height>
  ```

- **Never two headers for one seq.** Every signed `(seq, header_hash)` is stored before the reply, and the validator refuses a different header for a seq it signed (`EQUIVOCATION`). Never delete a signing validator's database to "fix" this; it is what makes signing safe.

### 2.6 Joining the signer set

A validator signs for the lane only when its key is in the settlement contract's signer set. In M0 the set is the team's three keys, weight 1 each, threshold 2, and changing it is a rotation (§4).

## 3. Deploy to testnet

Testnet only. Mainnet passphrases and contract IDs never go into defaults.

### 3.1 Contracts (T-012, done once)

```sh
gh run download <main run id> -n contracts-wasm -D /tmp/wasm
WASM_DIR=/tmp/wasm ./scripts/deploy-testnet.sh
```

- It checks both Wasm hashes against `versions.json` (the x86_64 Linux CI build is the one of record).
- It creates and funds the `caravel-admin`, `caravel-relayer` and `caravel-validator-1..3` identities in your Stellar CLI keystore if missing. `caravel-backstop`, `caravel-treasury` and `caravel-oracle` must match the lane file.
- It deploys the engine and settlement contracts, sends the witness `step` transaction and prints what goes into `versions.json` and `docs/RESULTS.md`.
- It refuses to redeploy when `versions.json` already has contract IDs, unless `FORCE=1`. A new settlement contract is a new lane deployment: every node store starts again from genesis.

### 3.2 The VM (DEC-046)

One e2-small VM (`caravel-1`, `us-central1-a`, project `caravel-testnet`) runs everything behind Caddy. The only public listener is HTTPS on the static IP's `sslip.io` name. SSH goes through IAP only.

First time:

```sh
gcloud compute scp lanes/perps/deploy/testnet/provision.sh caravel-1:/tmp/ --zone us-central1-a --project caravel-testnet --tunnel-through-iap
gcloud compute ssh caravel-1 --zone us-central1-a --project caravel-testnet --tunnel-through-iap --command 'sudo bash /tmp/provision.sh'
```

`provision.sh` installs Caddy and Node.js (checked against `SHASUMS256.txt`), and creates the unprivileged `caravel` user, `/opt/caravel` and 1 GB of swap.

Deploy or upgrade a release with the deploy tool (M0.5 P-16, DEC-070). Lane #1's deployment is the `[env.testnet]` table in `lanes/perps/config/lane.caravel-perps.testnet.toml`:

```sh
gh run download <run id> -n caravel-release-linux-x86_64 -D /tmp/release
L=lanes/perps/config/lane.caravel-perps.testnet.toml
./target/release/caravel plan $L --env testnet --release-dir /tmp/release --diff   # what would change; changes nothing
./target/release/caravel apply $L --env testnet --release-dir /tmp/release         # asks, then installs and restarts
./target/release/caravel status $L --env testnet --release-dir /tmp/release
```

- The plan reads Stellar and the VM (over `gcloud compute ssh --tunnel-through-iap`) and lists every step. A release step installs the binary, contracts, relayer (with its `node_modules`), feed module and web app, then restarts every node. Nothing on Stellar changes unless the lane file's signers changed.
- The keys come from your Stellar keystore (`caravel-validator-1..3`, `caravel-relayer`, `caravel-oracle`). They stream over the ssh connection into `/opt/caravel/keys` (mode 600, owner `caravel`), and the internal API token there is kept. Secrets never enter git, the release or your disk.
- The stores in `/opt/caravel/data` stay. A new release must open them. Check the store schema in `platform/crates/caravel-runtime/src/store.rs` before deploying one that changes it.
- For a release that could change execution (the engine or the lane's consensus sections), first re-execute a copy of the live store on the VM with the new binary: `caravel-perps-node check-store --config <a sequencer config pointing at the copy>`. The human chose not to require this for other releases (P-11).
- To roll back, apply with the previous release.

Status and logs:

```sh
curl -s https://35-224-76-64.sslip.io/v1/status | jq '{height, checkpoints, mempool, halted}'
gcloud compute ssh caravel-1 --zone us-central1-a --project caravel-testnet --tunnel-through-iap \
  --command 'systemctl is-active caravel-sequencer caravel-validator@{1,2,3} caravel-relayer caddy; sudo journalctl -u caravel-relayer -n 20 --no-pager'
```

### 3.3 Budget

The project has a monthly budget of R$100 with alerts at 50%, 90% and 100%. At 100%, the `stop-billing` Cloud Run function detaches billing from the project, and Google then shuts down its paid resources, the VM included (`infra/gcp/setup-billing-cap.sh`). To bring it back, relink billing (`gcloud billing projects link caravel-testnet --billing-account <id>`) and start the VM. The expected cost is about US$12 a month for the VM and its disk and IP.

## 4. Rotate keys

### 4.1 A validator key (testnet-only admin rotation)

`admin_rotate_signers(new)` installs a new signer set at once and makes every older set invalid immediately (`MinValidEpoch = Epoch`, spec §13.2). It cannot reuse a set that was ever installed. The e2e script runs this procedure as step 4b.

With the deploy tool (DEC-068, DEC-070), the whole procedure is a lane-file edit: in `[env.testnet]`, replace the validator (for example `name = "3"`, `key = "caravel-validator-3"` becomes `name = "4"`, `key = "caravel-validator-4"`), then run `caravel plan` and `caravel apply`. Apply starts the new validator, calls `admin_rotate_signers`, restarts the sequencer with the new epoch and stops the old validator, in that order, and a re-run after an interruption finishes the job. The manual steps below are what it does.

1. Generate the new key (§2.2) and, if it is a new machine, start its validator (§2) so it catches up.
2. Build the new set: raw hex keys (not `G...`), sorted, weight 1, threshold 2.

   ```sh
   raw() { node -e 'const {StrKey}=require("@stellar/stellar-sdk");console.log(Buffer.from(StrKey.decodeEd25519PublicKey(process.argv[1])).toString("hex"))' "$1"; }
   NEW=$(for g in G..1 G..2 G..NEW; do raw "$g"; done | sort | jq -R '{key: ., weight: 1}' | jq -sc '{signers: ., threshold: 2}')
   ```

   (Run it where `@stellar/stellar-sdk` resolves, e.g. `platform/relayer`.)
3. Rotate:

   ```sh
   stellar contract invoke --id <settlement> --source-account caravel-admin --network testnet -- admin_rotate_signers --new "$NEW"
   stellar contract invoke --id <settlement> --source-account caravel-admin --network testnet --send=no -- epoch
   ```

4. Update the sequencer's `[signers]`: `epoch` = the new epoch, and the validator list (key and URL). Restart the sequencer.
5. On start, the sequencer sends every checkpoint that was signed by the old set and not accepted yet back for signatures (log: `checkpoints signed under an older epoch go back for signatures`). The header does not hold the epoch, so the old signatures of validators that stay in the set still count; only the new validator is asked. The relayer then submits them with the new epoch.
6. Check that the next checkpoints are accepted under the new epoch: `caravel wait checkpoint --seq <seq> --epoch <epoch>`, or `caravel api /v1/checkpoints/<seq>`.

Between steps 3 and 5, submissions fail with the old epoch and the relayer retries; nothing is lost.

### 4.2 Rotation signed by the validators

`rotate_signers(new, epoch, sigs)` is the rotation without the admin: the current set signs the new one, at most once per `min_rotation_delay_secs` (1 hour on testnet), and older sets stay valid for `signer_retention_epochs` (2). The contract implements and tests it (spec §13.4, §13.7), but M0 has no tool that collects the validators' rotation signatures. On testnet, use §4.1.

### 4.3 Other keys

| Key | Where | How to rotate |
|---|---|---|
| Relayer (`CARAVEL_RELAYER_SECRET`) | `/opt/caravel/keys/env` | Only pays fees; `submit_checkpoint` needs no particular submitter. Put a funded account's secret in `env`, restart `caravel-relayer`. |
| Internal API token (`CARAVEL_INTERNAL_TOKEN`) | `/opt/caravel/keys/env` | `openssl rand -hex 24`, then restart the sequencer and the relayer together. |
| Oracle (`CARAVEL_ORACLE_SECRET`) | `/opt/caravel/keys/env` | Read by the relayer's perps feed module (`lanes/perps/relayer-feeds`). Its public key is in the lane file's `[perps.oracle] keys`, which is part of the genesis config the contract commits to (`config_hash`). Changing it is a new lane. |
| Settlement admin (`caravel-admin`) | your Stellar CLI keystore | Fixed at deployment in M0. |
| Backstop and treasury | lane file, genesis | Part of the genesis config: a new lane. |

## 5. Freeze drill

The escape hatch (spec §13.6): if no checkpoint is accepted for `escape_timeout_secs`, or a deposit or forced-withdrawal request sent through Stellar is not processed within `force_inclusion_window_secs`, anyone can call `freeze`. After that, nothing more is accepted from the lane, and each account claims its equity from the last accepted checkpoint, pro rata to what the vault holds.

**On the testnet contract a freeze is permanent and ends the demo lane.** The testnet timeouts are 6 hours without a checkpoint or 1 hour for an unprocessed request. Drill it on a throwaway contract instead: step 5 of the end-to-end script stops the sequencer, freezes after a 30 s timeout, has both accounts escape, and step 6 replays the frozen lane.

```sh
./scripts/e2e-local.sh                                   # on a local network
gh run download <main run id> -n contracts-wasm -D /tmp/wasm
E2E_NETWORK=testnet E2E_WASM_DIR=/tmp/wasm ./scripts/e2e-local.sh   # on testnet, its own contract
```

The testnet run deploys its own settlement contract (the Wasm of record, checked against `versions.json`) and buys its USDC on the testnet DEX; it takes about 3.5 minutes.

By hand, on any network:

```sh
caravel stop sequencer relayer          # checkpoints stop; it prints when a freeze becomes possible
stellar contract invoke --id <settlement> --source-account <anyone> --network <net> -- freeze
caravel wait frozen                     # until the contract is frozen
caravel escape <identity>               # the account's withdrawals, then its share of the last checkpoint
```

`caravel escape` needs only the lane file and the admin's public key (`stellar keys add <admin> --public-key G…`). It takes the proof from `exit.json` when it is for Stellar's last checkpoint, else from a validator or the sequencer, and `caravel replay --prove-escape` builds one from Stellar alone (§6). It refuses a claim that would pay nothing, since that uses the escape up (`--allow-zero` claims it anyway).

Without caravel, the contract takes `escape_claim --recipient G... --lane_account <raw hex> --index <index> --equity <equity> --proof '<proof json>'`. Deposits the lane never processed are refunded 1:1 to the depositor with `refund_unprocessed_deposit --index <inbox index>`. The web app's Escape page does all of this with the connected wallet.

## 6. Replay

Rebuilds the lane from Stellar data only and checks every hash (spec §16):

```sh
caravel replay -f lanes/perps/config/lane.caravel-perps.testnet.toml --env testnet [--prove-escape G...] [--prove-withdrawals G...] [--json]
```

It fills in the network, the settlement address (derived from the admin's public key), the genesis document and the engine Wasm from the lane file and the release. Without the lane file, the template's binary takes them all as flags:

```sh
./target/release/caravel-perps-node replay --rpc https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015" \
  --settlement CBIHBEUZYFZQZEQPBJH2ID6CDRDZFEDI6XHAXVOCHG6FO5XWUIGPONWO \
  --genesis-config lanes/perps/config/lane.caravel-perps.testnet.toml \
  --engine-wasm target/contracts/perps_engine.wasm \
  [--prove-escape G...] [--prove-withdrawals G...]
```

The first JSON line is the report: `ok`, the checkpoints replayed and the final state hash, or the first mismatch with its seq and field. `--prove-escape` and `--prove-withdrawals` add a line with that account's proofs, so a user can claim with nothing but Stellar RPC and this CLI.

## 7. Data retention

- **Stellar RPC** keeps transactions for about 7 days on testnet (120,959 ledgers, checked 2026-09-29). Replay needs every checkpoint transaction since genesis, so after 7 days it needs another source: the M1 plan is a Galexie ledger archive (`--from-archive` is not built in M0, DEC-042). Until then, the validators' stores are the lane's history. `caravel-perps-node check-store` re-executes a store and checks each header, and the header hashes on Stellar (`checkpoint(seq)`) anchor it.
- **Node stores** (`/opt/caravel/data/*.sqlite`) keep every block, receipt, checkpoint and snapshot; nothing is pruned in M0. Back them up while the node runs with `sqlite3 <db> ".backup <file>"`. The growth rate is in `docs/RESULTS.md`.
- **Relayer log** `/opt/caravel/data/relayer-checkpoints.jsonl`: one line per submitted checkpoint (fee, size), the input of `scripts/measure-report.mjs`.

## 8. Relayer XLM

The relayer pays for every checkpoint: about 0.3 XLM each on testnet, one a minute when idle, so about 440 XLM a day (`docs/RESULTS.md`). Friendbot funds a new account with 10,000 XLM, which lasts about three weeks. To top up, fund a new account with friendbot and merge it into the relayer:

```sh
stellar keys generate topup --fund --network testnet
stellar tx new account-merge --source-account topup --account <relayer G...> --network testnet
curl -s https://horizon-testnet.stellar.org/accounts/<relayer G...> | jq -r '.balances[] | select(.asset_type=="native") | .balance'
```

If the relayer runs dry, checkpoints stop and the 6-hour escape timeout starts counting.

## 9. Incidents

| Symptom | Where to look | Action |
|---|---|---|
| `checkpoints.accepted` stops growing | `journalctl -u caravel-relayer` | XLM balance (§8), RPC errors. The relayer reconciles with Stellar on restart. |
| `checkpoints.signed` stops growing | sequencer log (`validator refused to sign`), each validator's `/v1/status` | `NOT_CAUGHT_UP` resolves itself; `SUSPICIOUS_BLOCK` needs §2.5; `HALTED` or `HEADER_MISMATCH` means a validator disagrees with the sequencer: stop and investigate before anything else. |
| `halted` on the sequencer | sequencer log | A block failed on the Wasm path. Keep the store; `check-store` shows where. |
| The VM is stopped | GCP console, billing | The billing cap fired (§3.3). |
| Deposits not credited | `/v1/status` `inbox` | `inbox.halted` means the relayer reported a message that does not continue the chain; the relayer log names it. |
