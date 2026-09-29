import { describe, expect, it } from "vitest";
import { Networks } from "@stellar/stellar-sdk";
import { TESTNET_PASSPHRASE, assertTestnet } from "./network.js";

describe("assertTestnet", () => {
  it("matches the SDK's testnet passphrase", () => {
    expect(TESTNET_PASSPHRASE).toBe(Networks.TESTNET);
    expect(() => assertTestnet(Networks.TESTNET)).not.toThrow();
  });

  it("refuses mainnet", () => {
    expect(() => assertTestnet(Networks.PUBLIC)).toThrow(/testnet only/);
  });
});
