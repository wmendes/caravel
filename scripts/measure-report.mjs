#!/usr/bin/env node
// T-014 (spec §19.6): turns one load run into the numbers in docs/RESULTS.md.
//
//   node scripts/measure-report.mjs --api URL --loadgen run.jsonl --metrics relayer-checkpoints.jsonl [--label TEXT]
//
// --loadgen is the load generator's output (its last line, `"final": true`,
// is the summary). --metrics is the relayer's checkpoint log (one JSON line
// per submitted checkpoint: fee charged, minResourceFee, tx size). The
// checkpoints counted are the ones whose blocks all fall inside the load
// window; their user transactions come from the lane's block API. Prints a
// JSON record and a Markdown table. No dependencies.
import { readFileSync } from "node:fs";

const args = Object.fromEntries(
  process.argv
    .slice(2)
    .reduce((acc, a, i, all) => (a.startsWith("--") ? [...acc, [a.slice(2), all[i + 1]]] : acc), []),
);
for (const k of ["api", "loadgen", "metrics"]) if (!args[k]) throw new Error(`--${k} is required`);
const api = args.api.replace(/\/$/, "");

const lines = (p) => readFileSync(p, "utf8").split("\n").filter((l) => l.trim().startsWith("{")).map((l) => JSON.parse(l));
const run = lines(args.loadgen).findLast((r) => r.final);
if (!run) throw new Error("no final summary in the load generator output");
const metrics = lines(args.metrics);

async function get(path) {
  for (let attempt = 0; ; attempt++) {
    try {
      const r = await fetch(`${api}${path}`);
      if (!r.ok) throw new Error(`${path}: HTTP ${r.status}`);
      return await r.json();
    } catch (e) {
      if (attempt >= 3) throw e;
      await new Promise((ok) => setTimeout(ok, 500 * (attempt + 1)));
    }
  }
}

async function pool(items, n, f) {
  const out = new Array(items.length);
  let next = 0;
  await Promise.all(
    Array.from({ length: n }, async () => {
      while (next < items.length) {
        const i = next++;
        out[i] = await f(items[i]);
      }
    }),
  );
  return out;
}

const pct = (v, p) => {
  if (v.length === 0) return null;
  const s = [...v].sort((a, b) => a - b);
  return s[Math.round((p / 100) * (s.length - 1))];
};
const xlm = (stroops) => stroops / 1e7;

// Checkpoints whose blocks were all produced under load, and the last ten
// before the load with no user transaction at all (the idle lane: oracle
// updates only).
const load = [];
const before = [];
for (const m of metrics) {
  const c = await get(`/v1/checkpoints/${m.seq}`).catch(() => null);
  if (!c) continue;
  const row = { ...m, first: Number(c.first_block_height), last: Number(c.last_block_height) };
  row.blocks = row.last - row.first + 1;
  if (row.first > run.start_height && row.last <= run.end_height) load.push(row);
  else if (row.last <= run.start_height) before.push(row);
}
const candidates = before.slice(-30);
if (load.length === 0) throw new Error("no checkpoint falls inside the load window");

// User transactions in those blocks, from the lane's block API.
const heights = [...candidates, ...load].flatMap((c) => Array.from({ length: c.blocks }, (_, i) => c.first + i));
const blocks = new Map(
  (await pool(heights, 8, async (h) => [h, await get(`/v1/blocks/${h}`)])).map(([h, b]) => [
    h,
    { user: b.entries.filter((e) => e.type === "user").length, entries: b.entries.length },
  ]),
);
for (const c of [...candidates, ...load]) {
  c.user_txs = 0;
  for (let h = c.first; h <= c.last; h++) c.user_txs += blocks.get(h).user;
}
const idle = candidates.filter((c) => c.user_txs === 0).slice(-10);

function summarize(group) {
  if (group.length === 0) return null;
  const fee = group.map((c) => Number(c.fee_charged_stroops));
  const minFee = group.map((c) => Number(c.min_resource_fee_stroops));
  const userTxs = group.reduce((a, c) => a + c.user_txs, 0);
  const totalFee = fee.reduce((a, b) => a + b, 0);
  return {
    count: group.length,
    seqs: `${group[0].seq}..${group.at(-1).seq}`,
    blocks_p50: pct(group.map((c) => c.blocks), 50),
    batch_bytes_p50: pct(group.map((c) => c.batch_bytes), 50),
    batch_bytes_max: Math.max(...group.map((c) => c.batch_bytes)),
    tx_size_bytes_p50: pct(group.map((c) => c.tx_size_bytes), 50),
    tx_size_bytes_max: Math.max(...group.map((c) => c.tx_size_bytes)),
    min_resource_fee_xlm_p50: xlm(pct(minFee, 50)),
    fee_charged_xlm_p50: xlm(pct(fee, 50)),
    fee_charged_xlm_max: xlm(Math.max(...fee)),
    user_txs: userTxs,
    fee_charged_xlm_per_1000_lane_tx: userTxs > 0 ? Math.round((xlm(totalFee) / userTxs) * 1000 * 1e4) / 1e4 : null,
    example_tx: group.at(-1).stellar_tx_hash,
  };
}
const loadBlocks = load.flatMap((c) => Array.from({ length: c.blocks }, (_, i) => blocks.get(c.first + i).user));

