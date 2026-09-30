import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { encodeInboxMsg, fromHex, inboxAccAfter, toHex } from "./codec.js";

type Vector = { name: string; fields: Record<string, string>; hex: string; hash: string };
// The platform formats' vectors; the perps oracle update is tested with its
// feed module (lanes/perps/relayer-feeds).
const vectors = (file: string, dir = "platform/test-vectors") =>
  JSON.parse(readFileSync(fileURLToPath(new URL(`../../../${dir}/${file}`, import.meta.url)), "utf8")) as { context?: Record<string, string>; vectors: Vector[] };

describe("InboxMsgV1 (test-vectors/inbox_msg.json)", () => {
  for (const v of vectors("inbox_msg.json").vectors) {
    it(v.name, () => {
      const f = v.fields;
      const msg = encodeInboxMsg({ kind: Number(f.kind), index: BigInt(f.index!), laneAccount: fromHex(f.lane_account!), amount: BigInt(f.amount!), enqueuedAt: BigInt(f.enqueued_at!) });
      expect(toHex(msg)).toBe(v.hex);
      // hash_rule: hash = acc_after = H(TAG_INBOX || acc_before || msg).
      expect(toHex(inboxAccAfter(fromHex(f.acc_before!), msg))).toBe(f.acc_after);
      expect(f.acc_after).toBe(v.hash);
    });
  }
});

describe("encoding limits", () => {
  it("rejects out-of-range integers and wrong lengths", () => {
    const base = { kind: 0, index: 0n, laneAccount: new Uint8Array(32), amount: 1n, enqueuedAt: 0n };
    expect(() => encodeInboxMsg({ ...base, index: -1n })).toThrow(/u64/);
    expect(() => encodeInboxMsg({ ...base, amount: 1n << 127n })).toThrow(/i128/);
    expect(() => encodeInboxMsg({ ...base, laneAccount: new Uint8Array(31) })).toThrow(/32 bytes/);
    expect(toHex(encodeInboxMsg({ ...base, amount: -1n })).slice(82, 114)).toBe("ff".repeat(16));
  });
});
