# Benchmarks

Host metering of the engine's `step` call, measured through `soroban-env-host 28.0.2` with the engine Wasm of record (spec §12.2, T-005).
- **Host CPU insns** and **host mem bytes** are host metering. They are not network fees (spec §14.5).
- **Wall ms** is the time one `step` takes on the machine below, with the host built in release mode.

**Conditions:**
- Date: 2026-09-29.
- Machine: Apple M5 Pro, macOS 26.7.
- Engine Wasm: `4571cd25…bf0a`, 84,764 bytes, `opt-level = 2`.
- Each call runs in a fresh host with the module cache off, so Wasm parsing and instantiation are included (about 7M CPU).

Reproduce with:

```sh
./scripts/build-contracts.sh
BENCH_ACCOUNTS=256 BENCH_ORDERS_PER_SIDE=128 BENCH_BLOCK_BYTES=12000 \
  cargo run --release -p caravel-runtime --example bench_full_caps
```

## Full caps of the testnet lane (DEC-028)

Caps: `max_accounts 256`, `max_orders_per_side 128` (3 markets × 2 sides), `max_block_bytes 12,000`, `max_entries_per_block 256`.

The state at full caps:
- 254 users with positions on every market;
- a session key each;
- full books;
- 254 pending withdrawals.

Every block also runs natively and must match byte for byte.

| Block at full caps | Entries | Block bytes | State bytes | Fills | Liquidations | Host CPU insns | Host mem bytes | Wall ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| empty block | 0 | 93 | 85,567 | 0 | 0 | 43,422,220 | 4,114,509 | 14 |
| 3 oracle updates + funding on 3 markets | 3 | 450 | 85,567 | 0 | 0 | 44,915,159 | 4,117,416 | 10 |
| deposits up to the entry and byte caps | 170 | 11,993 | 85,567 | 0 | 0 | 58,315,446 | 4,529,973 | 9 |
| GTC orders that rest-reject (book full), max bytes | 56 | 11,845 | 85,567 | 0 | 0 | 72,789,792 | 4,245,243 | 9 |
| IOC takers sweeping 3 levels each, max bytes | 56 | 11,845 | 85,567 | 159 | 0 | **95,770,584** | 4,376,627 | 12 |
| CHECKPOINT_END: every account leaf + a near-full withdrawal queue | 3 | 450 | 85,567 | 0 | 0 | 62,036,207 | 4,667,332 | 8 |
| −9% shock: half the accounts (levered longs) liquidated | 3 | 450 | 85,567 | 0 | 127 | 67,960,207 | 4,310,570 | 9 |

- **CPU:** the worst block is 95.8M host CPU instructions (95,770,584). That is under the §12.2 target of 100M, and 48% of `exec_cpu_limit = 200,000,000`.
- **Memory:** at most 4.7 MB against `exec_mem_limit` 40 MiB.

## How we got here

### 1. The spec caps did not fit

The spec's §10.3 caps are 1,024 accounts, 256 orders per side and 24,000-byte blocks. With `opt-level = "z"`:

| Block at 1,024 / 256 / 24,000 | Host CPU insns |
|---|---:|
| empty block | 607,419,443 |
| IOC takers sweeping, max bytes | 724,183,120 |

Even an empty block exceeded the 400M `exec_cpu_limit`, so a full lane would have stopped.

The host's budget table showed where the cost went: `WasmInsnExec` was 585M of the 607M. So the cost was the engine's own instructions, not host functions or Wasm parsing.

### 2. Profiling the phases

A profiling contract (`lanes/perps/engine/contracts/engine-profile`, never deployed) meters each phase at 1,024 / 256.

| Phase | `opt-level = "z"` | `"s"` | `2` | `3` |
|---|---:|---:|---:|---:|
| engine Wasm size (bytes) | 63,090 | 69,711 | 82,834 | 85,528 |
| decode state (CPU, cumulative) | 102M | 31M | 22M | 22M |
| decode + encode | 161M | 74M | 32M | 32M |
| decode + margin for every account | 466M | 177M | 172M | 172M |

Two changes followed. Neither changes a single output byte: every scenario hash and the 10,050-block parity run are unchanged.

- **`opt-level = 2` instead of `"z"`** (DEC-027). `"z"` refuses to inline, which is expensive in an interpreter. `2` is as fast as `3` and smaller.
- **Resting lots computed once per block** for the liquidation scan: one O(orders) pass instead of a scan of every book for every account. The state encoder also allocates its exact length once.

The result at 1,024 / 256 / 24,000:

