import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { loadConfig } from "./config.js";
import { FakeSequencer } from "./fakes.js";
import { loadFeedModule, withDeadline, type FeedHost } from "./feeds.js";
import { RPC_TIMEOUT_MS, RpcContract } from "./stellar.js";

const dir = mkdtempSync(join(tmpdir(), "caravel-feeds-"));

// A feed module as an app ships one: an ES module with createFeed.
writeFileSync(
  join(dir, "echo.mjs"),
  `export async function createFeed(host, options) {
     const markets = await host.getJson("/v1/markets");
     return {
       name: "echo",
       describe: () => ({ route: options.route, markets: markets.length }),
       tick: async () => { await host.postFeed(options.route, "ab".repeat(4)); return { errors: [] }; },
     };
   }`,
);
writeFileSync(join(dir, "not-a-feed.mjs"), "export const x = 1;");
// A module whose tick never settles, as an RPC call without a timeout does.
writeFileSync(
  join(dir, "stuck.mjs"),
  `export async function createFeed() {
     return { name: "stuck", describe: () => ({}), tick: () => new Promise(() => {}) };
   }`,
);

const host = (seq: FakeSequencer): FeedHost => ({
  laneId: new Uint8Array(32),
  rpcUrl: "http://127.0.0.1:8000/rpc",
  networkPassphrase: "Standalone Network ; February 2017",
  getJson: (p) => seq.getJson(p),
  postFeed: (r, h) => seq.postFeed(r, h),
  log: () => {},
});

describe("feed modules (DEC-053)", () => {
  it("loads a module relative to the config and runs its ticks", async () => {
    const seq = new FakeSequencer();
    seq.public_["/v1/markets"] = [{}, {}, {}];
    const feed = await (await loadFeedModule({ module: "./echo.mjs" }, dir)).createFeed(host(seq), { route: "oracle" });
    expect(feed.describe()).toEqual({ route: "oracle", markets: 3 });
    expect(await feed.tick()).toEqual({ errors: [] });
    expect(seq.feeds).toEqual([{ route: "oracle", updateHex: "abababab" }]);
  });

  it("refuses a file that is not a feed module", async () => {
    await expect(loadFeedModule({ module: join(dir, "not-a-feed.mjs") }, "/")).rejects.toThrow(/no createFeed/);
    await expect(loadFeedModule({ module: "" }, dir)).rejects.toThrow(/must name/);
  });

  it("refuses an M0 config with the oracle loop in the relayer", async () => {
    process.env.CARAVEL_INTERNAL_TOKEN = "t".repeat(32);
    const base = {
      rpcUrl: "http://127.0.0.1:8000/rpc",
      networkPassphrase: "Standalone Network ; February 2017",
      settlementContract: "CADQOBYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQP5KR",
      sequencerUrl: "http://127.0.0.1:8080",
      metricsFile: "m.jsonl",
    };
    const m0 = join(dir, "m0.json");
    writeFileSync(m0, JSON.stringify({ ...base, loops: { inbox: true, checkpoints: false, oracle: true }, oracle: { markets: {} } }));
    await expect(loadConfig(m0)).rejects.toThrow(/moved to a feed module/);
    const ok = join(dir, "ok.json");
    writeFileSync(ok, JSON.stringify({ ...base, loops: { inbox: true, checkpoints: false }, feeds: [{ module: "./echo.mjs", options: { route: "oracle" } }] }));
    const cfg = await loadConfig(ok);
    expect(cfg.dir).toBe(dir);
    expect(cfg.file.feeds?.[0]?.module).toBe("./echo.mjs");
  });

  it("abandons a tick that never settles, so the loop goes on", async () => {
    const seq = new FakeSequencer();
    const feed = await (await loadFeedModule({ module: "./stuck.mjs" }, dir)).createFeed(host(seq), {});
    await expect(withDeadline(feed.tick(), 50, "stuck tick")).rejects.toThrow("stuck tick: no answer in 50 ms");
    await expect(withDeadline(Promise.resolve(7), 50, "quick")).resolves.toBe(7);
    await expect(withDeadline(Promise.reject(new Error("boom")), 50, "failing")).rejects.toThrow("boom");
  });

  it("refuses a feed deadline that is not a positive integer", async () => {
    process.env.CARAVEL_INTERNAL_TOKEN = "t".repeat(32);
    const bad = join(dir, "bad-deadline.json");
    writeFileSync(
      bad,
      JSON.stringify({
        rpcUrl: "http://127.0.0.1:8000/rpc",
        networkPassphrase: "Standalone Network ; February 2017",
        settlementContract: "CADQOBYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQP5KR",
        sequencerUrl: "http://127.0.0.1:8080",
        metricsFile: "m.jsonl",
        loops: { inbox: true, checkpoints: false },
        feeds: [{ module: "./echo.mjs", deadlineMs: 0 }],
      }),
    );
    await expect(loadConfig(bad)).rejects.toThrow(/deadlineMs/);
  });
});

describe("Stellar RPC client", () => {
  it("bounds every call with a timeout (stellar-sdk 17.2.0 waits forever by default)", () => {
    const c = new RpcContract("http://127.0.0.1:8000/rpc", "CADQOBYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQP5KR", "Standalone Network ; February 2017");
    expect(c.server.httpClient.defaults.timeout).toBe(RPC_TIMEOUT_MS);
  });
});
