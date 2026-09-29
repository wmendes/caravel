import { describe, expect, it } from "vitest";

import { FixedPrice, firstFresh, parseDecimal, pricePerLot, roundHalfEven, snapToTick, type PriceSource } from "./prices.js";

// The testnet lane's markets (config/lane.caravel-perps.testnet.toml).
const BTC = { lot: 10_000n, decimals: 8, tick: 1_000n };
const ETH = { lot: 100_000n, decimals: 8, tick: 1_000n };
const XLM = { lot: 100_000_000n, decimals: 7, tick: 1_000n };

describe("price conversion (spec §17.3)", () => {
  it("matches the lane's reference prices", () => {
    expect(pricePerLot(parseDecimal("65000"), BTC.lot, BTC.decimals)).toBe(65_000_000n);
    expect(pricePerLot(parseDecimal("3500"), ETH.lot, ETH.decimals)).toBe(35_000_000n);
    expect(pricePerLot(parseDecimal("0.40"), XLM.lot, XLM.decimals)).toBe(40_000_000n);
  });

  it("rounds half to even", () => {
    expect(roundHalfEven(5n, 2n)).toBe(2n);
    expect(roundHalfEven(7n, 2n)).toBe(4n);
    expect(roundHalfEven(6n, 4n)).toBe(2n);
    expect(roundHalfEven(10n, 4n)).toBe(2n);
    expect(roundHalfEven(11n, 4n)).toBe(3n);
    // $83,574.9255 per BTC is 83,574,925.5 stroops per lot: to the even 83,574,926.
    expect(pricePerLot(parseDecimal("83574.9255"), BTC.lot, BTC.decimals)).toBe(83_574_926n);
  });

  it("snaps to the tick", () => {
    expect(snapToTick(83_574_925n, 1_000n)).toBe(83_575_000n);
    expect(snapToTick(83_574_499n, 1_000n)).toBe(83_574_000n);
    expect(snapToTick(2_500n, 1_000n)).toBe(2_000n);
    expect(snapToTick(3_500n, 1_000n)).toBe(4_000n);
    expect(snapToTick(10n, 1_000n)).toBe(1_000n);
  });

  it("reads Reflector's 14-decimal prices exactly", () => {
    // lastprice(Other("BTC")) on testnet, 2026-09-29: 8358052750414796421 / 10^14 USD.
    const usd = { num: 8_358_052_750_414_796_421n, den: 10n ** 14n };
    expect(pricePerLot(usd, BTC.lot, BTC.decimals)).toBe(83_580_528n);
  });

  it("parses only plain decimals", () => {
    expect(parseDecimal("0.2239365")).toEqual({ num: 2_239_365n, den: 10_000_000n });
    expect(() => parseDecimal("-1")).toThrow();
    expect(() => parseDecimal("1e5")).toThrow();
  });
});

describe("source priority", () => {
  const stale: PriceSource = { name: "stale", quote: async () => ({ usd: parseDecimal("1"), observedAt: 0, source: "stale" }) };
  const broken: PriceSource = { name: "broken", quote: async () => Promise.reject(new Error("down")) };

  it("falls through stale and failing sources", async () => {
    const q = await firstFresh([stale, broken, new FixedPrice("2")], 900, Math.floor(Date.now() / 1000));
    expect(q.source).toBe("fixed:2");
  });

  it("fails when nothing is fresh", async () => {
    await expect(firstFresh([stale, broken], 900, 10_000)).rejects.toThrow(/stale: 10000s old; Error: down/);
  });
});
