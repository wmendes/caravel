/**
 * OracleUpdateV1 (spec §9.5), the perps oracle feed's payload, little-endian,
 * and the hash its key signs. Checked against
 * `lanes/perps/engine/test-vectors/oracle_update.json`, the file the Rust
 * tests use.
 */
import { createHash } from "node:crypto";

export const TAG_ORACLE = new TextEncoder().encode("CARAVEL/ORACLE/V1");
export const ORACLE_UPDATE_LEN = 114;

export function sha256(data: Uint8Array): Uint8Array {
  return new Uint8Array(createHash("sha256").update(data).digest());
}

export function toHex(b: Uint8Array): string {
  return Buffer.from(b).toString("hex");
}

export function fromHex(s: string): Uint8Array {
  if (!/^([0-9a-fA-F]{2})*$/.test(s)) throw new Error(`not hex: ${s.slice(0, 16)}`);
  return new Uint8Array(Buffer.from(s, "hex"));
}

function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let o = 0;
  for (const p of parts) {
    out.set(p, o);
    o += p.length;
  }
  return out;
}

function fixed(b: Uint8Array, len: number): Uint8Array {
  if (b.length !== len) throw new Error(`expected ${len} bytes, got ${b.length}`);
  return b;
}

export interface OracleUpdate {
  marketId: number;
  /** USDC stroops per lot. */
  price: bigint;
  publishTimeMs: bigint;
  oracleKey: Uint8Array;
  signature: Uint8Array;
}

/** `market_id u16 · price i64 · publish_time_ms u64`: the signed bytes. */
export function oracleSignedFields(u: Omit<OracleUpdate, "signature" | "oracleKey">): Uint8Array {
  if (!Number.isInteger(u.marketId) || u.marketId < 0 || u.marketId > 0xffff) throw new Error(`u16 out of range: ${u.marketId}`);
  if (u.price < -(1n << 63n) || u.price >= 1n << 63n) throw new Error(`i64 out of range: ${u.price}`);
  if (u.publishTimeMs < 0n || u.publishTimeMs >= 1n << 64n) throw new Error(`u64 out of range: ${u.publishTimeMs}`);
  const out = new Uint8Array(18);
  const v = new DataView(out.buffer);
  v.setUint16(0, u.marketId, true);
  v.setBigInt64(2, u.price, true);
  v.setBigUint64(10, u.publishTimeMs, true);
  return out;
}

/** `TAG_ORACLE || lane_id || signed fields`; the oracle key signs its SHA-256. */
export function oracleSigningPreimage(laneId: Uint8Array, u: Omit<OracleUpdate, "signature" | "oracleKey">): Uint8Array {
  if (laneId.length !== 32) throw new Error("lane_id must be 32 bytes");
  return concat(TAG_ORACLE, laneId, oracleSignedFields(u));
}

export function encodeOracleUpdate(u: OracleUpdate): Uint8Array {
  return concat(oracleSignedFields(u), fixed(u.oracleKey, 32), fixed(u.signature, 64));
}
