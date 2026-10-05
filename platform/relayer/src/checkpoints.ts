/**
 * Checkpoint submitter (spec §17.2): takes the head of the sequencer's
 * submission queue and gets it accepted on Stellar, strictly in seq order.
 * Before submitting it reads `last_checkpoint()`: if Stellar already has the
 * seq (a previous run sent it and stopped before reporting), it only reports
 * it. It never reports a checkpoint whose on-chain header differs.
 *
 * F-14: the checkpoint right after one this run saw accepted goes out without
 * that read (the contract refuses any seq but the next, so a surprise fails
 * the simulation, and the read comes back after any error). The sequencer
 * holds the pending request until a checkpoint is signed.
 */
import { sha256, toHex } from "./codec.js";
import type { MetricsSink } from "./metrics.js";
import type { PendingCheckpoint, SequencerApi } from "./sequencer.js";
import type { SettlementApi } from "./stellar.js";

export type SubmitOutcome = { kind: "idle" } | { kind: "reconciled"; seq: bigint; hash: string } | { kind: "submitted"; seq: bigint; hash: string };

export class CheckpointDivergence extends Error {}

/** What a running submitter knows: the last seq it saw accepted, if any. */
export interface SubmitState {
  lastAccepted: bigint | null;
}

export async function submitNext(seq: SequencerApi, st: SettlementApi, metrics: MetricsSink, state: SubmitState = { lastAccepted: null }, waitMs = 0): Promise<SubmitOutcome> {
  const p = await seq.pendingCheckpoint(waitMs);
  if (!p) return { kind: "idle" };
  if (state.lastAccepted === null || p.seq !== state.lastAccepted + 1n) {
    const r = await reconcile(seq, st, p);
    if (r) {
      state.lastAccepted = p.seq;
      return r;
    }
  }
  let r;
  try {
    r = await st.submitCheckpoint(p);
  } catch (e) {
    state.lastAccepted = null;
    throw e;
  }
  state.lastAccepted = p.seq;
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

/**
 * Reads `last_checkpoint()`: reports `p` if Stellar already has it, and
 * returns null if `p` is the next seq to send.
 */
async function reconcile(seq: SequencerApi, st: SettlementApi, p: PendingCheckpoint): Promise<SubmitOutcome | null> {
  const headerHash = toHex(sha256(p.header));
  const last = await st.lastCheckpoint();
  if (last.seq >= p.seq) {
    // A checkpoint with withdrawals has a record; since DEC-124 one without
    // has none, and `LastCkpt` (if it is the last) or its event says what
    // Stellar accepted.
    const onChain = await st.checkpoint(p.seq);
    const tx = await st.findCheckpointTx(p.seq, onChain?.stellarLedger);
    const accepted = onChain?.headerHash ?? (last.seq === p.seq ? last.headerHash : tx?.headerHash);
    if (!accepted) throw new Error(`Stellar is at seq ${last.seq} but has no record or event of ${p.seq} in RPC's window`);
    if (toHex(accepted) !== headerHash) {
      throw new CheckpointDivergence(`seq ${p.seq}: Stellar accepted header ${toHex(accepted)}, the sequencer has ${headerHash}`);
    }
    // Without the event (older than RPC retention) the record's ledger still identifies it.
    const hash = tx?.hash ?? "unknown";
    await seq.reportAccepted(p.seq, hash, tx?.ledger ?? onChain?.stellarLedger ?? 0);
    return { kind: "reconciled", seq: p.seq, hash };
  }
  if (last.seq + 1n !== p.seq) throw new Error(`Stellar is at seq ${last.seq}; the queue head is ${p.seq}`);
  return null;
}