| Block at 1,024 / 256 / 24,000 | Host CPU insns |
|---|---:|
| empty block | 134,571,167 |
| 3 oracle updates + funding | 136,063,798 |
| deposits up to the caps | 156,700,397 |
| GTC rest-rejects, max bytes | 193,557,854 |
| IOC takers sweeping, max bytes | 270,110,534 |
| CHECKPOINT_END | 188,187,218 |
| −9% shock, 511 liquidations | 253,857,750 |

### 3. Choosing the caps

What is left scales with state size, at about 70M for decoding and encoding 1,024 accounts. On top of that, each transaction costs about 0.4M in host signature checks and about 1M end to end. The spec's rule applies here: reduce caps before optimizing algorithms (§12.2).

| Caps (accounts / orders per side / block bytes) | Empty | Worst block |
|---|---:|---:|
| 1,024 / 256 / 12,000 | 134.6M | 253.9M (shock) |
| 512 / 256 / 12,000 | 77.7M | 148.6M (shock) |
| 512 / 128 / 12,000 | 73.1M | 125.4M (IOC sweep) |
| 512 / 128 / 8,000 | 73.1M | 114.4M (shock) |
| **256 / 128 / 12,000** | **43.4M** | **95.8M (IOC sweep)** |

256 / 128 / 12,000 is the largest tested set where every block shape stays under 100M. It still carries about 56 order transactions per 1-second block, enough for the T-007 load test of 50 tx/s.

## Random workloads

`cargo test -p caravel-runtime --test parity -- --ignored` runs 10,050 random blocks across 50 seeded lanes. On them:
- the Wasm and native paths produced identical state and receipt bytes at every block;
- the heaviest block used 18.5M host CPU instructions, since these states are small.

## Settlement contract: `submit_checkpoint` at the batch cap (T-006)

The settlement Wasm of record (`8a2fafbd…d503`, 42,900 bytes, the x86_64 Linux build from CI, DEC-033) is registered from its file, so the VM costs are counted. The macOS build of the same source (`8280828f…1a52`) meters identically. The call carries a real 3-block batch padded to exactly 96,000 bytes (the contract hashes the batch and does not parse it) and all 3 validator signatures. The budget is set to the live testnet transaction limits (spec §3.3).

Reproduce with `cargo test -p settlement a_full_batch -- --nocapture` after `./scripts/build-contracts.sh`.

| Resource | Used | Testnet limit per tx |
|---|---:|---:|
| CPU instructions | 7,889,048 | 400,000,000 |
| Memory bytes | 1,318,162 | 41,943,040 |
| Entries read from disk | 0 | 200 |
| Entries written | 2 (1,976 bytes) | 200 (132,096 bytes) |
| Event bytes | 236 | 16,384 |
| Arguments (XDR) | 96,892 bytes | tx size 132,096 bytes |

About 35 KB of the transaction-size limit is left for the envelope (source account, footprint, fee, one signature), which needs a few hundred bytes.

The SDK's fee estimate is 7,699,033 stroops, almost all of it rent (7,682,542) for the new 120-day `Ckpt(seq)` entry. The SDK computes it from hardcoded mainnet fee rates dated 2026-07-10, with a rent rate it calls a deliberate overestimate, so it is not a testnet fee. T-014 measures real fees on testnet.

## Sequencer soak: 1 hour at 50 tx/s (T-007)

`DURATION=3600 TPS=50 ./scripts/soak-sequencer.sh` on 2026-09-29, same machine. The sequencer ran the local lane (DEC-037) with 1 s blocks through the engine Wasm of record. The load generator (`platform/crates/caravel-node/examples/loadgen.rs`) sent 24 accounts' orders, IOC takers, cancels and small withdrawals, plus signed oracle updates every 2 s. There was no Stellar and there were no validators, so checkpoints were sealed but not signed. Halfway through, the sequencer was stopped and started again.

| Measure | Result |
|---|---:|
| Blocks | 3,603 |
| Transactions accepted (HTTP 202) | 180,002 at 50.0 tx/s |
| Rejected at the API / HTTP errors | 0 / 0 |
| Mempool at each 60 s report | 1 to 30 |
| Checkpoints sealed | 450 |
| Blocks per checkpoint | 8 (448), 9 (1), 10 (1) |
| Worst block, host CPU insns | 49,183,652 |

- **Restart.** The sequencer stopped at height 1,802. `caravel-node check-store` re-executed its whole store through the Wasm and got state hash `b7045073…6a`; the restarted sequencer resumed at height 1,802 with the same hash.
- **Final check.** `check-store` re-executed all 3,603 blocks through the Wasm and rebuilt all 450 checkpoint headers byte for byte.
- **Checkpoint spacing.** At this load a block carries about 50 transactions (roughly 10.5 KB), so after 8 blocks the next block might not fit in the 96,000-byte batch. Rule (b) of §14.2 then ends the batch before rule (a) would at 10 blocks. At lighter load, checkpoints come every 10 blocks.
