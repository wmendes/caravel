/**
 * "Trading key on this device" (spec §18.3): an ed25519 session key kept in
 * IndexedDB, allowed to trade and cancel for 24 h. Acceptable on testnet only.
 */
import * as ed from "@noble/ed25519";

import { fromHex, toHex } from "./codec/bytes";

export interface SessionKey {
  account: string;
  secretHex: string;
  publicHex: string;
  expiresAtMs: number;
}

const DB = "caravel";
const STORE = "session-keys";

function db(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const r = indexedDB.open(DB, 1);
    r.onupgradeneeded = () => r.result.createObjectStore(STORE, { keyPath: "account" });
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error);
  });
}

async function tx<T>(mode: IDBTransactionMode, f: (s: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  const d = await db();
  return new Promise((resolve, reject) => {
    const r = f(d.transaction(STORE, mode).objectStore(STORE));
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error);
  });
}

export async function load(account: string): Promise<SessionKey | null> {
  try {
    const k = (await tx("readonly", (s) => s.get(account))) as SessionKey | undefined;
    return k && k.expiresAtMs > Date.now() + 60_000 ? k : null;
  } catch {
    return null;
  }
}

export async function create(account: string, ttlMs = 24 * 3600 * 1000): Promise<SessionKey> {
  const secret = ed.utils.randomSecretKey();
  const pub = await ed.getPublicKeyAsync(secret);
  return { account, secretHex: toHex(secret), publicHex: toHex(pub), expiresAtMs: Date.now() + ttlMs };
}

export async function save(k: SessionKey): Promise<void> {
  await tx("readwrite", (s) => s.put(k));
}

export async function forget(account: string): Promise<void> {
  await tx("readwrite", (s) => s.delete(account));
}

export async function sign(k: SessionKey, message: Uint8Array): Promise<Uint8Array> {
  return ed.signAsync(message, fromHex(k.secretHex));
}
