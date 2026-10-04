import { mkdtemp, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { type CheckpointMetric, JsonlMetrics } from "./metrics.js";

const metric = (seq: number): CheckpointMetric => ({
  seq: String(seq),
  stellar_tx_hash: "00".repeat(32),
  ledger: seq,
  fee_charged_stroops: "100",
  min_resource_fee_stroops: "90",
  tx_size_bytes: 1000,
  header_bytes: 200,
  batch_bytes: 700,
  signatures: 2,
  submitted_at: "2026-10-04T00:00:00Z",
});

const seqs = async (path: string) =>
  (await readFile(path, "utf8"))
    .trim()
    .split("\n")
    .map((l) => (JSON.parse(l) as CheckpointMetric).seq);

describe("JsonlMetrics", () => {
  it("moves the file aside past its size cap, keeping one old file", async () => {
    const path = join(await mkdtemp(join(tmpdir(), "metrics-")), "sub", "m.jsonl");
    const line = JSON.stringify(metric(1)).length + 1;
    const sink = new JsonlMetrics(path, 2 * line);
    for (let seq = 1; seq <= 5; seq++) await sink.record(metric(seq));
    expect(await seqs(path)).toEqual(["5"]);
    expect(await seqs(`${path}.1`)).toEqual(["3", "4"]);
  });
});
