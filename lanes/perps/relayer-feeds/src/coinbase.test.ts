import { Keypair } from "@stellar/stellar-sdk";
import { describe, expect, it } from "vitest";

import { COINBASE_WS, CoinbaseStream, MAX_TRADE_AGE_MS, SILENCE_MS, type Clock, type SocketLike } from "./coinbase.js";
import { fromHex } from "./codec.js";
import { createFeed, type FeedHost } from "./index.js";
import { firstFresh, pricePerLot, snapToTick } from "./prices.js";

class FakeSocket implements SocketLike {
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  sent: unknown[] = [];
  closed = false;
  constructor(readonly url: string) {}
  send(data: string): void {
    this.sent.push(JSON.parse(data));
  }
  close(): void {
    this.closed = true;
  }
  msg(m: unknown): void {
    this.onmessage?.({ data: JSON.stringify(m) });
  }
}

/** Timers run only when the test advances time. */
class FakeClock implements Clock {
  t = 1_790_000_000_000;
  private timers: { at: number; fn: () => void; id: number }[] = [];
  private next = 0;
  now(): number {
    return this.t;
  }
  setTimeout(fn: () => void, ms: number): unknown {
    const id = ++this.next;
    this.timers.push({ at: this.t + ms, fn, id });
    return id;
  }
  clearTimeout(h: unknown): void {
    this.timers = this.timers.filter((x) => x.id !== h);
  }
  advance(ms: number): void {
    const end = this.t + ms;
    for (;;) {
      this.timers.sort((a, b) => a.at - b.at);
      const due = this.timers[0];
      if (!due || due.at > end) break;
      this.timers.shift();
      this.t = due.at;
      due.fn();
    }
    this.t = end;
  }
}

let tradeId = 1_000;
const tick = (product: string, price: string, timeMs: number, id = ++tradeId) => ({
  type: "ticker",
  sequence: 1,
  product_id: product,
  price,
  trade_id: id,
  time: new Date(timeMs).toISOString().replace("Z", "123Z"),
});
const heartbeat = (product: string, lastTradeId: number, timeMs: number) => ({
  type: "heartbeat",
  last_trade_id: lastTradeId,
  product_id: product,
  sequence: 2,
  time: new Date(timeMs).toISOString().replace("Z", "000Z"),
});

function setup(products = ["BTC-USD", "ETH-USD"]) {
  const clock = new FakeClock();
  const sockets: FakeSocket[] = [];
  const stream = new CoinbaseStream(products, (url) => {
    const s = new FakeSocket(url);
    sockets.push(s);
    return s;
  }, clock);
  stream.start();
  return { clock, sockets, stream };
}

