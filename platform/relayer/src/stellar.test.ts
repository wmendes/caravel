import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { xdr } from "@stellar/stellar-sdk";
import { describe, expect, it } from "vitest";

import { cursorLedger, feeBreakdown } from "./stellar.js";

describe("feeBreakdown (K-06)", () => {
  it("reads the resource fees from a checkpoint's metadata", () => {
    // Lane #1's checkpoint 7,747 on testnet (2026-10-05), as getTransaction returned it.
    const meta = readFileSync(fileURLToPath(new URL("../test-fixtures/checkpoint-7747.meta.b64", import.meta.url)), "utf8").trim();
    expect(feeBreakdown(xdr.TransactionMeta.fromXDR(meta, "base64"))).toEqual({ rentFee: 3326064n, refundableFee: 3327236n, nonRefundableFee: 104427n });
  });

  it("is empty without Soroban metadata", () => {
    expect(feeBreakdown(undefined)).toEqual({});
  });
});

describe("cursorLedger", () => {
  it("reads the ledger out of an events cursor", () => {
    // RPC's cursor after one 10,000-ledger scan from 4,911,468 (testnet, 2026-10-05).
    expect(cursorLedger("0021137544108310527-4294967295")).toBe(4921467);
  });
});
