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
