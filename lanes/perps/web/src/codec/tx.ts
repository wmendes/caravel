/**
 * LaneTxV1 (spec §9.2): encoding, tx_hash and the SEP-53 message a wallet
 * signs. Tested against test-vectors/lane_tx.json, the file the Rust tests use.
 */
import { ascii, concat, sha256, Writer } from "./bytes";

export const LANE_TX_VERSION = 1;
export const TX_HEADER_LEN = 117;
export const TAG_TX = ascii("CARAVEL/TX/V1");
export const SEP53_PREFIX = ascii("Stellar Signed Message:\n");
export const SEP53_TX_PREFIX = "Caravel lane tx ";
export const PERM_TRADE = 0x01;
export const PERM_CANCEL = 0x02;
export const ALL_MARKETS = 0xffff;

export enum SigScheme {
  RawEd25519 = 0,
  Sep53 = 1,
}

export enum Side {
  Buy = 0,
  Sell = 1,
}

export enum Tif {
  Gtc = 0,
  Ioc = 1,
  PostOnly = 2,
}

export type TxBody =
  | { kind: "place_order"; marketId: number; side: Side; tif: Tif; reduceOnly: boolean; price: bigint; lots: bigint; clientOrderId: bigint }
  | { kind: "cancel_order"; marketId: number; orderId: bigint }
  | { kind: "cancel_all"; marketId: number }
  | { kind: "withdraw"; amount: bigint }
  | { kind: "add_session_key"; sessionKey: Uint8Array; expiresAtMs: bigint; permissions: number }
  | { kind: "revoke_session_key"; sessionKey: Uint8Array };

const KIND: Record<TxBody["kind"], { id: number; len: number }> = {
  place_order: { id: 1, len: 29 },
  cancel_order: { id: 2, len: 10 },
  cancel_all: { id: 3, len: 2 },
  withdraw: { id: 4, len: 16 },
  add_session_key: { id: 5, len: 41 },
  revoke_session_key: { id: 6, len: 32 },
};

export interface LaneTx {
  laneId: Uint8Array;
  /** The owner's raw ed25519 key (their G... address). */
  account: Uint8Array;
  /** The owner, or one of its session keys. */
  signer: Uint8Array;
  nonce: bigint;
  expiryMs: bigint;
  sigScheme: SigScheme;
  body: TxBody;
}

function encodeBody(w: Writer, b: TxBody): void {
  switch (b.kind) {
    case "place_order":
      w.u16(b.marketId).u8(b.side).u8(b.tif).bool(b.reduceOnly).i64(b.price).i64(b.lots).u64(b.clientOrderId);
      break;
    case "cancel_order":
      w.u16(b.marketId).u64(b.orderId);
      break;
    case "cancel_all":
      w.u16(b.marketId);
      break;
    case "withdraw":
      w.i128(b.amount);
      break;
    case "add_session_key":
      w.bytes(b.sessionKey, 32).u64(b.expiresAtMs).u8(b.permissions);
      break;
    case "revoke_session_key":
      w.bytes(b.sessionKey, 32);
      break;
  }
}

/** The signed bytes: header and body, without the signature. */
export function signingBytes(tx: LaneTx): Uint8Array {
  const k = KIND[tx.body.kind];
  const w = new Writer(TX_HEADER_LEN + k.len);
  w.u8(LANE_TX_VERSION).bytes(tx.laneId, 32).bytes(tx.account, 32).bytes(tx.signer, 32).u64(tx.nonce).u64(tx.expiryMs).u8(k.id).u8(tx.sigScheme).u16(k.len);
  encodeBody(w, tx.body);
  return w.done();
}

/** `H(TAG_TX || config_hash || signing bytes)`. */
export function txHash(tx: LaneTx, configHash: Uint8Array): Uint8Array {
  return sha256(concat(TAG_TX, configHash, signingBytes(tx)));
}

/** The text a SEP-53 wallet (Freighter `signMessage`) signs for scheme 1. */
export function sep53Message(hash: Uint8Array): string {
  let hex = "";
  for (const x of hash) hex += x.toString(16).padStart(2, "0");
  return SEP53_TX_PREFIX + hex;
}

/** What the wallet's signature covers: `H(SEP53_PREFIX || message)`. */
export function sep53Hash(message: string): Uint8Array {
  return sha256(concat(SEP53_PREFIX, ascii(message)));
}

export function encodeSigned(tx: LaneTx, signature: Uint8Array): Uint8Array {
  if (signature.length !== 64) throw new Error("signature must be 64 bytes");
  return concat(signingBytes(tx), signature);
}
