import { describe, expect, it } from "vitest";
import { applyConfig, config, loadConfig, type Config } from "./config";

const base = (): Config => ({ ...config, validatorUrls: [...config.validatorUrls] });

describe("the deployment's config (C-25)", () => {
  it("lays known keys over the defaults", () => {
    const c = base();
    const set = applyConfig(c, {
      sequencerUrl: "https://lane.example/",
      validatorUrls: ["https://lane.example/validators/1", "/validators/2"],
      settlementContract: "CABC",
      networkName: "local",
    });
    expect(set).toEqual(["sequencerUrl", "validatorUrls", "settlementContract", "networkName"]);
    expect(c.sequencerUrl).toBe("https://lane.example");
    expect(c.validatorUrls).toEqual(["https://lane.example/validators/1", "http://localhost/validators/2"]);
    expect(c.settlementContract).toBe("CABC");
    expect(c.networkName).toBe("local");
  });

  it("ignores unknown keys, wrong types and empty values", () => {
    const c = base();
    expect(applyConfig(c, { nope: "x", rpcUrl: 7, usdcContract: "", validatorUrls: "a,b" })).toEqual([]);
    expect(applyConfig(c, ["sequencerUrl"])).toEqual([]);
    expect(applyConfig(c, null)).toEqual([]);
    expect(c).toEqual(base());
  });

  it("keeps the defaults without a config.json", async () => {
    const before = base();
    const notFound = (async () => new Response("<html></html>", { status: 200, headers: { "content-type": "text/html" } })) as typeof fetch;
    await loadConfig(notFound);
    await loadConfig((async () => { throw new Error("offline"); }) as typeof fetch);
    expect(config).toEqual(before);
    const served = (async () =>
      new Response(JSON.stringify({ networkName: "e2e" }), { headers: { "content-type": "application/json" } })) as typeof fetch;
    await loadConfig(served);
    expect(config.networkName).toBe("e2e");
  });
});
