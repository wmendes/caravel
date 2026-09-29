/**
 * USD prices to lane prices (spec §17.3), in exact integer arithmetic:
 * `price_per_lot = round_half_even(usd × 10^7 × display_lot_base_units / 10^display_base_decimals)`,
 * then snapped to a multiple of `tick`.
 */

/** A non-negative decimal as `num / den`. */
export interface Ratio {
  num: bigint;
  den: bigint;
}

/** Parses a decimal string such as "83574.925" exactly. */
export function parseDecimal(s: string): Ratio {
  const m = /^(\d+)(?:\.(\d+))?$/.exec(s.trim());
  if (!m) throw new Error(`not a non-negative decimal: ${JSON.stringify(s)}`);
  const frac = m[2] ?? "";
  return { num: BigInt((m[1] ?? "0") + frac), den: 10n ** BigInt(frac.length) };
}

/** `round_half_even(num / den)` for `num ≥ 0`, `den > 0`. */
export function roundHalfEven(num: bigint, den: bigint): bigint {
  if (num < 0n || den <= 0n) throw new Error("roundHalfEven needs num ≥ 0 and den > 0");
  const q = num / den;
  const r = num % den;
  const twice = 2n * r;
  if (twice > den || (twice === den && q % 2n === 1n)) return q + 1n;
  return q;
}

/** USDC stroops per lot for a USD price per whole base unit. */
export function pricePerLot(usd: Ratio, displayLotBaseUnits: bigint, displayBaseDecimals: number): bigint {
  return roundHalfEven(usd.num * 10_000_000n * displayLotBaseUnits, usd.den * 10n ** BigInt(displayBaseDecimals));
}

/** The nearest multiple of `tick` (ties to even multiples), at least one tick. */
export function snapToTick(price: bigint, tick: bigint): bigint {
  const snapped = roundHalfEven(price, tick) * tick;
  return snapped < tick ? tick : snapped;
}

/** A price with the time the source says it was observed. */
export interface Quote {
  usd: Ratio;
  /** Unix seconds. */
  observedAt: number;
  source: string;
}

export interface PriceSource {
  readonly name: string;
  quote(): Promise<Quote>;
}

/** Coinbase's public spot price: `GET /v2/prices/{pair}/spot` → `{data: {amount}}` (checked 2026-09-29). */
export class CoinbaseSpot implements PriceSource {
  readonly name: string;

  constructor(
    private readonly pair: string,
    private readonly fetchImpl: typeof fetch = fetch,
  ) {
    this.name = `coinbase:${pair}`;
  }

  async quote(): Promise<Quote> {
    const r = await this.fetchImpl(`https://api.coinbase.com/v2/prices/${encodeURIComponent(this.pair)}/spot`, { signal: AbortSignal.timeout(5_000) });
    if (!r.ok) throw new Error(`${this.name}: HTTP ${r.status}`);
    const body = (await r.json()) as { data?: { amount?: string } };
    const amount = body.data?.amount;
    if (typeof amount !== "string") throw new Error(`${this.name}: no data.amount`);
    return { usd: parseDecimal(amount), observedAt: Math.floor(Date.now() / 1000), source: this.name };
  }
}

/** A fixed price, for local lanes (scripts/e2e-local.sh). */
export class FixedPrice implements PriceSource {
  readonly name: string;
  private readonly usd: Ratio;

  constructor(usd: string) {
    this.usd = parseDecimal(usd);
    this.name = `fixed:${usd}`;
  }

  async quote(): Promise<Quote> {
    return { usd: this.usd, observedAt: Math.floor(Date.now() / 1000), source: this.name };
  }
}

/** Reflector's SEP-40 `lastprice(asset)`: `{price: i128, timestamp: u64}` with `decimals()` places. */
export interface ReflectorReader {
  lastPrice(asset: string): Promise<{ price: bigint; timestamp: bigint } | null>;
}

/**
 * Reflector updates every 300 s, so a quote is reused for `cacheSecs` rather
 * than simulating `lastprice` on every 2 s tick against a public RPC.
 */
export class ReflectorPrice implements PriceSource {
  readonly name: string;
  private cached: { quote: Quote; atMs: number } | null = null;

  constructor(
    private readonly reader: ReflectorReader,
    private readonly asset: string,
    private readonly decimals: number,
    private readonly cacheSecs = 30,
    private readonly now: () => number = Date.now,
  ) {
    this.name = `reflector:${asset}`;
  }

  async quote(): Promise<Quote> {
    if (this.cached && this.now() - this.cached.atMs < this.cacheSecs * 1000) return this.cached.quote;
    const p = await this.reader.lastPrice(this.asset);
    if (!p) throw new Error(`${this.name}: no price`);
    const quote = { usd: { num: p.price, den: 10n ** BigInt(this.decimals) }, observedAt: Number(p.timestamp), source: this.name };
    this.cached = { quote, atMs: this.now() };
    return quote;
  }
}

/**
 * The first source whose quote is recent enough (spec §17.3: Reflector first,
 * a public spot API as fallback). Errors and stale quotes fall through.
 */
export async function firstFresh(sources: PriceSource[], maxAgeSecs: number, nowSecs: number): Promise<Quote> {
  const problems: string[] = [];
  for (const s of sources) {
    try {
      const q = await s.quote();
      if (nowSecs - q.observedAt <= maxAgeSecs) return q;
      problems.push(`${s.name}: ${nowSecs - q.observedAt}s old`);
    } catch (e) {
      problems.push(String(e));
    }
  }
  throw new Error(`no fresh price: ${problems.join("; ")}`);
}
