import { Keypair } from "@stellar/stellar-sdk";
import { describe, expect, it } from "vitest";

import { oracleSigningPreimage, sha256, fromHex } from "./codec.js";
import { createFeed, type FeedHost } from "./index.js";
import { OracleFeeder } from "./oracle.js";
import { FixedPrice, type PriceSource, parseDecimal } from "./prices.js";

const LANE_ID = fromHex("11".repeat(32));
const market = { market_id: 1, symbol: "BTC-PERP", tick: "1000", display_lot_base_units: 10_000, display_base_decimals: 8 };
const oracle = Keypair.fromRawEd25519Seed(Buffer.alloc(32, 0x31));

/** The relayer side, as the platform's FeedHost gives it. */
class Host implements FeedHost {
  laneId = LANE_ID;
  rpcUrl = "http://127.0.0.1:8000/rpc";
  networkPassphrase = "Standalone Network ; February 2017";
  posted: { route: string; updateHex: string }[] = [];
  markets = [market, { ...market, market_id: 2, symbol: "ETH-PERP", display_lot_base_units: 100_000 }];
  async getJson(path: string): Promise<unknown> {
    if (path !== "/v1/markets") throw new Error(`unexpected ${path}`);
    return this.markets;
  }
  async postFeed(route: string, updateHex: string): Promise<void> {
    this.posted.push({ route, updateHex });
  }
  log(): void {}
}

describe("oracle feeder (spec §17.3)", () => {
  it("signs updates the engine accepts and skips small moves inside 10 s", async () => {
    const posted: string[] = [];
    let now = 1_790_000_000_000;
    let price = "65000";
    const src: PriceSource = { name: "test", quote: async () => ({ usd: parseDecimal(price), observedAt: Math.floor(now / 1000), source: "test" }) };
    const feeder = new OracleFeeder(async (h) => void posted.push(h), oracle, LANE_ID, [{ info: market, sources: [src] }], 900, () => now);
    expect((await feeder.tick()).published).toHaveLength(1);
    const u = fromHex(posted[0]!);
    expect(u.length).toBe(114);
    // The signature verifies over H(TAG_ORACLE || lane_id || signed fields).
    const fields = { marketId: 1, price: 65_000_000n, publishTimeMs: BigInt(now) };
    expect(Keypair.fromPublicKey(oracle.publicKey()).verify(Buffer.from(sha256(oracleSigningPreimage(LANE_ID, fields))), Buffer.from(u.slice(50)))).toBe(true);
    now += 2_000;
    price = "65000.0004"; // +0.4 stroops per lot: less than a tick
    expect((await feeder.tick()).published).toHaveLength(0);
    now += 2_000;
    price = "65001"; // +1,000 stroops per lot: one tick
    expect((await feeder.tick()).published[0]?.price).toBe(65_001_000n);
    now += 10_000;
    expect((await feeder.tick()).published).toHaveLength(1); // heartbeat
  });

  it("reports a market without a fresh price and keeps the others going", async () => {
    const dead: PriceSource = { name: "dead", quote: async () => Promise.reject(new Error("down")) };
    const eth = { ...market, market_id: 2, display_lot_base_units: 100_000 };
    const feeder = new OracleFeeder(async () => {}, oracle, LANE_ID, [{ info: market, sources: [dead] }, { info: eth, sources: [new FixedPrice("3500")] }], 900);
    const r = await feeder.tick();
    expect(r.published.map((p) => [p.marketId, p.price])).toEqual([[2, 35_000_000n]]);
    expect(r.errors[0]).toMatch(/market 1: .*down/);
  });
});

describe("the feed module (DEC-053)", () => {
  it("feeds the configured markets to the perps node's oracle route", async () => {
    process.env.TEST_ORACLE_SECRET = oracle.secret();
    const host = new Host();
    const feed = await createFeed(host, { maxSourceAgeSecs: 900, keyEnv: "TEST_ORACLE_SECRET", markets: { "1": [{ fixed: "65000" }] } });
    expect(feed.describe()).toEqual({ oracle_key: oracle.publicKey(), markets: [{ id: 1, sources: ["fixed:65000"] }] });
    expect(await feed.tick()).toEqual({ errors: [] });
    expect(host.posted.map((p) => p.route)).toEqual(["oracle"]);
    expect(fromHex(host.posted[0]!.updateHex).length).toBe(114);
  });

  it("needs its key from the environment and valid options", async () => {
    delete process.env.NO_SUCH_KEY;
    await expect(createFeed(new Host(), { maxSourceAgeSecs: 900, keyEnv: "NO_SUCH_KEY", markets: {} })).rejects.toThrow(/set NO_SUCH_KEY/);
    await expect(createFeed(new Host(), { markets: {} })).rejects.toThrow(/maxSourceAgeSecs/);
    process.env.TEST_ORACLE_SECRET = oracle.secret();
    await expect(createFeed(new Host(), { maxSourceAgeSecs: 900, keyEnv: "TEST_ORACLE_SECRET", markets: { "1": [{ reflector: "BTC" }] } })).rejects.toThrow(/reflectorContract/);
  });
});
