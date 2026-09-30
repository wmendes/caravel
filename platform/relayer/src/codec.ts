/**
 * The platform byte formats the relayer builds (spec §9): InboxMsgV1 (§9.4),
 * little-endian, and the inbox hash chain. Checked against
 * `platform/test-vectors/`, the files the Rust tests use. An app's feed
 * formats live with its feed module (perps: `lanes/perps/relayer-feeds`).
 */
import { createHash } from "node:crypto";

export const TAG_INBOX = new TextEncoder().encode("CARAVEL/INBOX/V1");
export const INBOX_MSG_LEN = 65;

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

export function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let o = 0;
  for (const p of parts) {
    out.set(p, o);
    o += p.length;
  }
  return out;
}

class Writer {
  private readonly buf: Uint8Array;
  private readonly view: DataView;
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

  u64(v: bigint): this {
    if (v < 0n || v >= 1n << 64n) throw new Error(`u64 out of range: ${v}`);
    this.view.setBigUint64(this.at, v, true);
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

export enum InboxKind {
  Deposit = 0,
  ForcedWithdrawal = 1,
}

export interface InboxMsg {
  kind: InboxKind;
  index: bigint;
  laneAccount: Uint8Array;
  amount: bigint;
  /** Stellar ledger timestamp, seconds. */
  enqueuedAt: bigint;
}

/** `kind u8 · index u64 · lane_account [32] · amount i128 · enqueued_at u64`. */
export function encodeInboxMsg(m: InboxMsg): Uint8Array {
  return new Writer(INBOX_MSG_LEN).u8(m.kind).u64(m.index).bytes(m.laneAccount, 32).i128(m.amount).u64(m.enqueuedAt).done();
}

/** `acc_after = H(TAG_INBOX || acc_prev || msg)`. */
export function inboxAccAfter(accPrev: Uint8Array, msg: Uint8Array): Uint8Array {
  return sha256(concat(TAG_INBOX, accPrev, msg));
}
