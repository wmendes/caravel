/**
 * Checkpoint submitter (spec §17.2): takes the head of the sequencer's
 * submission queue and gets it accepted on Stellar, strictly in seq order.
 * Before submitting it reads `last_checkpoint()`: if Stellar already has the
 * seq (a previous run sent it and stopped before reporting), it only reports
 * it. It never reports a checkpoint whose on-chain header differs.
 */
import { sha256, toHex } from "./codec.js";
import type { MetricsSink } from "./metrics.js";
import type { SequencerApi } from "./sequencer.js";
import type { SettlementApi } from "./stellar.js";

export type SubmitOutcome = { kind: "idle" } | { kind: "reconciled"; seq: bigint; hash: string } | { kind: "submitted"; seq: bigint; hash: string };

export class CheckpointDivergence extends Error {}

export async function submitNext(seq: SequencerApi, st: SettlementApi, metrics: MetricsSink): Promise<SubmitOutcome> {
  const p = await seq.pendingCheckpoint();
  if (!p) return { kind: "idle" };
  const headerHash = toHex(sha256(p.header));
  const last = await st.lastCheckpoint();
  if (last.seq >= p.seq) {
    const onChain = await st.checkpoint(p.seq);
    if (!onChain) throw new Error(`Stellar is at seq ${last.seq} but has no record of ${p.seq}`);
    if (toHex(onChain.headerHash) !== headerHash) {
      throw new CheckpointDivergence(`seq ${p.seq}: Stellar accepted header ${toHex(onChain.headerHash)}, the sequencer has ${headerHash}`);
    }
    const tx = await st.findCheckpointTx(p.seq, onChain.stellarLedger);
    // Without the event (older than RPC retention) the ledger still identifies it.
    const hash = tx?.hash ?? "unknown";
    await seq.reportAccepted(p.seq, hash, tx?.ledger ?? onChain.stellarLedger);
    return { kind: "reconciled", seq: p.seq, hash };
  }
  if (last.seq + 1n !== p.seq) throw new Error(`Stellar is at seq ${last.seq}; the queue head is ${p.seq}`);
  const r = await st.submitCheckpoint(p);
  await metrics.record({
    seq: p.seq.toString(),
    stellar_tx_hash: r.hash,
    ledger: r.ledger,
    fee_charged_stroops: r.feeCharged.toString(),
    min_resource_fee_stroops: r.minResourceFee.toString(),
    tx_size_bytes: r.txSizeBytes,
    header_bytes: p.header.length,
    batch_bytes: p.batch.length,
    signatures: p.sigs.length,
    submitted_at: new Date().toISOString(),
  });
  await seq.reportAccepted(p.seq, r.hash, r.ledger);
  return { kind: "submitted", seq: p.seq, hash: r.hash };
}
