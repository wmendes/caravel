/** Byte helpers for the lane formats (spec §9): little-endian, exact lengths. */
import { sha256 as nobleSha256 } from "@noble/hashes/sha2.js";

export function sha256(data: Uint8Array): Uint8Array {
  return nobleSha256(data);
}

export function toHex(b: Uint8Array): string {
  let s = "";
  for (const x of b) s += x.toString(16).padStart(2, "0");
  return s;
}

export function fromHex(s: string): Uint8Array {
  const h = s.startsWith("0x") ? s.slice(2) : s;
  if (!/^([0-9a-fA-F]{2})*$/.test(h)) throw new Error("not hex");
  const out = new Uint8Array(h.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(h.slice(2 * i, 2 * i + 2), 16);
  return out;
}

export function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let o = 0;
  for (const p of parts) {
    out.set(p, o);
    o += p.length;
  }
  return out;
}

export const ascii = (s: string): Uint8Array => new TextEncoder().encode(s);

export class Writer {
  private buf: Uint8Array;
  private view: DataView;
  private at = 0;

  constructor(len: number) {
    this.buf = new Uint8Array(len);
    this.view = new DataView(this.buf.buffer);
  }

  u8(v: number): this {
    this.view.setUint8(this.at, v);
    this.at += 1;
    return this;
  }

  u16(v: number): this {
    this.view.setUint16(this.at, v, true);
    this.at += 2;
    return this;
  }

  u64(v: bigint): this {
    if (v < 0n || v >= 1n << 64n) throw new Error(`u64 out of range: ${v}`);
    this.view.setBigUint64(this.at, v, true);
    this.at += 8;
    return this;
  }

  i64(v: bigint): this {
    if (v < -(1n << 63n) || v >= 1n << 63n) throw new Error(`i64 out of range: ${v}`);
    this.view.setBigInt64(this.at, v, true);
    this.at += 8;
    return this;
  }

  i128(v: bigint): this {
    if (v < -(1n << 127n) || v >= 1n << 127n) throw new Error(`i128 out of range: ${v}`);
    const u = BigInt.asUintN(128, v);
    this.view.setBigUint64(this.at, u & ((1n << 64n) - 1n), true);
    this.view.setBigUint64(this.at + 8, u >> 64n, true);
    this.at += 16;
    return this;
  }

  bool(v: boolean): this {
    return this.u8(v ? 1 : 0);
  }

  bytes(b: Uint8Array, len: number): this {
    if (b.length !== len) throw new Error(`expected ${len} bytes, got ${b.length}`);
    this.buf.set(b, this.at);
    this.at += len;
    return this;
  }

  done(): Uint8Array {
    if (this.at !== this.buf.length) throw new Error("length mismatch");
    return this.buf;
  }
}
