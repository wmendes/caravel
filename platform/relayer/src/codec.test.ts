import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { Keypair } from "@stellar/stellar-sdk";
import { describe, expect, it } from "vitest";

import { encodeInboxMsg, encodeOracleUpdate, fromHex, inboxAccAfter, oracleSigningPreimage, sha256, toHex } from "./codec.js";

type Vector = { name: string; fields: Record<string, string>; hex: string; hash: string };
const vectors = (file: string) =>
  JSON.parse(readFileSync(fileURLToPath(new URL(`../../../lanes/perps/engine/test-vectors/${file}`, import.meta.url)), "utf8")) as { context?: Record<string, string>; vectors: Vector[] };

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

describe("OracleUpdateV1 (test-vectors/oracle_update.json)", () => {
  const file = vectors("oracle_update.json");
  for (const v of file.vectors) {
    it(v.name, () => {
      const f = v.fields;
      const laneId = fromHex(file.context!.lane_id!);
      const fields = { marketId: Number(f.market_id), price: BigInt(f.price!), publishTimeMs: BigInt(f.publish_time_ms!) };
      const preimage = oracleSigningPreimage(laneId, fields);
      expect(toHex(preimage)).toBe(f.signing_preimage);
      // hash_rule: hash = H(signing preimage).
      expect(toHex(sha256(preimage))).toBe(v.hash);
      // The fixture oracle key is the ed25519 key with seed [0x31; 32].
      const key = Keypair.fromRawEd25519Seed(Buffer.alloc(32, 0x31));
      expect(toHex(key.rawPublicKey())).toBe(f.oracle_key);
      const signature = key.sign(Buffer.from(sha256(preimage)));
      expect(toHex(signature)).toBe(f.signature);
      expect(toHex(encodeOracleUpdate({ ...fields, oracleKey: key.rawPublicKey(), signature }))).toBe(v.hex);
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
