import { describe, expect, it } from "vitest";
import { Networks } from "@stellar/stellar-sdk";
import { LOCAL_PASSPHRASE, TESTNET_PASSPHRASE, assertTestnet } from "./network.js";

describe("assertTestnet", () => {
  it("matches the SDK's testnet and standalone passphrases", () => {
    expect(TESTNET_PASSPHRASE).toBe(Networks.TESTNET);
    expect(LOCAL_PASSPHRASE).toBe(Networks.STANDALONE);
    expect(() => assertTestnet(Networks.TESTNET)).not.toThrow();
    expect(() => assertTestnet(Networks.STANDALONE)).not.toThrow();
  });

  it("refuses mainnet", () => {
    expect(() => assertTestnet(Networks.PUBLIC)).toThrow(/testnet only/);
  });
});
