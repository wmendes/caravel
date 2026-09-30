/**
 * Building, signing and sending lane transactions (spec §18.3). Trades and
 * cancels use the device's session key when one is enabled; everything else
 * (withdrawals, enabling the key) is signed in Freighter with SEP-53.
 */
import { StrKey } from "@stellar/stellar-sdk";

import { ApiError, lane } from "./api/lane";
import { signSep53 } from "./api/wallet";
import { fromHex, toHex } from "./codec/bytes";
import { encodeSigned, PERM_CANCEL, PERM_TRADE, sep53Message, SigScheme, txHash, type LaneTx, type TxBody } from "./codec/tx";
import * as session from "./session";

export interface LaneIds {
  laneId: Uint8Array;
  configHash: Uint8Array;
}

const nonces = new Map<string, bigint>();

async function nextNonce(g: string, refresh = false): Promise<bigint> {
  const cached = nonces.get(g);
  if (cached !== undefined && !refresh) return cached;
  const a = await lane.account(g);
  if (!a) throw new Error("No lane account yet: deposit USDC first.");
  const n = BigInt(a.next_nonce);
  nonces.set(g, n);
  return n;
}

const SESSION_KINDS = new Set<TxBody["kind"]>(["place_order", "cancel_order", "cancel_all"]);

async function build(ids: LaneIds, g: string, body: TxBody, nonce: bigint, key: session.SessionKey | null): Promise<Uint8Array> {
  const account = StrKey.decodeEd25519PublicKey(g);
  const useKey = key !== null && SESSION_KINDS.has(body.kind);
  const tx: LaneTx = {
    laneId: ids.laneId,
    account,
    signer: useKey ? fromHex(key.publicHex) : account,
    nonce,
    expiryMs: BigInt(Date.now() + 60_000),
    sigScheme: useKey ? SigScheme.RawEd25519 : SigScheme.Sep53,
    body,
  };
  const hash = txHash(tx, ids.configHash);
  const signature = useKey ? await session.sign(key, hash) : await signSep53(sep53Message(hash), g);
  return encodeSigned(tx, signature);
}

/** Signs and posts `body`; on BAD_NONCE refetches the account and retries once. */
export async function send(ids: LaneIds, g: string, body: TxBody, key: session.SessionKey | null): Promise<string> {
  for (let attempt = 0; ; attempt++) {
    const nonce = await nextNonce(g, attempt > 0);
    try {
      const r = await lane.postTx(toHex(await build(ids, g, body, nonce, key)));
      nonces.set(g, nonce + 1n);
      return r.tx_hash;
    } catch (e) {
      if (e instanceof ApiError && e.code === "BAD_NONCE" && attempt === 0) continue;
      throw e;
    }
  }
}

/** Creates a trading key and registers it with a Freighter-signed ADD_SESSION_KEY. */
export async function enableSessionKey(ids: LaneIds, g: string): Promise<session.SessionKey> {
  const key = await session.create(g);
  await send(ids, g, { kind: "add_session_key", sessionKey: fromHex(key.publicHex), expiresAtMs: BigInt(key.expiresAtMs), permissions: PERM_TRADE | PERM_CANCEL }, null);
  await session.save(key);
  return key;
}

/** The ids transactions are signed for, or null if the sequencer does not report them. */
export function laneIds(status: { lane_id?: string; config_hash?: string } | null): LaneIds | null {
  if (!status?.lane_id || !status.config_hash) return null;
  return { laneId: fromHex(status.lane_id), configHash: fromHex(status.config_hash) };
}

/** Human text for a lane rejection code (engine receipts, spec §11). */
export function explainReject(e: unknown): string {
  if (e instanceof ApiError) return `${e.message} (${e.code})`;
  return e instanceof Error ? e.message : String(e);
}
