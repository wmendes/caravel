/**
 * Coinbase's public market data stream (DEC-058): the `ticker` channel on
 * `wss://ws-feed.exchange.coinbase.com`. It needs no API key and sends a
 * message on every trade, several a second for BTC-USD (checked 2026-09-30).
 *
 * One connection serves every configured product, on the `ticker` and
 * `heartbeat` channels. The stream keeps the latest trade price per product.
 * `source(product)` exposes it as a `PriceSource`. A quote is as old as its
 * trade, or as the latest heartbeat that names that trade as the product's
 * last (`last_trade_id`), so a quiet market such as XLM-USD stays usable
 * between trades. A heartbeat refreshes a trade for up to `MAX_TRADE_AGE_MS`.
 * A missed trade, a stale trade or a dropped stream falls through to the next
 * source. The connection reconnects with backoff, and after `SILENCE_MS`
 * without a message.
 */
import { parseDecimal, type PriceSource, type Quote } from "./prices.js";

export const COINBASE_WS = "wss://ws-feed.exchange.coinbase.com";
/** No message for this long means the connection is dead: reconnect. */
export const SILENCE_MS = 30_000;
/** A heartbeat keeps a trade's price current for at most this long. */
export const MAX_TRADE_AGE_MS = 120_000;
const MAX_BACKOFF_MS = 30_000;

/** The part of a WebSocket the stream uses (the Node 22 global has it). */
export interface SocketLike {
  onopen: ((ev: unknown) => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
  onclose: ((ev: unknown) => void) | null;
  onerror: ((ev: unknown) => void) | null;
  send(data: string): void;
  close(): void;
}

export type Connect = (url: string) => SocketLike;

export interface Clock {
  now(): number;
  setTimeout(fn: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

const realClock: Clock = {
  now: () => Date.now(),
  setTimeout: (fn, ms) => setTimeout(fn, ms),
  clearTimeout: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
};

const realConnect: Connect = (url) => new WebSocket(url) as unknown as SocketLike;

interface Last {
  quote: Quote;
  tradeId: number | null;
  tradeAtMs: number;
}

export class CoinbaseStream {
  private readonly latest = new Map<string, Last>();
  private socket: SocketLike | null = null;
  private lastMessageMs = 0;
  private backoffMs = 1_000;
  private stopped = true;
  private timer: unknown = null;
  connections = 0;

  constructor(
    private readonly products: string[],
    private readonly connect: Connect = realConnect,
    private readonly clock: Clock = realClock,
    private readonly log: (msg: string) => void = () => {},
  ) {}

  start(): void {
    if (!this.stopped) return;
    this.stopped = false;
    this.open();
  }

  stop(): void {
    this.stopped = true;
    if (this.timer !== null) this.clock.clearTimeout(this.timer);
    this.timer = null;
    this.socket?.close();
    this.socket = null;
  }

  /** The latest price for `product` as a price source. */
  source(product: string, maxAgeSecs = 5): PriceSource {
    if (!this.products.includes(product)) throw new Error(`coinbase stream: ${product} is not subscribed`);
    return {
      name: `coinbase-ws:${product}`,
      maxAgeSecs,
      quote: async () => {
        const last = this.latest.get(product);
        if (!last) throw new Error(`coinbase-ws:${product}: no trade received yet`);
        return last.quote;
      },
    };
  }

  status(): { connected: boolean; products: string[] } {
    return { connected: this.socket !== null, products: this.products };
  }

  private open(): void {
    const ws = this.connect(COINBASE_WS);
    this.socket = ws;
    this.connections += 1;
    this.lastMessageMs = this.clock.now();
    ws.onopen = () => {
      ws.send(JSON.stringify({ type: "subscribe", product_ids: this.products, channels: ["ticker", "heartbeat"] }));
    };
    ws.onmessage = (ev) => {
      this.lastMessageMs = this.clock.now();
      this.backoffMs = 1_000;
      this.onMessage(ev.data);
    };
    ws.onerror = () => {};
    ws.onclose = () => {
      if (this.socket !== ws) return;
      this.socket = null;
      if (this.stopped) return;
      this.log(`coinbase stream closed; reconnecting in ${this.backoffMs} ms`);
      this.schedule(this.backoffMs, () => this.open());
      this.backoffMs = Math.min(this.backoffMs * 2, MAX_BACKOFF_MS);
    };
    this.schedule(SILENCE_MS / 3, () => this.watch(ws));
  }

  /** Closes a connection that has gone quiet; `onclose` reconnects. */
  private watch(ws: SocketLike): void {
    if (this.stopped || this.socket !== ws) return;
    if (this.clock.now() - this.lastMessageMs > SILENCE_MS) {
      this.log("coinbase stream silent; reconnecting");
      ws.close();
      ws.onclose?.({});
      return;
    }
    this.schedule(SILENCE_MS / 3, () => this.watch(ws));
  }

  private schedule(ms: number, fn: () => void): void {
    if (this.timer !== null) this.clock.clearTimeout(this.timer);
    this.timer = this.clock.setTimeout(() => {
      this.timer = null;
      fn();
    }, ms);
  }

  private onMessage(data: unknown): void {
    let m: { type?: unknown; product_id?: unknown; price?: unknown; time?: unknown; trade_id?: unknown; last_trade_id?: unknown; message?: unknown; reason?: unknown };
    try {
      m = JSON.parse(String(data)) as typeof m;
    } catch {
      return;
    }
    if (m.type === "error") {
      this.log(`coinbase stream error: ${String(m.message)} ${String(m.reason ?? "")}`);
      return;
    }
    if (typeof m.product_id !== "string" || !this.products.includes(m.product_id)) return;
    const at = typeof m.time === "string" ? Date.parse(m.time) : NaN;
    const atMs = Number.isNaN(at) ? this.clock.now() : at;
    if (m.type === "heartbeat") {
      const last = this.latest.get(m.product_id);
      if (last && last.tradeId !== null && m.last_trade_id === last.tradeId && atMs - last.tradeAtMs <= MAX_TRADE_AGE_MS) {
        last.quote = { ...last.quote, observedAt: Math.max(last.quote.observedAt, Math.floor(atMs / 1000)) };
      }
      return;
    }
    if (m.type !== "ticker" || typeof m.price !== "string") return;
    let usd;
    try {
      usd = parseDecimal(m.price);
    } catch {
      return;
    }
    this.latest.set(m.product_id, {
      quote: { usd, observedAt: Math.floor(atMs / 1000), source: `coinbase-ws:${m.product_id}` },
      tradeId: typeof m.trade_id === "number" ? m.trade_id : null,
      tradeAtMs: atMs,
    });
  }
}
