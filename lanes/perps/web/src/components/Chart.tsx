/** Oracle price per base unit, sampled every 2 s while the page is open (lightweight-charts 5). */
import { ColorType, createChart, LineSeries, type IChartApi, type ISeriesApi, type UTCTimestamp } from "lightweight-charts";
import { useEffect, useRef } from "react";

export function PriceChart({ points }: { points: { t: number; v: number }[] }) {
  const el = useRef<HTMLDivElement>(null);
  const chart = useRef<IChartApi | null>(null);
  const series = useRef<ISeriesApi<"Line"> | null>(null);

  useEffect(() => {
    if (!el.current) return;
    const c = createChart(el.current, {
      autoSize: true,
      layout: { background: { type: ColorType.Solid, color: "transparent" }, textColor: "#a8bab8", fontFamily: "Schibsted Grotesk, system-ui, sans-serif" },
      grid: { vertLines: { color: "#1d3d42" }, horzLines: { color: "#1d3d42" } },
      rightPriceScale: { borderColor: "#1d3d42" },
      timeScale: { borderColor: "#1d3d42", timeVisible: true, secondsVisible: true },
      crosshair: { vertLine: { color: "#4b7178" }, horzLine: { color: "#4b7178" } },
    });
    series.current = c.addSeries(LineSeries, { color: "#f3eee4", lineWidth: 2, priceLineColor: "#4b7178" });
    chart.current = c;
    return () => {
      c.remove();
      chart.current = null;
      series.current = null;
    };
  }, []);

  useEffect(() => {
    const seen = new Map<number, number>();
    for (const p of points) seen.set(Math.floor(p.t / 1000), p.v);
    series.current?.setData([...seen.entries()].sort((a, b) => a[0] - b[0]).map(([t, v]) => ({ time: t as UTCTimestamp, value: v })));
  }, [points]);

  return <div className="chart" ref={el} aria-label="Oracle price chart" />;
}
