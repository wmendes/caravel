import { readFileSync } from "node:fs";
import { join } from "node:path";

import * as ed from "@noble/ed25519";
import { StrKey } from "@stellar/stellar-sdk";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { fromHex } from "../codec/bytes";
import { sep53Hash } from "../codec/tx";

// The kit, as a wallet module would drive it.
const kitState: { signMessage: (m: string) => Promise<{ signedMessage: string }>; network: string; address: string | null } = {
  signMessage: async () => ({ signedMessage: "" }),
  network: "Test SDF Network ; September 2015",
  address: null,
};
vi.mock("@creit.tech/stellar-wallets-kit/modules/utils", () => ({ defaultModules: () => [] }));
vi.mock("@creit.tech/stellar-wallets-kit", () => ({
  SwkAppDarkTheme: {},
  StellarWalletsKit: {
    init: () => {},
    authModal: async () => ({ address: kitState.address }),
    getAddress: async () => {
      if (!kitState.address) throw { code: -1, message: "No wallet has been connected." };
      return { address: kitState.address };
    },
    getNetwork: async () => ({ network: "testnet", networkPassphrase: kitState.network }),
    signMessage: (m: string) => kitState.signMessage(m),
    signTransaction: async (xdr: string) => ({ signedTxXdr: `signed:${xdr}` }),
    disconnect: async () => {},
  },
}));

const wallet = await import("./wallet");

type V = { name: string; fields: Record<string, string> };
const laneTx = JSON.parse(readFileSync(join(import.meta.dirname, "../../../engine/test-vectors/lane_tx.json"), "utf8")) as { vectors: V[] };
// ADD_SESSION_KEY signed by the owner with SEP-53 (scheme 1): the signature the engine accepts.
const owner = laneTx.vectors.find((v) => v.name === "add_session_key_sep53_owner")!.fields;
const ownerG = StrKey.encodeEd25519PublicKey(Buffer.from(owner.signer!, "hex"));
const ownerSig = fromHex(owner.signature!);
const b64 = (b: Uint8Array) => btoa(String.fromCharCode(...b));

describe("the SEP-53 check (DEC-059)", () => {
  it("accepts the frozen vector's owner signature and nothing else", async () => {
    expect(await wallet.isSep53Signature(owner.sep53_message!, ownerG, ownerSig)).toBe(true);
    const bad = ownerSig.slice();
    bad[0]! ^= 1;
    expect(await wallet.isSep53Signature(owner.sep53_message!, ownerG, bad)).toBe(false);
    expect(await wallet.isSep53Signature(`${owner.sep53_message!} `, ownerG, ownerSig)).toBe(false);
    const other = StrKey.encodeEd25519PublicKey(Buffer.alloc(32, 7));
    expect(await wallet.isSep53Signature(owner.sep53_message!, other, ownerSig)).toBe(false);
  });

  it("reads base64 and hex signatures, and nothing that is not 64 bytes", () => {
    expect(wallet.decodeSignature(b64(ownerSig))).toEqual(ownerSig);
    expect(wallet.decodeSignature(owner.signature!)).toEqual(ownerSig);
    expect(wallet.decodeSignature(owner.signature!.toUpperCase())).toEqual(ownerSig);
    expect(wallet.decodeSignature(b64(ownerSig.slice(0, 63)))).toBeNull();
    expect(wallet.decodeSignature("not a signature!")).toBeNull();
  });
});

describe("the wallet layer over Stellar Wallets Kit", () => {
  const seed = new Uint8Array(32).fill(0x42);
  let address: string;
  beforeEach(async () => {
    address = StrKey.encodeEd25519PublicKey(Buffer.from(await ed.getPublicKeyAsync(seed)));
    kitState.address = address;
    kitState.network = "Test SDF Network ; September 2015";
  });

  it("connects through the picker and remembers the account", async () => {
    expect(await wallet.connect()).toBe(address);
    expect(await wallet.current()).toBe(address);
    kitState.address = null;
    expect(await wallet.current()).toBeNull();
  });

  it("refuses a wallet on another network", async () => {
    kitState.network = "Standalone Network ; February 2017";
    await expect(wallet.connect()).rejects.toThrow(/Switch it to/);
    await expect(wallet.signTx("AAAA", address)).rejects.toThrow(/Switch it to/);
  });

  it("returns a SEP-53 signature a wallet gives in base64 or hex", async () => {
    const message = "Caravel lane tx 00";
    const sig = await ed.signAsync(sep53Hash(message), seed);
    kitState.signMessage = async () => ({ signedMessage: b64(sig) });
    expect(await wallet.signSep53(message, address)).toEqual(sig);
    kitState.signMessage = async () => ({ signedMessage: Buffer.from(sig).toString("hex") });
    expect(await wallet.signSep53(message, address)).toEqual(sig);
  });

  it("refuses a wallet whose message signature is not SEP-53, or that cannot sign messages", async () => {
    const message = "Caravel lane tx 01";
    // Signs the raw message instead of the SEP-53 hash.
    const raw = await ed.signAsync(new TextEncoder().encode(message), seed);
    kitState.signMessage = async () => ({ signedMessage: b64(raw) });
    await expect(wallet.signSep53(message, address)).rejects.toThrow(wallet.NOT_SEP53);
    kitState.signMessage = async () => Promise.reject({ code: -3, message: 'Albedo does not support the "signMessage" function' });
    await expect(wallet.signSep53(message, address)).rejects.toThrow(wallet.NOT_SEP53);
    kitState.signMessage = async () => Promise.reject({ code: -4, message: "User declined" });
    await expect(wallet.signSep53(message, address)).rejects.toThrow("Sign message: User declined");
  });

  it("signs transactions with the lane's passphrase", async () => {
    expect(await wallet.signTx("AAAA", address)).toBe("signed:AAAA");
  });
});
