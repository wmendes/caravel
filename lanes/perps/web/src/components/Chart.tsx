/** Oracle price per base unit, sampled every 2 s while the page is open (lightweight-charts 5). */
import { AreaSeries, ColorType, createChart, CrosshairMode, LineStyle, type IChartApi, type ISeriesApi, type UTCTimestamp } from "lightweight-charts";
import { useEffect, useRef } from "react";

// The chart library takes sRGB strings; these match the OKLCH tokens in styles.css.
const INK = "#eef1f2";
const INK_3 = "#8a9599";
const HAIR = "#232b2e";
const LINE = "#3a4448";

export function PriceChart({ points, digits }: { points: { t: number; v: number }[]; digits: number }) {
  const el = useRef<HTMLDivElement>(null);
  const chart = useRef<IChartApi | null>(null);
  const series = useRef<ISeriesApi<"Area"> | null>(null);

  useEffect(() => {
    if (!el.current) return;
    const c = createChart(el.current, {
      autoSize: true,
      layout: { background: { type: ColorType.Solid, color: "transparent" }, textColor: INK_3, fontFamily: "Inter, system-ui, sans-serif", fontSize: 11 },
      grid: { vertLines: { color: HAIR }, horzLines: { color: HAIR } },
      rightPriceScale: { borderColor: HAIR },
      timeScale: { borderColor: HAIR, timeVisible: true, secondsVisible: true },
      crosshair: { mode: CrosshairMode.Normal, vertLine: { color: LINE, style: LineStyle.Dashed, labelBackgroundColor: LINE }, horzLine: { color: LINE, style: LineStyle.Dashed, labelBackgroundColor: LINE } },
    });
    series.current = c.addSeries(AreaSeries, {
      lineColor: INK,
      lineWidth: 2,
      topColor: "rgba(238, 241, 242, 0.10)",
      bottomColor: "rgba(238, 241, 242, 0)",
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

  useEffect(() => {
    const seen = new Map<number, number>();
    for (const p of points) seen.set(Math.floor(p.t / 1000), p.v);
    series.current?.setData([...seen.entries()].sort((a, b) => a[0] - b[0]).map(([t, v]) => ({ time: t as UTCTimestamp, value: v })));
  }, [points]);

  return <div className="chart" ref={el} role="img" aria-label="Oracle price chart" />;
}
