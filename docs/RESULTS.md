# Results

Dated measurements from the M0 testnet demo (spec §19.6). Every number says when and how it was taken. T-014 adds the scripted cost and latency measurements.

## Testnet deployment (T-012, 2026-09-29)

| | |
|---|---|
| Engine contract | [`CD5ILOTZEE5WDPLANZYA4WMWHUGTUOJIXKS4WJCPUJNKEM2QHQLH63B4`](https://stellar.expert/explorer/testnet/contract/CD5ILOTZEE5WDPLANZYA4WMWHUGTUOJIXKS4WJCPUJNKEM2QHQLH63B4) |
| Settlement contract | [`CBIHBEUZYFZQZEQPBJH2ID6CDRDZFEDI6XHAXVOCHG6FO5XWUIGPONWO`](https://stellar.expert/explorer/testnet/contract/CBIHBEUZYFZQZEQPBJH2ID6CDRDZFEDI6XHAXVOCHG6FO5XWUIGPONWO) |
| Engine Wasm | `4571cd252d4b78d523d01fc4a31ab763a1aff77aecafbb9d7302cfb879abbf0a` (84,764 bytes) |
| Settlement Wasm | `8a2fafbd1ad48d53333ab79d73afa1c50d5f790f39a97fafb88b5443b383d503` (42,900 bytes, the x86_64 Linux build from CI run 36637804316, DEC-033) |
| USDC | Circle's testnet USDC asset contract `CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA` (`USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5`, 7 decimals) |
| Validators (2 of 3, weight 1 each) | `GCWQLDEUMRBWE23V6NOSCNWKS6JXJY4FDZ5BTGRH4JMVH5LXQV46PE7T`, `GA6V6WIJVMEV3J5EJW4CVM25N3YXHBAZWW2RCAOFYJGRZX7WPVTPVX6E`, `GAV7B6KXTML5ELBMPFAELTIHBZEPUXALPFMKJVABPBLKDIRXTSISUAG5` |
| Admin (testnet-only powers, spec §4.3) | `GCRUFYE63UACUYULWEWHJOCCGHUK2AULLQOLBNSHC5LKJWTT77UULNUM` |
| Parameters | force-inclusion window 3,600 s, escape timeout 21,600 s, rotation delay 3,600 s, signer retention 2 epochs, minimum deposit 1 USDC |

Deployed with `scripts/deploy-testnet.sh`.

## Witness transaction (T-012)

The engine contract's `step` was invoked once on testnet with a small input. The genesis state of the testnet lane (1,277 bytes) plus one `CHECKPOINT_END` block (163 bytes) holding a deposit, so the call creates an account and computes a commitment. The input comes from `caravel-node witness`.

| | |
|---|---|
| Transaction | [`d0aa560819871acfb81e02933250d1aa3c160c56b4b5288a6fce5659ffa700b0`](https://stellar.expert/explorer/testnet/tx/d0aa560819871acfb81e02933250d1aa3c160c56b4b5288a6fce5659ffa700b0) |
| Ledger | 4,939,321, 2026-09-29 22:16:32 UTC |
| Network protocol | 29 (the executor pins `soroban-env-host` 28.0.2) |
| Return value | 1,552 bytes, sha256 `3a0ecdaf24d739b6a661ced7d445b06ea0d4b2febe37f4f05953c855daff3628` |
| Byte-equal to the executor's output | **yes**. The transaction's `returnValue` was read back with `getTransaction` and compared with `WasmExecutor::step` on the same input |
| Fee charged | 17,363 stroops (0.0017 XLM) |
| Host metering in the executor | 9,528,446 CPU instructions, 3,488,005 memory bytes (not network fees) |

This shows the exact call the nodes make is a valid Stellar invocation with the same output. It does not mean Stellar validators execute lane trades (spec §2.2).

## The lane on testnet (T-012, 2026-09-29)

The sequencer, 3 validators and relayer run on one e2-small VM (DEC-046). Public API: `https://35-224-76-64.sslip.io/v1/status`, and each validator at `/validators/N/v1/status`. Oracle prices come from Reflector's testnet feed (DEC-041). Since 2026-09-30 the same host also serves the web app at [`https://35-224-76-64.sslip.io`](https://35-224-76-64.sslip.io) (release `6c1224d`, from CI run 36651914634).

| First checkpoint on testnet | |
|---|---|
| Transaction | [`59821f87f5dfdf051138dd0ed4671265cce8a7a78ef3981ee024e56d75f87802`](https://stellar.expert/explorer/testnet/tx/59821f87f5dfdf051138dd0ed4671265cce8a7a78ef3981ee024e56d75f87802) |
| Ledger | 4,939,489, 2026-09-29 22:30:32 UTC |
| Covers | seq 1, lane blocks 1 to 60 (`checkpoint_every_blocks = 60`, DEC-044) |
| Batch | 9,934 bytes; header 442 bytes; 3 signatures; transaction 11,496 bytes |
| Fee charged | 3,048,833 stroops (0.305 XLM); `minResourceFee` from simulation 3,510,124 |

At one checkpoint a minute that is about 440 XLM a day of testnet XLM for the relayer account. T-014 breaks the fee down and measures it under load.

## Measurements (T-014, 2026-09-29)

The spec §19.6 numbers, measured on the live testnet lane with `scripts/measure-testnet.sh` and `scripts/measure-report.mjs` (DEC-048). Reproduce with:

```sh
TPS=20 DURATION=600 ./scripts/measure-testnet.sh
TPS=45 DURATION=300 ./scripts/measure-testnet.sh
```

**Conditions:**
- **Lane:** the testnet deployment above, on one e2-small VM (2 shared vCPUs, 2 GB) that runs the sequencer, the 3 validators and the relayer. 1 s blocks; `checkpoint_every_blocks = 60`, `max_batch_bytes = 96,000`, `max_block_bytes = 12,000`. Stellar testnet on protocol 29.
- **Load:** 12 throwaway accounts, each with 500 USDC deposited through the settlement contract (Circle's testnet USDC, bought with friendbot XLM on the testnet DEX). One sender per account, one request at a time. The mix over BTC, ETH and XLM:
  - 60% resting limit orders 1 to 20 ticks from the oracle price;
  - 25% IOC orders crossing 1 to 5 ticks;
  - 12% cancel-all;
  - 3% withdrawals of 1 USDC.

  Prices come from Reflector through the relayer, as always.
- **Client:** a laptop on a home connection, through the public HTTPS API that the web app uses. Round trip to the VM about 115 ms; a request after the TLS handshake takes 117 to 120 ms.
- **Idle column:** the last 10 checkpoints (between seq 26 and 39) before the first run that hold no user transaction, only oracle updates, and 90 s of `/v1/status` samples after the runs.
- **Load columns:** only checkpoints whose blocks were all produced under load. Hard latency covers only receipts in those checkpoints; after the load stops, the last receipts wait for an idle 60-block checkpoint.

| Measure | Idle lane | 20 tx/s, 10 min | 45 tx/s, 5 min |
|---|---|---|---|
| Blocks/s | 1.00 | 1.00 | 1.00 |
| User tx/s (in checkpointed blocks) | 0 | 20.0 (p50 19, max 24 per block) | 44.7 (p50 45, max 50 per block) |
| Transactions executed OK / rejected by the engine | – | 11,953 / 49 | 13,287 / 141 |
| Host `cpu_insns` per block, p50 / p99 | 11.4M / 12.9M | 23.4M / 27.8M | 39.3M / 41.8M |
| Soft latency (`POST /v1/tx` → receipt on the WebSocket), p50 / p99 | – | 652 ms / 1,129 ms | 648 ms / 1,131 ms |
| Hard latency (receipt → its checkpoint accepted on Stellar), p50 / p99 | – | 17.0 s / 27.8 s | 12.4 s / 19.1 s |
| Blocks per checkpoint, p50 | 60 | 20 | 9 |
| Batch, p50 | 9,934 B | 86,964 B | 87,126 B |
| Checkpoint transaction, p50 (max) | 11,496 B | 88,536 B (90,064 B) | 88,772 B (93,920 B) |
| `minResourceFee` from simulation, p50 | 0.3524 XLM | 0.3908 XLM | 0.3909 XLM |
| Fee charged per checkpoint, p50 | 0.3061 XLM | 0.3410 XLM | 0.3411 XLM |
| Fee charged per 1,000 lane transactions | – | 0.869 XLM (11,768 tx, 30 checkpoints) | 0.842 XLM (12,969 tx, 32 checkpoints) |
| Example checkpoint transaction | [`41de59d9…`](https://stellar.expert/explorer/testnet/tx/41de59d9ca213fe18b61319ecd74a0e6baaec7018f0f00ed4f31629c7a2da6e6) | [`ae928e35…`](https://stellar.expert/explorer/testnet/tx/ae928e3573ac7b11dda3eb86396941bbbe326681b6eba7eedafae9abe338efab) | [`ed93039c…`](https://stellar.expert/explorer/testnet/tx/ed93039c90d680a9d590b377836a68d03a9d253112833a109212e2a672d3c4fb) |

The rejections are the load mix hitting `TOO_MANY_OPEN_ORDERS` (32 per account), not errors. No request failed, and every checkpoint in these windows was accepted on Stellar. The WebSocket did not deliver 1 receipt (of 12,003) and 2 receipts (of 13,430), so those have no soft latency. The transactions themselves were not lost: nonces run in order, so a lost one would have held back every later transaction of its account, and nothing else went missing.

**What the numbers say:**
- **Throughput:** 45 tx/s is close to the lane's cap. `max_block_bytes = 12,000` fits about 56 orders in a block, and the run peaked at 50 per block. Blocks stayed at 1 per second, and host CPU stayed under half of the 100M target (§12.2).
- **Soft latency:** about 650 ms whatever the load. A transaction waits for the next 1 s block (about 500 ms on average), plus one round trip.
- **Hard latency:** it falls as load rises. Batches fill sooner (rule (b) of §14.2 ends one when the next full block might not fit), so checkpoints come every 20 blocks at 20 tx/s and every 9 at 45 tx/s, instead of every 60. Each is accepted about 5 to 10 s after it is sealed. The relayer kept up at both rates: the sequenced, signed and accepted counts stayed within one checkpoint of each other.
- **Fees:**
  - The fee barely depends on the batch: 0.306 XLM for 10 KB and 0.341 XLM for 87 KB, about 0.0005 XLM per extra KB. So the cost per lane transaction falls as load rises.
  - The fee charged is below the simulated `minResourceFee` because Stellar refunds unused refundable resource fees.
- **Relayer XLM per day:**
  - idle: about 1,440 checkpoints, 441 XLM;
  - sustained 20 tx/s: 4,320 checkpoints, about 1,470 XLM;
  - sustained 45 tx/s: 9,600 checkpoints, about 3,270 XLM.

  One friendbot account (10,000 XLM) lasts about 22, 6.8 and 3 days respectively (top-up: `docs/RUNBOOK.md` §8).
- **Storage:** under load the sequencer's store grew by about 15 KB per block (19.2 MB at height 3,670, 32.7 MB at 5,056). Each validator's store is the same size. At height 5,056 the four stores took 131 MB of the VM's 19 GB disk.

## Freeze drill on testnet (T-016, 2026-09-29)

The spec §24 freeze drill, run with `E2E_NETWORK=testnet SETTLEMENT_WASM=<CI settlement.wasm> ./scripts/e2e-local.sh`. It ran on a throwaway settlement contract of its own, so the demo lane's contract was not frozen. The contract ran the settlement Wasm of record (`8a2fafbd…`) with a 30 s escape timeout, and the lane was the local lane file (fixture oracle key, fixed prices) with its sequencer, 3 validators and relayer on a laptop. It passed in 202 s.

| Step | Transaction (testnet) |
|---|---|
| Drill contract | [`CDAHFCPAJIL3BXB2OS5SYSZUERU2ZW5PQ7QL3MGBG5KTZ2NLF2VDUOGC`](https://stellar.expert/explorer/testnet/contract/CDAHFCPAJIL3BXB2OS5SYSZUERU2ZW5PQ7QL3MGBG5KTZ2NLF2VDUOGC) |
| 1. Deposits of 1,000 USDC (Circle's testnet USDC) for A and B | [`e3e418b1…`](https://stellar.expert/explorer/testnet/tx/e3e418b19673442ff606d5e14bd2ab1909b85a6fcd86ebba81ce3dad8cf5ba40), [`33acade7…`](https://stellar.expert/explorer/testnet/tx/33acade708c78dfa6eb6441061c2f5b34e34c93742e7702fdc34770bc526f409) |
| 3. First checkpoint accepted | [`21768485…`](https://stellar.expert/explorer/testnet/tx/2176848566e61ab62e430332e31bb778d226bfc8be101cc6b0601e6f580a600d) |
| 4. A claims a 100 USDC withdrawal | [`351df6ad…`](https://stellar.expert/explorer/testnet/tx/351df6ad876541c67db162fdb5d595dfa139416d1405c47b7982bc9feb039022) |
| 4b. Validator 3 rotated to a new key (`admin_rotate_signers`) | [`a909bf29…`](https://stellar.expert/explorer/testnet/tx/a909bf29e3e8a02cbb585eaefa7401efe0c7d6a0fd883875fda58311d06de9d3) |
| 4b. Checkpoint 5 (signed under epoch 1, re-signed under epoch 2) and 6 accepted | [`d1873c88…`](https://stellar.expert/explorer/testnet/tx/d1873c8883d6d9fb8e1fc898d4c317fcc2dec586f0d9d5a8bd84ef6af9955083), [`796a3e2f…`](https://stellar.expert/explorer/testnet/tx/796a3e2fcb145fb8dad033002efb730f0a6728132c202b1c51a960881354e408) |
| 5. The sequencer stops; `freeze` after the escape timeout | [`db37a01f…`](https://stellar.expert/explorer/testnet/tx/db37a01f98b740dc532e0d648052edacdd8ade9fbb5052384efbd2ccfba566b0) |
| 5. A and B escape: 900 and 999.987 USDC, payout ratio 1,900 / 1,900 | [`17760671…`](https://stellar.expert/explorer/testnet/tx/177606712ac6662bca8cb68c810f2ee23521f11cc20b65861526da39bf0f690b), [`939bc60c…`](https://stellar.expert/explorer/testnet/tx/939bc60c32fa196740813413599b1738f863fd6e55f8c07e7c07e0153fd42228) |
| 6. `caravel-node replay` from testnet data only, after the freeze | `OK seq=1..6 final_state_hash=b1e696c6…aa87` |

Each escape paid the account's equity in the last accepted checkpoint times the payout ratio, and the payouts did not exceed the vault. The escape proofs came from validator 1, and replay's proof for A matched it.

## Lane #1 upgraded to the platform node (M0.5 P-07, 2026-09-30)

The testnet VM moved from the M0 `caravel-node` (release `6c1224d`) to `caravel-perps-node` (CI release `3f3248f`, P-06). The lane file changed to the platform layout (DEC-054), and the oracle now runs as the relayer's perps feed module (DEC-053). The engine Wasm (`4571cd25…`), the settlement contract and the stores did not change. Before the swap, `scripts/vm-preflight.sh` checked the release against the live lane without changing it:

| Check | Result |
|---|---|
| `check-store`: the release binary and lane file re-execute a copy of the live sequencer database through the engine Wasm | `ok`: 39,856 blocks, all 759 checkpoint headers rebuilt byte for byte |
| The shadow validator's startup check against the settlement contract's on-chain config (lane_id, engine hash, config_hash, genesis_state_hash) | passed |
| Shadow validator from block 1 (stopped at height 30,140, as the human chose) | 597 checkpoint headers compared with the sequencer's, 0 differing, no halt, no suspicious block |

The swap (`deploy-vm.sh`) was at height 45,256 with checkpoint 849 accepted. It took a few seconds of downtime. After the swap:
- the sequencer and all 3 validators report `template: perps`, with the Wasm executor and no halt;
- oracle prices were 3 s old, from the relayer's perps feed module;
- the web app is served;
- checkpoint 852 (blocks 45,324–45,383) was sealed and signed entirely by the new nodes and accepted on Stellar ([`3de4f6b5…`](https://stellar.expert/explorer/testnet/tx/3de4f6b50a52980f1d9f0e94269162254858308aee97a26cafdb29a03473575f), ledger 4,948,567, `getTransaction` status SUCCESS).
- `caravel-perps-node replay` from testnet data only, after the swap, with the platform-layout lane file: `OK seq=1..852 final_state_hash=744af89e…4b93`. That covers every M0 checkpoint and the first ones the new nodes produced. The first attempt got an RPC-side error (`getLedgerEntries: could not query captive core … 404`); the rerun, with RPC reporting healthy, passed.

## Coinbase oracle feed and Stellar Wallets Kit on lane #1 (M0.5 P-07a, P-07b, 2026-09-30)

CI release `c6f9b48` was deployed at height 49,926, with checkpoint 927 accepted. Its node binary is byte-identical to the one the P-07 preflight checked (`639f3cce…`), so the deploy changed only the relayer's perps feed module, its config and the web app.

- **Oracle (DEC-058):** the feed reads Coinbase's public ticker and heartbeat stream and runs every block.
  - Sampled from `/v1/markets` every 2 s for 20 s after the deploy, BTC-PERP's `oracle_time_ms` advanced at +7, +2, +5, +2 and +3 s. It publishes on each one-tick ($1) move, at most once per block. ETH-PERP moved at a similar rate, and XLM-PERP every 6 to 10 s, on its sparser trades.
  - Before, the first source was Reflector, whose price changes every 300 s, re-signed on a 10 s heartbeat.
  - Lane prices tracked Coinbase spot: BTC 83,812–83,829 USD on the lane, against 83,814.70 spot.
  - Checkpoints 928 and 929 were accepted after the deploy. The relayer logged no warnings.
- **Wallets (DEC-059):**
  - The web app now connects through Stellar Wallets Kit. The new bundle is served, and headless Chrome opened the kit's picker against the live lane.
  - Manual flows with real wallets are pending: connect, deposit, enable trading (SEP-53), trade, withdraw and claim, with Freighter and xBull.

## Node groundwork on lane #1 (M0.5 P-11, 2026-09-30)

CI release `2159e7a` (DEC-065) was installed with `deploy-vm.sh` at height about 77,800. The release adds status fields, `export-proofs` and vendored relayer dependencies, and changes nothing in consensus. The human judged the full preflight unnecessary for it, so `check-store` and the shadow validator were stopped before they finished.

- The sequencer and validator 1 report `release.commit` `2159e7ad…`, with `config_hash` `f4b9db09…`, engine `4571cd25…` and settlement `CBIHBEUZ…` as before.
- Checkpoints 1393 and 1394 were accepted within a minute of the restart, with signed equal to accepted, and no node halted.
- `COMMIT` and `SHA256SUMS` are now in `/opt/caravel`, and the relayer ran from its vendored `node_modules`, with no `npm` on the VM.

## Lane #1 under the deploy tool (M0.5 P-16, 2026-09-30)

Lane #1's deployment is now the `[env.testnet]` table of its lane file (DEC-070).

- **Before applying:** `caravel plan lanes/perps/config/lane.caravel-perps.testnet.toml --env testnet --release-dir <CI release 2159e7a> --diff` read Stellar and the VM:
  - no Stellar step and no problem;
  - the same release and systemd units;
  - 7 config files to normalize (the lane file renamed to `lane.toml`, comments, an explicit `weight = 1`, JSON key order), with a restart of the 5 nodes.
- **`caravel apply --yes`**, with the human's go-ahead, took 3 min 59 s over IAP. It ended with "Applied. The lane matches the lane file".
- **Afterwards:**
  - `plan` reports "No changes";
  - checkpoints 1420 and 1421 were accepted, with signed equal to accepted;
  - BTC, ETH and XLM oracle prices were 2 to 9 s old;
  - the web app answers 200;
  - `status` shows every node up and the relayer at 9,535.76 XLM.

## A payments lane on testnet, from its lane file (M0.5 P-17, 2026-09-30)

`E2E_NETWORK=testnet E2E_TEMPLATE=payments E2E_WASM_DIR=<CI contracts-wasm> ./scripts/e2e-local.sh` passed in 215 s. It used the `local` provider against Stellar testnet with the Wasm of record (DEC-071).

| Step | Result |
|---|---|
| `caravel apply` | Settlement `CDVVLXV6T5LBIQUS2KHYD7C6UMUBMIOLG4O27G6UMMQXYGJTTPXO47PX` at its derived address, build `fb68ee32…`; a second plan shows no changes |
| Users | 1,000 USDC deposits each (Circle's testnet USDC from the DEX); a 100 USDC transfer with the 0.01 USDC fee to the treasury; a 100 USDC withdrawal claimed |
| Rotation from the lane file | Epoch 2 with validators 1, 2 and 4; checkpoints 6 and 7 accepted under epoch 2, 6 after being signed again by the new set |
| Forced withdrawal | 50 USDC asked for on Stellar, processed by the lane, claimed |
| `caravel destroy` | Drained, stopped, exported `exit.json`, triggered, frozen 10 s later (20 s window) |
| Escape | Alice 799.99 USDC and Bob 1,050 USDC at payout 1:1 (vault 1,850 USDC; the treasury's 0.01 USDC stays unclaimed) |
| Replay | OK over 11 checkpoints from Stellar data alone, with the same escape proof as `exit.json` |

## The lifecycle on testnet with the CLI alone (M0.6 C-14, 2026-10-03)

`E2E_NETWORK=testnet E2E_TEMPLATE=payments ./scripts/e2e-local.sh` passed in 324 s. Outside its cleanup, the script now calls only `caravel` and `jq` (`scripts/check-e2e.sh`, DEC-086). The lane came from `caravel init payments --prefix e2e`, and the `local` provider ran its nodes on a laptop against Stellar testnet. No `E2E_WASM_DIR` was given, so the settlement contract runs this machine's build, pinned by the `settlement_wasm` var (`d2c67d28…`), not the Wasm of record.

| Step | Command | Result |
|---|---|---|
| Deploy | `caravel apply` | Settlement `CBUW43OVL7NFBCZUBMXMYHIF6RBD4LSSMOUQWBGHBBHX67AK5LMW33BP` at its derived address; `plan --exit-code` then exits 0 |
| Users | `caravel account create`, `deposit`, `tx` | Circle's testnet USDC bought on the DEX; 1,000 USDC deposits each; a 100 USDC transfer with the 0.01 USDC fee to the treasury |
| Withdrawal | `caravel withdraw` | 100 USDC, claimed on Stellar once its checkpoint was accepted |
| Rotation | `caravel stop relayer`, `caravel apply --var 'validators=["1","2","4"]'` | Epoch 2; checkpoints 10 and 11 accepted under it, 10 after being signed again by the new set |
| Forced withdrawal | `caravel force-withdraw` | 50 USDC asked for on Stellar, processed by the lane, its exact leaf claimed |
| Destroy | `caravel destroy` | Drained, stopped, exported `exit.json`, triggered, frozen 4 s later |
| Escape | `caravel escape` | Alice 799.99 USDC and Bob 1,050 USDC, each equal to its expected payout |
| Replay | `caravel replay --prove-escape` | OK over 15 checkpoints from Stellar data alone, with the same escape proof as `exit.json` |

The same script passed on a local network for both templates (perps in 251 s, payments in 182 s), and `scripts/check-quickstart.sh` ran the README's quickstart as written, from `install.sh` to `caravel escape`, on macOS. Lane #1's read-only `caravel plan --exit-code` with its CI release still printed "No changes." the same day.

## Performance baseline (M0.9 F-03, 2026-10-04)

What the lane costs before the M0.9 fixes (spec §20.8), as the reference for F-14. Measured with `scripts/soak-lane.sh` (F-02):

```sh
BLOCK_MS=500 TPS=45 ACCOUNTS=32 INFLIGHT=2 DURATION=600 ./scripts/soak-lane.sh
```

**Conditions:**
- **Lane:** the perps template from `caravel init`, deployed with `caravel apply` on a local Stellar network (quickstart, 1 s ledgers): the sequencer, 3 validators (threshold 2) and the relayer with fixed-price feeds, all as processes on one laptop (Apple M-series, 16 cores). One checkpoint a minute (`checkpoint_every_blocks = 60000 / block_time_ms`), unless a batch fills first.
- **Load:** 32 accounts that deposited 1,000 USDC each through the settlement contract, 2 requests in flight per account, the load generator's usual mix (60% resting orders, 25% IOC, 12% cancel-all, 3% withdrawals of 1 USDC). 10 minutes a run.
- **Storage:** the bytes each SQLite store's live pages hold (WAL included, free pages left out), growth over the run, per block and per day. All four stores grow alike before F-08.
- **Timings:** each node's `/v1/status` `perf` (F-01), p50 / p99 over the last 1,024 samples.

| Measure | 500 ms, idle | 500 ms, 45 tx/s | 500 ms, 100 tx/s | 200 ms, idle | 200 ms, 45 tx/s | 200 ms, 100 tx/s |
|---|---|---|---|---|---|---|
| Blocks/s | 2.0 | 2.0 | 2.0 | 5.0 | 5.0 | 5.0 |
| User tx/s, all OK or rejected by the engine | 0 | 45.0 | 100.0 | 0 | 45.0 | 100.0 |
| Soft latency, p50 / p99 | – | 268 / 514 ms | 274 / 519 ms | – | 112 / 212 ms | 117 / 213 ms |
| Hard latency, p50 / p99 | – | 8.7 / 13.7 s | 5.6 / 8.9 s | – | 8.5 / 13.3 s | 6.0 / 8.8 s |
| Checkpoints accepted in the run | 10 | 72 | 156 | 10 | 78 | 156 |
| Batch, p50 | 17,674 B | 88,100 B | 85,785 B | 40,894 B | 85,105 B | 87,235 B |
| Store growth per block (each of 4) | 290 B | 7,302 B | 13,143 B | 238 B | 2,944 B | 5,500 B |
| Store growth per day (each of 4) | 50 MB | 1.26 GB | 2.27 GB | 103 MB | 1.27 GB | 2.37 GB |
| Host `cpu_insns` per block, p50 | – | 29.9M | 47.9M | – | 20.1M | 27.7M |
| Sequencer: execute, p50 / p99 | 8.6 / 13.5 ms | 14.5 / 20.7 ms | 20.0 / 26.7 ms | 8.1 / 12.2 ms | 9.5 / 12.3 ms | 10.7 / 14.2 ms |
| Sequencer: commit, p50 / p99 | 0.5 / 3.0 ms | 0.5 / 4.5 ms | 0.5 / 6.8 ms | 0.5 / 2.8 ms | 0.5 / 1.0 ms | 0.4 / 1.1 ms |
| Seal to signed, p50 | 2.00 s | 2.01 s | 2.01 s | 2.01 s | 2.01 s | 2.01 s |
| Validator: fetch, p50 / p99 | 0.3 / 8.3 ms | 1.4 / 17.0 ms | 2.7 / 24.6 ms | 0.4 / 7.7 ms | 0.6 / 9.3 ms | 1.0 / 10.2 ms |
| CPU, average (sequencer / a validator / relayer) | 1.4 / 1.2 / 0.2% | 4.0 / 2.1 / 0.6% | 7.1 / 2.9 / 1.5% | 3.7 / 3.6 / 0.3% | 6.7 / 4.6 / 0.5% | 9.1 / 5.0 / 0.8% |
| Memory, max (sequencer / a validator / relayer) | 23 / 23 / 218 MB | 37 / 27 / 126 MB | 41 / 27 / 155 MB | 24 / 24 / 222 MB | 39 / 28 / 138 MB | 42 / 27 / 155 MB |

The engine's rejections (0.5 to 0.8% of transactions) are `TOO_MANY_OPEN_ORDERS` (code 15) from the load mix. CPU is the percentage of one core.

**Lane #1 on testnet, the same day:**
- 432,883 blocks at 0.5 s;
- ~280 B a block in each of the four stores (sequencer and 3 validators on one 20 GB disk), ~121 MB a store;
- the sequencer's file at 261 MB, 120 MB of it free pages, since nothing ever gave space back;
- ~190 MB a day across the four stores.

Its traffic is mostly oracle updates, which is why it matches the idle column.

**What the numbers say:**
- **Storage is the first limit:**
  - Idle, 200 ms blocks double the growth (238 B × 5 a second against 290 B × 2): about 410 MB a day across lane #1's four stores, a 20 GB disk in about 45 days.
  - Under load, each transaction costs about 325 B in every store (its bytes in the block and its receipt), whatever the block time: 1.26 GB a day per store at 45 tx/s, 5 GB across four. F-07 and F-08 go after this.
- **Every signature waits 2 s:** `seal to signed` is 2.0 s in every run. The sequencer asks for signatures as it seals, a validator that has not fetched the last block yet refuses, and the signer sleeps 2 s before asking again. That is a fixed 2 s in every hard latency (F-10).
- **Block time is not a throughput limit:** 100 tx/s ran at both block times with nothing refused by the sequencer, and the sequencer used under 10% of one core. A 200 ms block executes in 11 ms (p50), not 20 ms, because it is smaller.
- **Checkpoints are:** under load a batch fills to its 96,000-byte cap long before a minute's blocks, so checkpoints come every 4 to 9 s. On this local network the relayer kept up (1 s ledgers). On testnet, ledgers close about every 5 s, and the relayer submits one checkpoint after the other: about 87 KB of blocks per ledger. At ~325 B a transaction that is roughly 85 to 90 tx/s sustained, at any block time. The limits allow two such transactions per ledger (266,240 B of transactions a ledger, 132,096 B each, `docs/SOURCES.md`), which the relayer cannot use yet (F-14).
- **An idle minute at 200 ms is a 41 KB batch** (17.7 KB at 500 ms): 300 blocks of headers and oracle updates. Still one Stellar transaction a minute.
- **Soft latency is half a block plus the round trip:** about 270 ms at 500 ms blocks and 115 ms at 200 ms.

## After the M0.9 fixes (F-14 gate, 2026-10-04)

The same soak on the M0.9 stack (F-01 to F-14), same machine and settings as the baseline above. The 45 tx/s run used the relayer from before its retry fix (F-14), and its hard-latency p99 includes one checkpoint that waited out a dropped connection plus a 4 s backoff (36 receipts); the other runs have the fix and no relayer error under load.

| Measure | 500 ms, idle | 500 ms, 45 tx/s | 500 ms, 100 tx/s | 200 ms, idle | 200 ms, 100 tx/s |
|---|---|---|---|---|---|
| Hard latency, p50 / p99 (baseline) | – | 5.6 / 18.3 s (8.7 / 13.7) | 3.1 / 5.2 s (5.6 / 8.9) | – | 3.7 / 6.0 s (6.0 / 8.8) |
| Seal to signed, p50 (baseline 2.0 s) | 9 ms | 12 ms | 15 ms | 12 ms | 15 ms |
| Soft latency, p50 / p99 | – | 267 / 512 ms | 275 / 521 ms | – | 117 / 217 ms |
| Sequencer store growth per block (baseline) | 248 B (290) | 3,143 B (7,302) | 5,494 B (13,143) | 191 B (238) | 2,250 B (5,500) |
| Sequencer store per day (baseline) | 43 MB (50) | 543 MB (1,261) | 949 MB (2,271) | 83 MB (103) | 973 MB (2,373) |
| A validator's store at the end (baseline) | 0.27 MB (0.38) | 1.27 MB (9.2) | 1.22 MB (17.6) | 0.49 MB (0.77) | 1.60 MB (18.9) |
| Validator commit, p50 / p99 (baseline) | 60 / 407 µs (308 / 3,232) | 79 / 478 µs (248 / 2,956) | 104 / 415 µs (260 / 7,890) | 89 / 638 µs (358 / 2,839) | 81 / 309 µs (208 / 1,005) |
| CPU average, sequencer / a validator / relayer | 1.6 / 1.2 / 0.3% | 3.4 / 1.8 / 0.6% | 6.0 / 2.3 / 1.2% | 4.7 / 3.9 / 0.3% | 9.6 / 5.3 / 1.8% |
| User tx OK / rejected by the engine | – | 26,896 / 139 | 59,586 / 468 | – | 59,567 / 467 |

**What changed:**
- **Hard latency fell by about 2.5 s at every load:** the 2 s signer retry is gone (seal to signed is now 9 to 15 ms), and the relayer picks up a signed checkpoint at once. The rest is the ledger: on this local network a checkpoint lands within a second or two of being signed.
- **Validators stop growing:** they keep the blocks after their oldest kept snapshot, about four checkpoints' worth, so a validator's store stays near 1 to 2 MB under load instead of growing 2.3 GB a day. Of the four stores on lane #1's disk, three now stay flat.
- **The sequencer's store grows 2.4× slower:** the blocks of old checkpoints are deflated into one archive row each. Under load that is about 135 B a transaction instead of 325 B.
- **Validators write less per block:** commit p50 is 60 to 100 µs instead of 250 to 360 µs (a lazy head, `synchronous=NORMAL`).
- **Unchanged:** soft latency (half a block plus the round trip), the engine's execute time (consensus is untouched), and the throughput limits of the baseline. A checkpoint is still one Stellar transaction at a time, so on testnet sustained load stays near 85 to 90 tx/s.
- **For lane #1 at 200 ms:** idle, the four stores would grow about 83 MB a day together instead of about 410 MB (and less than today's 190 MB at 500 ms). CPU is the open question: a 200 ms block executes in about 10 ms on this laptop, and lane #1 runs four nodes on an e2-small, whose two shared vCPUs sustain about half a vCPU. That needs measuring on the VM (F-14 gate).

## Lane #1 on 0.3.0 (2026-10-05)

`caravel apply` with the v0.3.0 release, at 05:57 to 06:00 UTC: a new release, the containers' log caps, and all five nodes restarted. Nothing changed on Stellar (engine `4571cd25`, `config_hash` `f4b9db09`). Checkpoint 7,722 was sealed, signed and accepted on the new release within 90 s.

On the VM (e2-small, 2 shared vCPUs, pd-standard), from the sequencer's `/v1/status` `perf` and `docker stats`, a few minutes after the restart:

| Measure | Lane #1 | Local soak, 500 ms idle |
|---|---|---|
| Execute a block, p50 / p99 | 14.5 / 25.1 ms | 8.5 / 10.2 ms |
| Commit a block, p50 / p99 | 2.2 / 3.5 ms | 0.5 / 2.5 ms |
| Seal to signed, p50 | 144 ms (2.0 s before) | 9 ms |
| CPU, all five containers | about 17% of one vCPU | – |
| Memory, all containers | about 176 MB | – |

The sequencer's `lock_wait` peaked at 190 ms while it archived the old checkpoints' blocks, 8 at a time. That is harmless at 500 ms blocks, but it needs to move outside the lock before any faster block time.

**200 ms stays off for lane #1.** All four nodes execute every block, so 200 ms would mean about 43% of one vCPU while idle. An e2-small sustains about half of one, which leaves no room for load.

## Checkpoints when needed (M0.10, 2026-10-05)

**Settings:** lane #1's new checkpoint rules (`checkpoint_urgent_ms = 5000`, `checkpoint_busy_ms = 60000`, `checkpoint_idle_ms = 3600000`) and the settlement contract v2, which keeps a record only for checkpoints with withdrawals (DEC-123, DEC-124).

**Fees, measured on testnet** (the e2e on v2, 12 checkpoints; the relayer records each fee's parts from the transaction's metadata):

| Checkpoint | Fee | Of which rent |
|---|---|---|
| Without withdrawals, 1 to 3 KB batch (10 of 12) | 0.0017 to 0.0027 XLM | 0 |
| With withdrawals (2 of 12) | 0.335 XLM | 0.332 XLM, the record's 120 days |
| Lane #1 today (contract v1, any checkpoint), for comparison | 0.354 XLM (p50) | ~0.333 XLM |

Batch size still adds about 0.0005 XLM per KB. A full 84 KB idle batch on v2 should therefore cost about 0.045 XLM.

**Cadence, measured on the local soak** (500 ms blocks, 15 minutes per run):
- **Idle:** 3 checkpoints, all ended by a full batch (84 KB p50). That is about one every 5 minutes, **288 a day instead of 1,440**. Empty blocks and oracle updates fill a batch long before the hourly heartbeat.
- **20 tx/s with the load mix's 3% withdrawals:** every batch ended `urgent`, one every 5 s (16,652 a day). Hard latency p50 / p99 was 4.0 / 7.3 s, and soft latency was 260 / 497 ms. Under steady load a withdrawal is always waiting, so the urgent rule fires all the time.

**What it costs, per day, on testnet prices:**

| Lane | Contract v1 (lane #1) | Contract v2 |
|---|---|---|
| Today's rule, one a minute, busy or idle | ~494 XLM | ~3.5 to 65 XLM |
| Idle, the new rules | ~102 XLM (288 × 0.354) | ~13 XLM (288 × ~0.045) |
| A few withdrawals a day (100 urgent checkpoints) | about +35 XLM | about +34 XLM |
| A withdrawal always waiting, `urgent_ms = 5000` | ~6,100 XLM (17,280 × 0.354) | ~5,800 XLM (17,280 × 0.335) |
| The same with `urgent_ms = 30000` | ~1,000 XLM | ~970 XLM |

So the new rules cut idle cost 80% on lane #1's contract and 97% on v2, and deposits settle within about 5 s at almost no cost on v2. Withdrawals are different. Each checkpoint that carries them pays the record's rent on either contract. Making them claimable in seconds under steady withdrawal traffic costs one record per `urgent_ms`, and `urgent_ms` is that trade-off's knob.
