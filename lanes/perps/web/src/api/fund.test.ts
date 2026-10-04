import { afterEach, describe, expect, it, vi } from "vitest";

import { stellar } from "./stellar";

// The "Get test USDC" quote (DEC-106): the cheapest strict-receive path on
// the testnet DEX for exactly the USDC asked for.
afterEach(() => vi.unstubAllGlobals());

const records = (rs: { source_amount: string; path: unknown[] }[]) => ({ ok: true, status: 200, json: async () => ({ _embedded: { records: rs } }) });

describe("Get test USDC", () => {
  it("asks Horizon for exactly the USDC wanted and takes the cheapest path", async () => {
    const fetch = vi.fn(async (_url: string) => records([{ source_amount: "530.5000000", path: [] }, { source_amount: "528.9587640", path: [{ asset_type: "credit_alphanum4", asset_code: "EURC", asset_issuer: "GB" }] }]));
    vi.stubGlobal("fetch", fetch);
    const q = await stellar.quoteUsdc(500n * 10_000_000n);
    expect(q).toEqual({ usdc: "500.0000000", xlm: "528.9587640", path: [{ asset_type: "credit_alphanum4", asset_code: "EURC", asset_issuer: "GB" }] });
    const url = new URL(fetch.mock.calls[0]![0]);
    expect(url.pathname).toBe("/paths/strict-receive");
    expect(url.searchParams.get("destination_amount")).toBe("500.0000000");
    expect(url.searchParams.get("source_assets")).toBe("native");
    expect(url.searchParams.get("destination_asset_code")).toBe("USDC");
  });

  it("says plainly when the DEX has no liquidity", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => records([])));
    await expect(stellar.quoteUsdc(10_000n * 10_000_000n)).rejects.toThrow(/no XLM to USDC liquidity/);
  });

  it("reads a new account as unfunded", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => ({ ok: false, status: 404, json: async () => ({}) })));
    expect(await stellar.xlm("GB3FBG2TQKLJGNM2COPNERKEHCMMLOTXFJBK22IDDXHCYJ2EBWFBFYYN")).toBeNull();
  });
});
