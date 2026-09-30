import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { Keypair } from "@stellar/stellar-sdk";
import { describe, expect, it } from "vitest";

import { encodeOracleUpdate, fromHex, oracleSigningPreimage, sha256, toHex } from "./codec.js";

type Vector = { name: string; fields: Record<string, string>; hex: string; hash: string };
const file = JSON.parse(
  readFileSync(fileURLToPath(new URL("../../engine/test-vectors/oracle_update.json", import.meta.url)), "utf8"),
) as { context: Record<string, string>; vectors: Vector[] };

describe("OracleUpdateV1 (lanes/perps/engine/test-vectors/oracle_update.json)", () => {
  for (const v of file.vectors) {
    it(v.name, () => {
      const f = v.fields;
      const laneId = fromHex(file.context.lane_id!);
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

  it("rejects out-of-range fields and wrong lengths", () => {
    const base = { marketId: 1, price: 1n, publishTimeMs: 0n, oracleKey: new Uint8Array(32), signature: new Uint8Array(64) };
    expect(() => encodeOracleUpdate({ ...base, marketId: 70_000 })).toThrow(/u16/);
    expect(() => encodeOracleUpdate({ ...base, price: 1n << 63n })).toThrow(/i64/);
    expect(() => encodeOracleUpdate({ ...base, publishTimeMs: -1n })).toThrow(/u64/);
    expect(() => encodeOracleUpdate({ ...base, signature: new Uint8Array(63) })).toThrow(/64 bytes/);
    expect(() => oracleSigningPreimage(new Uint8Array(31), base)).toThrow(/32 bytes/);
    expect(encodeOracleUpdate(base).length).toBe(114);
  });
});
