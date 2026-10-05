import { describe, expect, it } from "vitest";

import { CheckpointDivergence, submitNext } from "./checkpoints.js";
import { FakeSequencer, FakeSettlement, pending } from "./fakes.js";
import { InboxMismatch, syncInbox } from "./inbox.js";
import type { CheckpointMetric, MetricsSink } from "./metrics.js";

class Metrics implements MetricsSink {
  rows: CheckpointMetric[] = [];
  async record(m: CheckpointMetric): Promise<void> {
    this.rows.push(m);
  }
}

const key = (n: number) => new Uint8Array(32).fill(n);

describe("inbox watcher (spec §17.1)", () => {
  it("forwards every message in order and is idempotent across restarts", async () => {
    const st = new FakeSettlement();
    const seq = new FakeSequencer();
    for (let i = 0; i < 5; i++) st.deposit(key(i + 1), BigInt(i + 1) * 10_000_000n, 1_790_000_000n + BigInt(i));
    expect(await syncInbox(seq, st, 3)).toBe(3);
    // A fresh relayer process: the sequencer's count is the cursor.
    expect(await syncInbox(seq, st)).toBe(2);
    expect(await syncInbox(seq, st)).toBe(0);
    expect(seq.inbox.map((m) => m.index)).toEqual([0n, 1n, 2n, 3n, 4n]);
  });

  it("stops on an acc mismatch", async () => {
    const st = new FakeSettlement();
    const seq = new FakeSequencer();
    st.deposit(key(1), 10_000_000n, 1n);
    st.inboxMsgs[0]!.accAfter = new Uint8Array(32).fill(9);
    await expect(syncInbox(seq, st)).rejects.toBeInstanceOf(InboxMismatch);
  });
});

describe("checkpoint submitter (spec §17.2)", () => {
  it("submits in order and records costs", async () => {
    const st = new FakeSettlement();
    const seq = new FakeSequencer();
    const m = new Metrics();
    seq.queue.push(pending(1n), pending(2n));
    expect((await submitNext(seq, st, m)).kind).toBe("submitted");
    expect((await submitNext(seq, st, m)).kind).toBe("submitted");
    expect((await submitNext(seq, st, m)).kind).toBe("idle");
    expect(seq.reported.map((r) => r.seq)).toEqual([1n, 2n]);
    expect(m.rows.map((r) => r.seq)).toEqual(["1", "2"]);
    expect(m.rows[0]).toMatchObject({ fee_charged_stroops: "1000", min_resource_fee_stroops: "900", header_bytes: 442, batch_bytes: 100, signatures: 1 });
  });

  it("reconciles a checkpoint Stellar accepted before the relayer died", async () => {
    const st = new FakeSettlement();
    const seq = new FakeSequencer();
    seq.queue.push(pending(1n));
    st.crashAfterSubmit = true;
    await expect(submitNext(seq, st, new Metrics())).rejects.toThrow(/died/);
    // Restart: Stellar has seq 1, so it is reported, not sent again.
    const r = await submitNext(seq, st, new Metrics());
    expect(r).toMatchObject({ kind: "reconciled", seq: 1n });
    expect(st.submits).toBe(1);
    expect(seq.reported[0]).toMatchObject({ seq: 1n, ledger: 101 });
  });

  it("retries a failed report without sending again", async () => {
    const st = new FakeSettlement();
    const seq = new FakeSequencer();
    seq.queue.push(pending(1n));
    seq.failReportOnce = true;
    await expect(submitNext(seq, st, new Metrics())).rejects.toThrow(/unreachable/);
    expect((await submitNext(seq, st, new Metrics())).kind).toBe("reconciled");
    expect(st.submits).toBe(1);
  });

  it("never reports a checkpoint whose on-chain header differs", async () => {
    const st = new FakeSettlement();
    const seq = new FakeSequencer();
    st.accepted.push({ seq: 1n, header: new Uint8Array(442).fill(7), ledger: 50, hash: "x" });
    seq.queue.push(pending(1n, 1));
    await expect(submitNext(seq, st, new Metrics())).rejects.toBeInstanceOf(CheckpointDivergence);
    expect(seq.reported).toEqual([]);
  });

  it("sends the next checkpoint without reading Stellar first (F-14)", async () => {
    const st = new FakeSettlement();
    const seq = new FakeSequencer();
    const state = { lastAccepted: null };
    seq.queue.push(pending(1n), pending(2n), pending(3n));
    for (let i = 0; i < 3; i++) expect((await submitNext(seq, st, new Metrics(), state)).kind).toBe("submitted");
    expect(st.lastReads).toBe(1);
    expect(state.lastAccepted).toBe(3n);
  });

  it("reads Stellar again after a failed send, and reports what is there (F-14)", async () => {
    const st = new FakeSettlement();
    const seq = new FakeSequencer();
    const state = { lastAccepted: null };
    const p2 = pending(2n);
    seq.queue.push(pending(1n), p2);
    expect((await submitNext(seq, st, new Metrics(), state)).kind).toBe("submitted");
    // Another relayer got seq 2 in first: the send fails the simulation.
    st.accepted.push({ seq: 2n, header: p2.header, ledger: 200, hash: "other" });
    await expect(submitNext(seq, st, new Metrics(), state)).rejects.toThrow(/simulation/);
    expect(state.lastAccepted).toBeNull();
    expect(await submitNext(seq, st, new Metrics(), state)).toMatchObject({ kind: "reconciled", seq: 2n, hash: "other" });
    expect(st.lastReads).toBe(2);
    expect(st.submits).toBe(2);
  });

  it("reconciles without a record, from LastCkpt or the event (DEC-124)", async () => {
    const st = new FakeSettlement();
    st.noRecords = true;
    const seq = new FakeSequencer();
    seq.queue.push(pending(1n), pending(2n));
    // Both land, and the relayer dies before reporting either.
    st.crashAfterSubmit = true;
    await expect(submitNext(seq, st, new Metrics())).rejects.toThrow(/died/);
    st.crashAfterSubmit = false;
    await st.submitCheckpoint(pending(2n));
    // Seq 1 is older than LastCkpt: its event says what was accepted.
    expect(await submitNext(seq, st, new Metrics())).toMatchObject({ kind: "reconciled", seq: 1n });
    // Seq 2 is LastCkpt.
    expect(await submitNext(seq, st, new Metrics())).toMatchObject({ kind: "reconciled", seq: 2n });
    expect(st.submits).toBe(2);
    expect(seq.reported.map((r) => r.seq)).toEqual([1n, 2n]);
  });

  it("refuses a different header without a record too", async () => {
    const st = new FakeSettlement();
    st.noRecords = true;
    const seq = new FakeSequencer();
    st.accepted.push({ seq: 1n, header: new Uint8Array(442).fill(7), ledger: 50, hash: "x" });
    seq.queue.push(pending(1n, 1));
    await expect(submitNext(seq, st, new Metrics())).rejects.toBeInstanceOf(CheckpointDivergence);
  });

  it("refuses to skip a seq", async () => {
    const st = new FakeSettlement();
    const seq = new FakeSequencer();
    seq.queue.push(pending(3n));
    await expect(submitNext(seq, st, new Metrics())).rejects.toThrow(/Stellar is at seq 0/);
  });
});
