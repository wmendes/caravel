/** The sequencer's and validators' public APIs (spec §14.4, §15, DEC-036). */
import { config } from "../config";
import type { MarketMeta } from "../format";

export interface Status {
  lane_id: string;
  config_hash: string;
  lane_name: string;
  height: string;
  state_hash: string;
  last_block_timestamp_ms: string;
  block_time_ms: number;
  checkpoint_every_blocks: number;
  halted: string | null;
  checkpoints: { sequenced: string | null; signed: string | null; accepted: string | null };
  signers: { epoch: string; threshold: number; validators: { index: number; url: string; key: string; weight: number }[] } | null;
}

export interface Position {
  market_id: number;
  symbol: string;
  lots: number;
  cost_basis: string;
  entry_price: string | null;
  mark_price: string;
  upnl: string;
  liq_price: string | null;
}

export interface OpenOrder {
  market_id: number;
  order_id: string;
  side: "buy" | "sell";
  price: string;
  lots_remaining: number;
  client_order_id: string;
}

export interface Account {
  account: string;
  collateral: string;
  equity: string;
  free_collateral: string;
  initial_margin: string;
  maintenance_margin: string;
  next_nonce: string;
  positions: Position[];
  open_orders: OpenOrder[];
  session_keys: { key: string; expires_at_ms: string; permissions: number }[];
}

export interface Market extends MarketMeta {
  imf_bps: number;
  mmf_bps: number;
  taker_fee_bps: number;
  maker_fee_bps: number;
  band_bps: number;
  max_position_lots: number;
  oracle_price: string;
  oracle_time_ms: string;
  open_interest_lots: number;
  best_bid: string | null;
  best_ask: string | null;
}

/** A market's line in the stream's per-block `tickers` message. */
export interface Ticker {
  market_id: number;
  oracle_price: string;
  oracle_time_ms: string;
  best_bid: string | null;
  best_ask: string | null;
  open_interest_lots: number;
}

/** A candle of the oracle price (the mark), stroops per lot; `t` is its start in ms. */
export interface Candle {
  t: number;
  open: string;
  high: string;
  low: string;
  close: string;
}

export type CandleInterval = "1m" | "5m" | "15m" | "1h";

export interface Level {
  price: string;
  lots: number;
  orders: number;
}

export interface Book {
  market_id: number;
  height: string;
  bids: Level[];
  asks: Level[];
}

export interface Fill {
  height: string;
  timestamp_ms: string;
  market_id: number;
  price: string;
  lots: number;
  taker_side: "buy" | "sell";
  maker_order_id: string;
  maker: string;
  taker: string;
}

export interface Proof {
  seq: string;
  index: number;
  account: string;
  amount?: string;
  equity?: string;
  proof: string[];
}

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

/**
 * A host that serves only the app answers every path with its index.html, so
 * a response that is not JSON means there is no lane API at `base`.
 */
function notTheApi(r: Response, base: string): ApiError | null {
  const type = r.headers.get("content-type") ?? "";
  if (type.includes("application/json")) return null;
  return new ApiError(
    r.status,
    "NOT_THE_API",
    `${base} answered with ${type.split(";")[0] || "no content type"}, not the lane API. Open the app where the lane serves it, or run it with npm run dev, which proxies the testnet lane.`,
  );
}

async function get<T>(path: string, base = config.sequencerUrl): Promise<T> {
  const r = await fetch(`${base}${path}`);
  const wrongHost = notTheApi(r, base);
  if (wrongHost && (r.ok || r.status === 404)) throw wrongHost;
  if (!r.ok) {
    const body = (await r.json().catch(() => ({}))) as { code?: string; error?: string };
    throw new ApiError(r.status, body.code ?? "HTTP", body.error ?? `HTTP ${r.status}`);
  }
  return (await r.json()) as T;
}

export const lane = {
  status: () => get<Status>("/v1/status"),
  account: async (g: string) => {
    try {
      return await get<Account>(`/v1/accounts/${g}`);
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) return null;
      throw e;
    }
  },
  markets: () => get<Market[]>("/v1/markets"),
  book: (id: number, depth = 20) => get<Book>(`/v1/markets/${id}/book?depth=${depth}`),
  trades: (id: number, limit = 50) => get<Fill[]>(`/v1/markets/${id}/trades?limit=${limit}`),
  candles: (id: number, interval: CandleInterval, limit = 500) => get<Candle[]>(`/v1/markets/${id}/candles?interval=${interval}&limit=${limit}`),
  block: (h: number | string, base?: string) => get<Record<string, unknown>>(`/v1/blocks/${h}`, base),
  checkpoint: (seq: number | string, base?: string) => get<Record<string, unknown>>(`/v1/checkpoints/${seq}`, base),
  withdrawalProofs: (g: string, base?: string) => get<{ withdrawals: Proof[] }>(`/v1/proofs/withdrawals?account=${g}`, base),
  escapeProof: (g: string, base?: string) => get<Proof>(`/v1/proofs/escape?account=${g}`, base),
  validatorStatus: (base: string) => get<Record<string, unknown>>("/v1/status", base),
  async postTx(signedHex: string): Promise<{ tx_hash: string }> {
    const r = await fetch(`${config.sequencerUrl}/v1/tx`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ tx: signedHex }) });
    const body = (await r.json().catch(() => ({}))) as { code?: string; error?: string; tx_hash?: string };
    if (r.status !== 202) throw new ApiError(r.status, body.code ?? "HTTP", body.error ?? `HTTP ${r.status}`);
    return { tx_hash: body.tx_hash ?? "" };
  },
};

/** `WS /v1/stream`: reconnects with backoff; returns a stop function. `onLive` hears when it connects and drops. */
export function stream(
  sub: { blocks?: boolean; markets?: number[]; tickers?: boolean; account?: string | null },
  onMessage: (m: Record<string, unknown> & { type: string }) => void,
  onLive?: (live: boolean) => void,
): () => void {
  let ws: WebSocket | null = null;
  let stopped = false;
  let delay = 500;
  const open = () => {
    if (stopped) return;
    ws = new WebSocket(`${config.sequencerUrl.replace(/^http/, "ws")}/v1/stream`);
    ws.onopen = () => {
      delay = 500;
      ws?.send(JSON.stringify({ blocks: sub.blocks ?? false, markets: sub.markets ?? [], tickers: sub.tickers ?? false, account: sub.account ?? undefined }));
      onLive?.(true);
    };
    ws.onmessage = (e) => {
      try {
        onMessage(JSON.parse(String(e.data)));
      } catch {
        /* ignore malformed frames */
      }
    };
    ws.onclose = () => {
      onLive?.(false);
      if (stopped) return;
      setTimeout(open, delay);
      delay = Math.min(delay * 2, 10_000);
    };
  };
  open();
  return () => {
    stopped = true;
    ws?.close();
  };
}
