/** Checkpoint cost metrics (spec §17.2 step 5, §19.6), one JSON line per checkpoint. */
import { appendFile, mkdir } from "node:fs/promises";
import { dirname } from "node:path";

export interface CheckpointMetric {
  seq: string;
  stellar_tx_hash: string;
  ledger: number;
  fee_charged_stroops: string;
  min_resource_fee_stroops: string;
  tx_size_bytes: number;
  header_bytes: number;
  batch_bytes: number;
  signatures: number;
  submitted_at: string;
}

export interface MetricsSink {
  record(m: CheckpointMetric): Promise<void>;
}

export class JsonlMetrics implements MetricsSink {
  constructor(private readonly path: string) {}

  async record(m: CheckpointMetric): Promise<void> {
    await mkdir(dirname(this.path), { recursive: true });
    await appendFile(this.path, `${JSON.stringify(m)}\n`);
  }
}
