/** In-memory sequencer and settlement contract for the loop tests. */
import { inboxAccAfter, encodeInboxMsg, sha256 } from "./codec.js";
import type { InboxReply, MarketInfo, PendingCheckpoint, SequencerApi, SequencerStatus } from "./sequencer.js";
import type { CheckpointRecord, InboxRecord, SettlementApi, SubmitResult } from "./stellar.js";

export class FakeSettlement implements SettlementApi {
  inboxMsgs: InboxRecord[] = [];
  accepted: { seq: bigint; header: Uint8Array; ledger: number; hash: string }[] = [];
  ledger = 100;
  /** Throw after the transaction lands, as if the process died before reporting. */
  crashAfterSubmit = false;
  submits = 0;

  deposit(laneAccount: Uint8Array, amount: bigint, enqueuedAt: bigint): void {
    const index = BigInt(this.inboxMsgs.length);
    const prev = this.inboxMsgs.at(-1)?.accAfter ?? new Uint8Array(32);
    const msg = encodeInboxMsg({ kind: 0, index, laneAccount, amount, enqueuedAt });
    this.inboxMsgs.push({ kind: 0, laneAccount, amount, enqueuedAt, accAfter: inboxAccAfter(prev, msg) });
  }

  async inboxCount(): Promise<bigint> {
    return BigInt(this.inboxMsgs.length);
  }

  async inbox(index: bigint): Promise<InboxRecord | null> {
    return this.inboxMsgs[Number(index)] ?? null;
  }

  async lastCheckpoint(): Promise<{ seq: bigint; headerHash: Uint8Array }> {
    const last = this.accepted.at(-1);
    return last ? { seq: last.seq, headerHash: sha256(last.header) } : { seq: 0n, headerHash: new Uint8Array(32) };
  }

  async checkpoint(seq: bigint): Promise<CheckpointRecord | null> {
    const c = this.accepted.find((a) => a.seq === seq);
    return c ? { headerHash: sha256(c.header), stellarLedger: c.ledger } : null;
  }

  async findCheckpointTx(seq: bigint, fromLedger: number): Promise<{ hash: string; ledger: number } | null> {
    const c = this.accepted.find((a) => a.seq === seq && a.ledger >= fromLedger);
    return c ? { hash: c.hash, ledger: c.ledger } : null;
  }

  async submitCheckpoint(p: PendingCheckpoint): Promise<SubmitResult> {
    this.submits += 1;
    const last = await this.lastCheckpoint();
    if (p.seq !== last.seq + 1n) throw new Error("simulation: Error(Contract, #25)");
    this.ledger += 1;
    const hash = `${p.seq.toString().padStart(64, "0")}`;
    this.accepted.push({ seq: p.seq, header: p.header, ledger: this.ledger, hash });
    if (this.crashAfterSubmit) {
      this.crashAfterSubmit = false;
      throw new Error("process died before reporting");
    }
    return { hash, ledger: this.ledger, feeCharged: 1_000n, minResourceFee: 900n, txSizeBytes: p.batch.length + 700 };
  }
}

export class FakeSequencer implements SequencerApi {
  laneId = "11".repeat(32);
  inbox: { index: bigint; msgHex: string; accHex: string }[] = [];
  /** Expected acc chain for each index; a wrong one is a mismatch. */
  queue: PendingCheckpoint[] = [];
  reported: { seq: bigint; hash: string; ledger: number }[] = [];
  oracle: string[] = [];
  markets_: MarketInfo[] = [];
  failReportOnce = false;

  async status(): Promise<SequencerStatus> {
    return { lane_id: this.laneId, height: "1", inbox: { reported: String(this.inbox.length), reported_acc: "", processed: "0", halted: false } };
  }

  async markets(): Promise<MarketInfo[]> {
    return this.markets_;
  }

  async postInbox(index: bigint, msgHex: string, accAfterHex: string): Promise<InboxReply> {
    const n = BigInt(this.inbox.length);
    if (index < n) return { status: "known" };
    if (index > n) return { status: "gap", expected: n };
    const prev = this.inbox.at(-1)?.accHex ?? "00".repeat(32);
    const expected = Buffer.from(inboxAccAfter(Buffer.from(prev, "hex"), Buffer.from(msgHex, "hex"))).toString("hex");
    if (expected !== accAfterHex) return { status: "mismatch" };
    this.inbox.push({ index, msgHex, accHex: accAfterHex });
    return { status: "added" };
  }

  async postOracle(updateHex: string): Promise<void> {
    this.oracle.push(updateHex);
  }

  async pendingCheckpoint(): Promise<PendingCheckpoint | null> {
    return this.queue[0] ?? null;
  }

  async reportAccepted(seq: bigint, hash: string, ledger: number): Promise<void> {
    if (this.failReportOnce) {
      this.failReportOnce = false;
      throw new Error("sequencer unreachable");
    }
    if (this.queue[0]?.seq !== seq) throw new Error(`not the head: ${seq}`);
    this.queue.shift();
    this.reported.push({ seq, hash, ledger });
  }
}

export function pending(seq: bigint, fill = Number(seq)): PendingCheckpoint {
  return { seq, header: new Uint8Array(442).fill(fill), batch: new Uint8Array(100), epoch: 1n, sigs: [{ signer_index: 0, signature: new Uint8Array(64) }] };
}
