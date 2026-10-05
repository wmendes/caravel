/** Checkpoint cost metrics (spec §17.2 step 5, §19.6), one JSON line per checkpoint. */
import { appendFile, mkdir, rename, stat } from "node:fs/promises";
import { dirname } from "node:path";

export interface CheckpointMetric {
  seq: string;
  stellar_tx_hash: string;
  ledger: number;
  fee_charged_stroops: string;
  min_resource_fee_stroops: string;
  /** From the transaction's metadata (K-06); rent is part of the refundable fee. */
  rent_fee_stroops?: string | undefined;
  refundable_fee_stroops?: string | undefined;
  non_refundable_fee_stroops?: string | undefined;
  tx_size_bytes: number;
  header_bytes: number;
  batch_bytes: number;
  signatures: number;
  submitted_at: string;
}

export interface MetricsSink {
  record(m: CheckpointMetric): Promise<void>;
}

/** The file moves to `<path>.1` (replacing the previous one) past this size (F-05). */
export const METRICS_MAX_BYTES = 10 * 1024 * 1024;

export class JsonlMetrics implements MetricsSink {
  constructor(
    private readonly path: string,
    private readonly maxBytes = METRICS_MAX_BYTES,
  ) {}

  async record(m: CheckpointMetric): Promise<void> {
    await mkdir(dirname(this.path), { recursive: true });
    const size = await stat(this.path).then((s) => s.size, () => 0);
    if (size >= this.maxBytes) await rename(this.path, `${this.path}.1`);
    await appendFile(this.path, `${JSON.stringify(m)}\n`);
  }
}
