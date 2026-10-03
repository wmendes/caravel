/**
 * Candles of the oracle price (the mark), from the lane's own history
 * (`/v1/markets/{id}/candles`, DEC-103), with the last candle moving on every
 * block as the stream's tickers arrive (lightweight-charts 5).
 */
import { CandlestickSeries, ColorType, createChart, CrosshairMode, LineStyle, type CandlestickData, type IChartApi, type ISeriesApi, type UTCTimestamp } from "lightweight-charts";
import { useEffect, useRef, useState } from "react";

import { lane, type CandleInterval } from "../api/lane";
import { perUnit, type MarketMeta } from "../format";

// The chart library takes sRGB strings; these match the OKLCH tokens in styles.css.
const INK_3 = "#8a9599";
const HAIR = "#232b2e";
const LINE = "#3a4448";
const BUY = "#3fcf8e";
const SELL = "#f2637e";

export const INTERVALS: { id: CandleInterval; label: string; ms: number }[] = [
  { id: "1m", label: "1m", ms: 60_000 },
  { id: "5m", label: "5m", ms: 300_000 },
  { id: "15m", label: "15m", ms: 900_000 },
  { id: "1h", label: "1h", ms: 3_600_000 },
];

export function PriceChart({ market, price, priceTimeMs, interval, digits }: { market: MarketMeta; price: number; priceTimeMs: number; interval: CandleInterval; digits: number }) {
  const el = useRef<HTMLDivElement>(null);
  const series = useRef<ISeriesApi<"Candlestick"> | null>(null);
  const chart = useRef<IChartApi | null>(null);
  const last = useRef<CandlestickData<UTCTimestamp> | null>(null);
  const [state, setState] = useState<"loading" | "ready" | "empty" | "error">("loading");
  const span = INTERVALS.find((i) => i.id === interval)?.ms ?? 60_000;

  useEffect(() => {
    if (!el.current) return;
    const c = createChart(el.current, {
      autoSize: true,
      layout: { background: { type: ColorType.Solid, color: "transparent" }, textColor: INK_3, fontFamily: "Inter, system-ui, sans-serif", fontSize: 11 },
      grid: { vertLines: { color: HAIR }, horzLines: { color: HAIR } },
      rightPriceScale: { borderColor: HAIR },
      timeScale: { borderColor: HAIR, timeVisible: true, secondsVisible: false, rightOffset: 6, barSpacing: 9, minBarSpacing: 3 },
      crosshair: { mode: CrosshairMode.Normal, vertLine: { color: LINE, style: LineStyle.Dashed, labelBackgroundColor: LINE }, horzLine: { color: LINE, style: LineStyle.Dashed, labelBackgroundColor: LINE } },
    });
    series.current = c.addSeries(CandlestickSeries, {
      upColor: BUY,
      downColor: SELL,
      borderUpColor: BUY,
      borderDownColor: SELL,
      wickUpColor: BUY,
      wickDownColor: SELL,
      priceLineColor: LINE,
      priceLineStyle: LineStyle.Dotted,
    });
    chart.current = c;
    return () => {
      c.remove();
      chart.current = null;
      series.current = null;
    };
  }, []);

  useEffect(() => {
    series.current?.applyOptions({ priceFormat: { type: "price", precision: digits, minMove: 1 / 10 ** digits } });
  }, [digits]);

  // History for this market and interval.
  useEffect(() => {
    let live = true;
    setState("loading");
    last.current = null;
    series.current?.setData([]);
    lane
      .candles(market.market_id, interval, 500)
      .then((cs) => {
        if (!live || !series.current) return;
        const data = cs.map((c) => ({
          time: (c.t / 1000) as UTCTimestamp,
          open: perUnit(c.open, market),
          high: perUnit(c.high, market),
          low: perUnit(c.low, market),
          close: perUnit(c.close, market),
        }));
        series.current.setData(data);
        last.current = data.at(-1) ?? null;
        setState(data.length ? "ready" : "empty");
        chart.current?.timeScale().scrollToRealTime();
      })
      .catch(() => live && setState("error"));
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [market.market_id, interval]);

  // Every block: the live price moves the last candle, or opens the next one.
  useEffect(() => {
    if (!series.current || price <= 0 || !priceTimeMs || state === "loading") return;
    const t = (Math.floor(priceTimeMs / span) * span) / 1000;
    const prev = last.current;
    let next: CandlestickData<UTCTimestamp>;
    if (prev && prev.time === t) next = { ...prev, high: Math.max(prev.high, price), low: Math.min(prev.low, price), close: price };
    else if (!prev || t > prev.time) next = { time: t as UTCTimestamp, open: prev?.close ?? price, high: Math.max(prev?.close ?? price, price), low: Math.min(prev?.close ?? price, price), close: price };
    else return;
    series.current.update(next);
    last.current = next;
    if (state !== "ready") setState("ready");
  }, [price, priceTimeMs, span, state]);

  return (
    <div className="chart-wrap">
      <div className="chart" ref={el} role="img" aria-label={`Candles of the ${interval} oracle price`} />
      {state !== "ready" && (
        <div className="chart-note">{state === "loading" ? "Loading price history…" : state === "error" ? "No price history from the lane API." : "No prices in this range yet."}</div>
      )}
    </div>
  );
}