// Hard latency of receipts in those checkpoints only: after the load stops,
// the last receipts wait for an idle checkpoint, which says nothing about load.
const inLoad = (h) => load.some((c) => h >= c.first && h <= c.last);
const hard = (run.hard_samples ?? []).filter(([h]) => inLoad(h)).map(([, ms]) => ms);
const hardLatency = run.hard_samples ? { n: hard.length, p50: pct(hard, 50), p99: pct(hard, 99) } : run.hard_latency_ms;
const userTxs = loadBlocks.reduce((a, b) => a + b, 0);

const out = {
  label: args.label ?? null,
  measured_at: new Date().toISOString(),
  api,
  load: {
    seconds: run.load_s,
    target_tx_per_s: args.tps ? Number(args.tps) : null,
    sent: run.sent,
    queued: run.queued,
    rejected_at_submit: run.rejected,
    http_errors: run.http_errors,
    receipts_ok: run.receipts.ok,
    receipts_rejected_by_code: run.receipts.rejected_by_code,
  },
  blocks_per_s: run.blocks_per_s,
  user_tx_per_s: {
    queued: run.queued_tx_per_s,
    included_in_window: Math.round((userTxs / loadBlocks.length) * 100) / 100,
    per_block_p50: pct(loadBlocks, 50),
    per_block_max: Math.max(...loadBlocks),
  },
  host_cpu_insns_per_block: run.host_cpu_insns_per_block,
  soft_latency_ms: run.soft_latency_ms,
  hard_latency_ms: hardLatency,
  checkpoints: summarize(load),
  idle_checkpoints: summarize(idle),
};
console.log(JSON.stringify(out));

const f = (v, d = 0) => (v === null || v === undefined ? "–" : Number(v).toLocaleString("en-US", { maximumFractionDigits: d, minimumFractionDigits: d }));
const c = out.checkpoints;
console.log(`
| Measure | Value |
|---|---|
| Load | ${f(out.load.seconds)} s, ${f(out.load.queued)} tx queued (${f(out.user_tx_per_s.queued, 1)} tx/s), ${f(out.load.receipts_ok)} executed OK |
| Blocks/s | ${f(out.blocks_per_s, 2)} |
| User tx/s in checkpointed blocks | ${f(out.user_tx_per_s.included_in_window, 1)} (p50 ${f(out.user_tx_per_s.per_block_p50)} / max ${f(out.user_tx_per_s.per_block_max)} per block) |
| Host \`cpu_insns\` per block | p50 ${f(out.host_cpu_insns_per_block.p50)}, p99 ${f(out.host_cpu_insns_per_block.p99)} (${f(out.host_cpu_insns_per_block.n)} blocks sampled) |
| Soft latency (POST → receipt on WS) | p50 ${f(out.soft_latency_ms.p50)} ms, p99 ${f(out.soft_latency_ms.p99)} ms (n = ${f(out.soft_latency_ms.n)}) |
| Hard latency (receipt → checkpoint accepted) | p50 ${f(out.hard_latency_ms.p50 / 1000, 1)} s, p99 ${f(out.hard_latency_ms.p99 / 1000, 1)} s (n = ${f(out.hard_latency_ms.n)}) |
| Checkpoints | ${c.count} (seq ${c.seqs}), p50 ${f(c.blocks_p50)} blocks each |
| Batch size | p50 ${f(c.batch_bytes_p50)} B, max ${f(c.batch_bytes_max)} B |
| Checkpoint tx size | p50 ${f(c.tx_size_bytes_p50)} B, max ${f(c.tx_size_bytes_max)} B |
| \`minResourceFee\` per checkpoint | p50 ${f(c.min_resource_fee_xlm_p50, 4)} XLM |
| Fee charged per checkpoint | p50 ${f(c.fee_charged_xlm_p50, 4)} XLM, max ${f(c.fee_charged_xlm_max, 4)} XLM |
| Fee charged per 1,000 lane tx | ${f(c.fee_charged_xlm_per_1000_lane_tx, 4)} XLM (${f(c.user_txs)} user tx in these checkpoints) |
`);
const i = out.idle_checkpoints;
if (i) {
  console.log(`| Idle lane, before the load | Value |
|---|---|
| Checkpoints | ${i.count} (seq ${i.seqs}), p50 ${f(i.blocks_p50)} blocks, ${f(i.user_txs)} user tx |
| Batch size / checkpoint tx size | p50 ${f(i.batch_bytes_p50)} B / ${f(i.tx_size_bytes_p50)} B |
| \`minResourceFee\` / fee charged per checkpoint | p50 ${f(i.min_resource_fee_xlm_p50, 4)} XLM / ${f(i.fee_charged_xlm_p50, 4)} XLM |
`);
}
