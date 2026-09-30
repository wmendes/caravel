/** Freighter (spec §18.3), checked against @stellar/freighter-api 6.0.1's typings. */
import { getAddress, getNetworkDetails, isConnected, requestAccess, signMessage, signTransaction } from "@stellar/freighter-api";

import { config } from "../config";

export class WalletError extends Error {}

function check<T extends { error?: { message?: string } | undefined }>(r: T, what: string): T {
  if (r.error) throw new WalletError(`${what}: ${r.error.message ?? "refused"}`);
  return r;
}

/** Asks Freighter for the account; the wallet must be on the lane's network. */
export async function connect(): Promise<string> {
  const c = await isConnected();
  if (!c.isConnected) throw new WalletError("Freighter is not installed. Install the Freighter extension, then reload this page.");
  const { address } = check(await requestAccess(), "Freighter");
  await assertNetwork();
  return address;
}

/** The connected account, if the site is already allowed. */
export async function current(): Promise<string | null> {
  const c = await isConnected();
  if (!c.isConnected) return null;
  const r = await getAddress();
  return r.error || !r.address ? null : r.address;
}

export async function assertNetwork(): Promise<void> {
  const n = check(await getNetworkDetails(), "Freighter network");
  if (n.networkPassphrase !== config.networkPassphrase) {
    throw new WalletError(`Freighter is on ${n.network || "another network"}. Switch it to ${config.networkName} (Settings → Network) and try again.`);
  }
}

/** Signs `message` per SEP-53. Freighter returns a Buffer (v3) or base64 (v4). */
export async function signSep53(message: string, address: string): Promise<Uint8Array> {
  const r = check(await signMessage(message, { networkPassphrase: config.networkPassphrase, address }), "Sign message");
  const s = r.signedMessage;
  if (s === null || s === undefined) throw new WalletError("Sign message: no signature");
  const bytes = typeof s === "string" ? Uint8Array.from(atob(s), (c) => c.charCodeAt(0)) : new Uint8Array(s);
  if (bytes.length !== 64) throw new WalletError(`Sign message: expected a 64-byte signature, got ${bytes.length}`);
  return bytes;
}

export async function signTx(xdr: string, address: string): Promise<string> {
  await assertNetwork();
  return check(await signTransaction(xdr, { networkPassphrase: config.networkPassphrase, address }), "Sign transaction").signedTxXdr;
}