describe("Coinbase ticker stream (DEC-058)", () => {
  it("subscribes to the ticker channel for every product on one connection", () => {
    const { sockets } = setup();
    expect(sockets).toHaveLength(1);
    expect(sockets[0]!.url).toBe(COINBASE_WS);
    sockets[0]!.onopen?.({});
    expect(sockets[0]!.sent).toEqual([{ type: "subscribe", product_ids: ["BTC-USD", "ETH-USD"], channels: ["ticker", "heartbeat"] }]);
  });

  it("keeps the latest trade per product and ignores everything else", async () => {
    const { clock, sockets, stream } = setup();
    const s = sockets[0]!;
    const btc = stream.source("BTC-USD");
    await expect(btc.quote()).rejects.toThrow(/no trade received yet/);
    s.msg({ type: "subscriptions", channels: [] });
    s.msg(tick("BTC-USD", "83857.7", clock.t - 2_000));
    s.msg(tick("SOL-USD", "150", clock.t));
    s.msg({ type: "heartbeat", product_id: "BTC-USD" });
    s.msg({ type: "ticker", product_id: "BTC-USD", price: "not a price", time: new Date(clock.t).toISOString() });
    s.onmessage?.({ data: "{not json" });
    s.msg(tick("BTC-USD", "83860.25", clock.t - 1_000));
    const q = await btc.quote();
    expect(q.usd).toEqual({ num: 8386025n, den: 100n });
    expect(q.observedAt).toBe(Math.floor((clock.t - 1_000) / 1000));
    expect(q.source).toBe("coinbase-ws:BTC-USD");
    await expect(stream.source("ETH-USD").quote()).rejects.toThrow(/no trade/);
    expect(() => stream.source("XLM-USD")).toThrow(/not subscribed/);
  });

  it("is used while fresh, then falls through to the next source", async () => {
    const { clock, sockets, stream } = setup();
    sockets[0]!.msg(tick("BTC-USD", "65000", clock.t));
    const fallback = { name: "spot", quote: async () => ({ usd: { num: 64000n, den: 1n }, observedAt: Math.floor(clock.t / 1000), source: "spot" }) };
    const sources = [stream.source("BTC-USD", 5), fallback];
    expect((await firstFresh(sources, 900, Math.floor(clock.t / 1000))).source).toBe("coinbase-ws:BTC-USD");
    // Six seconds without a trade: older than the stream's own 5 s, even though the feed allows 900.
    expect((await firstFresh(sources, 900, Math.floor(clock.t / 1000) + 6)).source).toBe("spot");
    // The lane price of the streamed quote, as the feeder computes it (BTC-PERP: lot 10,000, 8 decimals, tick 1,000).
    const q = await stream.source("BTC-USD").quote();
    expect(snapToTick(pricePerLot(q.usd, 10_000n, 8), 1_000n)).toBe(65_000_000n);
  });

  it("a heartbeat naming the last trade keeps a quiet market fresh, within limits", async () => {
    const { clock, sockets, stream } = setup(["XLM-USD"]);
    const s = sockets[0]!;
    const xlm = stream.source("XLM-USD", 5);
    const t0 = clock.t;
    s.msg(tick("XLM-USD", "0.2283", t0, 500));
    // 20 s without a trade, but heartbeats confirm trade 500 is still the last.
    s.msg(heartbeat("XLM-USD", 500, t0 + 20_000));
    expect((await xlm.quote()).observedAt).toBe(Math.floor((t0 + 20_000) / 1000));
    expect((await firstFresh([xlm], 900, Math.floor((t0 + 22_000) / 1000))).usd).toEqual({ num: 2283n, den: 10_000n });
    // A heartbeat naming a newer trade means we missed one: no refresh.
    s.msg(heartbeat("XLM-USD", 501, t0 + 40_000));
    expect((await xlm.quote()).observedAt).toBe(Math.floor((t0 + 20_000) / 1000));
    // A heartbeat for an old trade stops refreshing it after MAX_TRADE_AGE_MS.
    s.msg(tick("XLM-USD", "0.2290", t0 + 50_000, 502));
    s.msg(heartbeat("XLM-USD", 502, t0 + 50_000 + MAX_TRADE_AGE_MS + 1_000));
    expect((await xlm.quote()).observedAt).toBe(Math.floor((t0 + 50_000) / 1000));
    // Heartbeats never make a quote look older, or touch other products.
    s.msg(heartbeat("BTC-USD", 1, t0 + 60_000));
    expect((await xlm.quote()).usd).toEqual({ num: 2290n, den: 10_000n });
  });

  it("reconnects with backoff when the connection closes", () => {
    const { clock, sockets } = setup();
    sockets[0]!.onclose?.({});
    expect(sockets).toHaveLength(1);
    clock.advance(1_000);
    expect(sockets).toHaveLength(2);
    sockets[1]!.onclose?.({});
    clock.advance(1_000);
    expect(sockets).toHaveLength(2); // the second wait is 2 s
    clock.advance(1_000);
    expect(sockets).toHaveLength(3);
    // A message resets the backoff.
    sockets[2]!.msg(tick("BTC-USD", "1", clock.t));
    sockets[2]!.onclose?.({});
    clock.advance(1_000);
    expect(sockets).toHaveLength(4);
  });

  it("reconnects after the stream goes silent, and stops for good on stop()", () => {
    const { clock, sockets, stream } = setup();
    sockets[0]!.msg(tick("BTC-USD", "1", clock.t));
    clock.advance(SILENCE_MS - 1_000);
    expect(sockets[0]!.closed).toBe(false);
    clock.advance(SILENCE_MS);
    expect(sockets[0]!.closed).toBe(true);
    clock.advance(1_000);
    expect(sockets).toHaveLength(2);
    stream.stop();
    expect(sockets[1]!.closed).toBe(true);
    sockets[1]!.onclose?.({});
    clock.advance(60_000);
    expect(sockets).toHaveLength(2);
  });
});

describe("the feed module with the stream", () => {
  const market = (id: number, symbol: string, lot: number) => ({ market_id: id, symbol, tick: "1000", display_lot_base_units: lot, display_base_decimals: 8 });
  const host = (posted: string[]): FeedHost => ({
    laneId: fromHex("11".repeat(32)),
    rpcUrl: "http://127.0.0.1:8000/rpc",
    networkPassphrase: "Standalone Network ; February 2017",
    getJson: async () => [market(1, "BTC-PERP", 10_000), market(2, "ETH-PERP", 100_000)],
    postFeed: async (_route, hex) => void posted.push(hex),
    log: () => {},
  });

  it("opens one stream for all markets and publishes streamed prices", async () => {
    process.env.TEST_ORACLE_SECRET = Keypair.fromRawEd25519Seed(Buffer.alloc(32, 0x31)).secret();
    const sockets: FakeSocket[] = [];
    const posted: string[] = [];
    const feed = await createFeed(
      host(posted),
      {
        maxSourceAgeSecs: 900,
        keyEnv: "TEST_ORACLE_SECRET",
        markets: { "1": [{ coinbaseStream: "BTC-USD" }, { fixed: "1" }], "2": [{ coinbaseStream: "ETH-USD" }, { fixed: "1" }] },
      },
      { connect: (url) => (sockets.push(new FakeSocket(url)), sockets.at(-1)!) },
    );
    expect(sockets).toHaveLength(1);
    expect(feed.describe().coinbase_stream).toEqual(["BTC-USD", "ETH-USD"]);
    // No trade yet: the fixed fallback publishes.
    expect(await feed.tick()).toEqual({ errors: [] });
    expect(posted).toHaveLength(2);
    sockets[0]!.msg(tick("BTC-USD", "65001", Date.now()));
    sockets[0]!.msg(tick("ETH-USD", "3500", Date.now()));
    await feed.tick();
    expect(posted).toHaveLength(4);
    // BTC at 65,001 USD is 65,001,000 stroops per lot (little-endian i64 after the u16 market id).
    const u = fromHex(posted[2]!);
    expect(new DataView(u.buffer).getBigInt64(2, true)).toBe(65_001_000n);
  });
});
