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
