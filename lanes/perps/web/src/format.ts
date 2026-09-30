/** Display formatting. Amounts arrive as decimal strings of stroops (1 USDC = 10^7). */
export const USDC = 10_000_000n;

export function usdc(stroops: string | bigint | null | undefined, digits = 2): string {
  if (stroops === null || stroops === undefined) return "–";
  const v = BigInt(stroops);
  const neg = v < 0n;
  const a = neg ? -v : v;
  const whole = a / USDC;
  const frac = (a % USDC).toString().padStart(7, "0").slice(0, digits);
  const w = whole.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return `${neg ? "−" : ""}${w}${digits > 0 ? `.${frac}` : ""}`;
}

/** Parses "12.5" USDC into stroops; null if not a plain decimal with at most 7 places. */
export function parseUsdc(s: string): bigint | null {
  const m = /^\s*(\d+)(?:\.(\d{0,7}))?\s*$/.exec(s);
  if (!m) return null;
  return BigInt(m[1]!) * USDC + BigInt((m[2] ?? "").padEnd(7, "0") || "0");
}

export interface MarketMeta {
  market_id: number;
  symbol: string;
  tick: string;
  display_lot_base_units: number;
  display_base_decimals: number;
}

/** USD per whole base unit from stroops per lot. */
export function perUnit(pricePerLot: string | bigint, m: MarketMeta): number {
  const lotsPerUnit = 10 ** m.display_base_decimals / m.display_lot_base_units;
  return (Number(pricePerLot) / 1e7) * lotsPerUnit;
}

/** Stroops per lot from USD per whole base unit, snapped down to the tick. */
export function toPricePerLot(usdPerUnit: number, m: MarketMeta): bigint {
  const perLot = (usdPerUnit * m.display_lot_base_units) / 10 ** m.display_base_decimals;
  const stroops = BigInt(Math.round(perLot * 1e7));
  const tick = BigInt(m.tick);
  return (stroops / tick) * tick;
}

/** Base units for a number of lots, e.g. 4 lots of BTC → "0.0004". */
export function baseAmount(lots: number | bigint, m: MarketMeta): string {
  const units = (Number(lots) * m.display_lot_base_units) / 10 ** m.display_base_decimals;
  return units.toLocaleString("en-US", { maximumFractionDigits: 8 });
}

export function base(symbol: string): string {
  return symbol.replace(/-PERP$/, "");
}

export function price(v: number): string {
  const digits = v >= 1000 ? 1 : v >= 1 ? 2 : 5;
  return v.toLocaleString("en-US", { minimumFractionDigits: digits, maximumFractionDigits: digits });
}

export function short(key: string, n = 4): string {
  return key.length > 2 * n + 1 ? `${key.slice(0, n)}…${key.slice(-n)}` : key;
}

export function ago(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  return `${Math.floor(s / 3600)}h ago`;
}
