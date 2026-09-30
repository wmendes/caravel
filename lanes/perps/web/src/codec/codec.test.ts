import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { fromHex, toHex } from "./bytes";
import { verify } from "./merkle";
import { encodeSigned, sep53Hash, sep53Message, signingBytes, txHash, type LaneTx, type TxBody } from "./tx";

type V = { name: string; fields: Record<string, unknown>; hex: string; hash: string };
const file = (name: string) => JSON.parse(readFileSync(join(import.meta.dirname, "../../../engine/test-vectors", name), "utf8")) as { context?: Record<string, string>; vectors: V[] };

function body(kind: string, b: Record<string, string | boolean>): TxBody {
  switch (kind) {
    case "PLACE_ORDER":
      return { kind: "place_order", marketId: Number(b.market_id), side: Number(b.side), tif: Number(b.tif), reduceOnly: b.reduce_only === true, price: BigInt(b.price as string), lots: BigInt(b.lots as string), clientOrderId: BigInt(b.client_order_id as string) };
    case "CANCEL_ORDER":
      return { kind: "cancel_order", marketId: Number(b.market_id), orderId: BigInt(b.order_id as string) };
    case "CANCEL_ALL":
      return { kind: "cancel_all", marketId: Number(b.market_id) };
    case "WITHDRAW":
      return { kind: "withdraw", amount: BigInt(b.amount as string) };
    case "ADD_SESSION_KEY":
      return { kind: "add_session_key", sessionKey: fromHex(b.session_key as string), expiresAtMs: BigInt(b.expires_at_ms as string), permissions: Number(b.permissions) };
    case "REVOKE_SESSION_KEY":
      return { kind: "revoke_session_key", sessionKey: fromHex(b.session_key as string) };
  }
  throw new Error(`unknown kind ${kind}`);
}

describe("LaneTxV1 (test-vectors/lane_tx.json)", () => {
  const f = file("lane_tx.json");
  const configHash = fromHex(f.context!.config_hash!);
  for (const v of f.vectors) {
    it(v.name, () => {
      const x = v.fields as Record<string, string>;
      const tx: LaneTx = {
        laneId: fromHex(x.lane_id!),
        account: fromHex(x.account!),
        signer: fromHex(x.signer!),
        nonce: BigInt(x.nonce!),
        expiryMs: BigInt(x.expiry_ms!),
        sigScheme: Number(x.sig_scheme),
        body: body(x.kind_name!, v.fields.body as Record<string, string | boolean>),
      };
      expect(toHex(signingBytes(tx))).toBe(x.signing_bytes);
      const hash = txHash(tx, configHash);
      expect(toHex(hash)).toBe(v.hash);
      expect(sep53Message(hash)).toBe(x.sep53_message);
      expect(toHex(sep53Hash(sep53Message(hash)))).toBe(x.sep53_hash);
      expect(toHex(encodeSigned(tx, fromHex(x.signature!)))).toBe(v.hex);
    });
  }
});

describe("Merkle proofs (test-vectors/merkle.json)", () => {
  for (const v of file("merkle.json").vectors) {
    it(v.name, () => {
      const x = v.fields as { n: string; proofs: { index: string; leaf: string; siblings: string[] }[] };
      const n = Number(x.n);
      const root = fromHex(v.hash);
      for (const p of x.proofs) {
        const siblings = p.siblings.map(fromHex);
        expect(verify(fromHex(p.leaf), Number(p.index), n, siblings, root)).toBe(true);
        // A tampered sibling or the wrong index fails.
        if (siblings.length > 0) {
          const bad = siblings.map((s) => s.slice());
          bad[0]![0]! ^= 1;
          expect(verify(fromHex(p.leaf), Number(p.index), n, bad, root)).toBe(false);
          expect(verify(fromHex(p.leaf), (Number(p.index) + 1) % n, n, siblings, root)).toBe(Number(p.index) === (Number(p.index) + 1) % n);
        }
      }
    });
  }
});
