/**
 * The wallet, through Stellar Wallets Kit (DEC-059, spec §18.3), checked
 * against @creit.tech/stellar-wallets-kit 2.7.0's typings. The kit is loaded
 * on first use, so its wallet libraries stay out of the main bundle.
 *
 * Any Stellar wallet the kit supports can deposit, claim and escape: those are
 * Stellar transactions. Trading also needs a SEP-53 message signature (the
 * owner signs ADD_SESSION_KEY with it, scheme 1). Wallets return it in
 * different encodings, and some can't sign messages, so every message
 * signature is checked here against the SEP-53 hash before the app uses it.
 */
import * as ed from "@noble/ed25519";
import { StrKey } from "@stellar/stellar-sdk";

import { sep53Hash } from "../codec/tx";
import { config } from "../config";

export class WalletError extends Error {}

type Kit = typeof import("@creit.tech/stellar-wallets-kit").StellarWalletsKit;
type KitNetwork = import("@creit.tech/stellar-wallets-kit").Networks;
let kitPromise: Promise<Kit> | null = null;

/** The kit, loaded and initialised once, with the modules that need no extra setup. */
function kit(): Promise<Kit> {
  kitPromise ??= (async () => {
    const [{ StellarWalletsKit, SwkAppDarkTheme }, { defaultModules }, { GhostsigModule, GHOSTSIG_ID }] = await Promise.all([
      import("@creit.tech/stellar-wallets-kit"),
      import("@creit.tech/stellar-wallets-kit/modules/utils"),
      import("@creit.tech/stellar-wallets-kit/modules/ghostsig"),
    ]);
    StellarWalletsKit.init({
      // GHOSTSIG's module starts on Stellar's public network unless it is
      // given one, and `defaultModules()` gives it none: connecting then asks
      // for an account there, which `assertNetwork` refuses. So it gets the lane's.
      modules: [
        ...defaultModules({ filterBy: (m) => m.productId !== GHOSTSIG_ID }),
        new GhostsigModule({ network: config.networkPassphrase }),
      ],
      network: config.networkPassphrase as KitNetwork,
      theme: SwkAppDarkTheme,
      authModal: { showInstallLabel: true },
    });
    return StellarWalletsKit;
  })();
  return kitPromise;
}

/** The kit rejects with `{code, message}` objects; make them errors people can read. */
function failure(e: unknown, what: string): WalletError {
  if (e instanceof WalletError) return e;
  const m = e && typeof e === "object" && "message" in e ? String((e as { message: unknown }).message) : String(e);
  return new WalletError(`${what}: ${m || "refused"}`);
}

/** Opens the wallet picker and returns the chosen account, on the lane's network. */
export async function connect(): Promise<string> {
  const k = await kit();
  let address: string;
  try {
    ({ address } = await k.authModal());
  } catch (e) {
    throw failure(e, "Connect wallet");
  }
  await assertNetwork();
  return address;
}

/** The account the kit remembers from an earlier visit, if any. */
export async function current(): Promise<string | null> {
  try {
    const { address } = await (await kit()).getAddress();
    return address || null;
  } catch {
    return null;
  }
}

export async function disconnect(): Promise<void> {
  try {
    await (await kit()).disconnect();
  } catch {
    /* nothing to forget */
  }
}

/** Refuses a wallet on another network. Wallets that can't say are trusted with the passphrase we send. */
export async function assertNetwork(): Promise<void> {
  let n: { network: string; networkPassphrase: string };
  try {
    n = await (await kit()).getNetwork();
  } catch {
    return;
  }
  if (n.networkPassphrase && n.networkPassphrase !== config.networkPassphrase) {
    throw new WalletError(`Your wallet is on ${n.network || "another network"}. Switch it to ${config.networkName} and try again.`);
  }
}

/** A 64-byte signature from the encodings wallets use: base64 or hex. */
export function decodeSignature(s: string): Uint8Array | null {
  const t = s.trim();
  if (/^[0-9a-fA-F]{128}$/.test(t)) return Uint8Array.from(t.match(/../g)!, (h) => parseInt(h, 16));
  try {
    const bytes = Uint8Array.from(atob(t), (c) => c.charCodeAt(0));
    return bytes.length === 64 ? bytes : null;
  } catch {
    return null;
  }
}

export const NOT_SEP53 =
  "This wallet can't sign SEP-53 messages, which trading needs (it signs your trading key). Deposits, claims and escape still work. Connect a wallet that supports SEP-53 message signing, such as Freighter or xBull.";

/** Whether `signature` is `address`'s ed25519 signature over the SEP-53 hash of `message`. */
export async function isSep53Signature(message: string, address: string, signature: Uint8Array): Promise<boolean> {
  try {
    return await ed.verifyAsync(signature, sep53Hash(message), StrKey.decodeEd25519PublicKey(address));
  } catch {
    return false;
  }
}

/** Signs `message` per SEP-53 and checks the signature before returning it. */
export async function signSep53(message: string, address: string): Promise<Uint8Array> {
  let signed: string;
  try {
    ({ signedMessage: signed } = await (await kit()).signMessage(message, { networkPassphrase: config.networkPassphrase, address }));
  } catch (e) {
    const err = failure(e, "Sign message");
    if (/does not support|not support|unsupported/i.test(err.message)) throw new WalletError(NOT_SEP53);
    throw err;
  }
  const sig = typeof signed === "string" ? decodeSignature(signed) : null;
  if (!sig || !(await isSep53Signature(message, address, sig))) throw new WalletError(NOT_SEP53);
  return sig;
}

export async function signTx(xdr: string, address: string): Promise<string> {
  await assertNetwork();
  try {
    return (await (await kit()).signTransaction(xdr, { networkPassphrase: config.networkPassphrase, address })).signedTxXdr;
  } catch (e) {
    throw failure(e, "Sign transaction");
  }
}
