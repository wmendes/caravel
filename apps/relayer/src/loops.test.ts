import { Keypair } from "@stellar/stellar-sdk";
import { describe, expect, it } from "vitest";

import { CheckpointDivergence, submitNext } from "./checkpoints.js";
import { fromHex, oracleSigningPreimage, sha256 } from "./codec.js";
import { FakeSequencer, FakeSettlement, pending } from "./fakes.js";
import { InboxMismatch, syncInbox } from "./inbox.js";
import type { CheckpointMetric, MetricsSink } from "./metrics.js";
import { OracleFeeder } from "./oracle.js";
import { FixedPrice, type PriceSource, parseDecimal } from "./prices.js";

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

  it("refuses to skip a seq", async () => {
    const st = new FakeSettlement();
    const seq = new FakeSequencer();
    seq.queue.push(pending(3n));
    await expect(submitNext(seq, st, new Metrics())).rejects.toThrow(/Stellar is at seq 0/);
  });
});

describe("oracle feeder (spec §17.3)", () => {
  const market = { market_id: 1, symbol: "BTC-PERP", tick: "1000", display_lot_base_units: 10_000, display_base_decimals: 8 };
  const oracle = Keypair.fromRawEd25519Seed(Buffer.alloc(32, 0x31));

  it("signs updates the engine accepts and skips small moves inside 10 s", async () => {
    const seq = new FakeSequencer();
    let now = 1_790_000_000_000;
    let price = "65000";
    const src: PriceSource = { name: "test", quote: async () => ({ usd: parseDecimal(price), observedAt: Math.floor(now / 1000), source: "test" }) };
    const feeder = new OracleFeeder(seq, oracle, fromHex(seq.laneId), [{ info: market, sources: [src] }], 900, () => now);
    expect((await feeder.tick()).published).toHaveLength(1);
    const u = fromHex(seq.oracle[0]!);
    expect(u.length).toBe(114);
    // The signature verifies over H(TAG_ORACLE || lane_id || signed fields).
    const fields = { marketId: 1, price: 65_000_000n, publishTimeMs: BigInt(now) };
    expect(Keypair.fromPublicKey(oracle.publicKey()).verify(Buffer.from(sha256(oracleSigningPreimage(fromHex(seq.laneId), fields))), Buffer.from(u.slice(50)))).toBe(true);
    now += 2_000;
    price = "65000.0004"; // +0.4 stroops per lot: less than a tick
    expect((await feeder.tick()).published).toHaveLength(0);
    now += 2_000;
    price = "65001"; // +1,000 stroops per lot: one tick
    expect((await feeder.tick()).published[0]?.price).toBe(65_001_000n);
    now += 10_000;
    expect((await feeder.tick()).published).toHaveLength(1); // heartbeat
  });

  it("reports a market without a fresh price and keeps the others going", async () => {
    const seq = new FakeSequencer();
    const dead: PriceSource = { name: "dead", quote: async () => Promise.reject(new Error("down")) };
    const eth = { ...market, market_id: 2, display_lot_base_units: 100_000 };
    const feeder = new OracleFeeder(seq, oracle, fromHex(seq.laneId), [{ info: market, sources: [dead] }, { info: eth, sources: [new FixedPrice("3500")] }], 900);
    const r = await feeder.tick();
    expect(r.published.map((p) => [p.marketId, p.price])).toEqual([[2, 35_000_000n]]);
    expect(r.errors[0]).toMatch(/market 1: .*down/);
  });
});
