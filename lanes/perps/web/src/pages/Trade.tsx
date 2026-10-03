/**
 * The trading terminal: market bar, chart, book and trades, the order ticket
 * with the account beneath it, and positions, orders and your trades below.
 */
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type FormEvent } from "react";

import { lane, stream, type Account, type Book, type Fill, type Level, type Market } from "../api/lane";
import { Side, Tif } from "../codec/tx";
import { PriceChart } from "../components/Chart";
import { Chip, Empty, SidePill, Signed, Skel, Tabs, useFlash, useNow, useToast } from "../components/ui";
import { ago, base, baseAmount, perUnit, price, short, toPricePerLot, usdc } from "../format";
import { Link, useRoute } from "../router";
import type { SessionKey } from "../session";
import { useApp } from "../state";
import { enableSessionKey, explainReject, laneIds, send, type LaneIds } from "../trade";

/** An oracle price older than this is shown as stale. */
const STALE_MS = 30_000;

const digitsFor = (v: number) => (v >= 1000 ? 1 : v >= 1 ? 2 : 5);
const unitsPerLot = (m: Market) => m.display_lot_base_units / 10 ** m.display_base_decimals;

/**
 * A market order is an IOC limit at the edge of the price band, kept a tenth
 * inside it so a small oracle move between signing and sequencing does not
 * reject it. The engine checks |price − oracle| ≤ band.
 */
function marketLimit(m: Market, side: Side): bigint {
  const oracle = BigInt(m.oracle_price);
  const tick = BigInt(m.tick);
  const band = BigInt(Math.floor((m.band_bps * 9) / 10));
  if (side === Side.Buy) return ((oracle * (10_000n + band)) / 10_000n / tick) * tick;
  const raw = (oracle * (10_000n - band)) / 10_000n;
  return ((raw + tick - 1n) / tick) * tick;
}

export function Trade({ symbol }: { symbol: string }) {
  const { markets, status, address, key, account, setAccount } = useApp();
  const market = markets.find((m) => m.symbol === symbol) ?? markets[0];
  const [book, setBook] = useState<Book | null>(null);
  const [fills, setFills] = useState<Fill[] | null>(null);
  const [points, setPoints] = useState<{ t: number; v: number }[]>([]);
  const [picked, setPicked] = useState<{ px: string; n: number } | null>(null);
  const [side, setSide] = useState<Side>(Side.Buy);
  const marketId = market?.market_id;
  const ids = laneIds(status);

  // Price history for the chart, from the oracle price the lane holds.
  useEffect(() => {
    if (!market) return;
    const v = perUnit(market.oracle_price, market);
    if (v > 0) setPoints((p) => [...p.slice(-900), { t: Date.now(), v }]);
  }, [market?.oracle_price, market]);
  useEffect(() => {
    setPoints([]);
    setBook(null);
    setFills(null);
  }, [marketId]);

  useEffect(() => {
    if (marketId === undefined) return;
    void lane.book(marketId).then(setBook).catch(() => {});
    void lane.trades(marketId, 60).then(setFills).catch(() => setFills([]));
    return stream({ markets: [marketId], account: address }, (m) => {
      if (m.type === "book" && m.market_id === marketId) setBook(m as unknown as Book);
      if (m.type === "fill" && m.market_id === marketId) setFills((f) => [m as unknown as Fill, ...(f ?? [])].slice(0, 60));
      if (m.type === "account") setAccount(m as unknown as Account);
    });
  }, [marketId, address, setAccount]);

  // B and S pick the side, as on most terminals; never while typing.
  useEffect(() => {
    const on = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable)) return;
      if (e.key === "b" || e.key === "B") setSide(Side.Buy);
      if (e.key === "s" || e.key === "S") setSide(Side.Sell);
    };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, []);

  if (!market) {
    return (
      <div className="terminal" aria-busy="true">
        <div className="marketbar">
          <div className="stats">
            <Skel w={120} h={14} />
            <Skel w={90} h={14} />
            <Skel w={90} h={14} />
          </div>
        </div>
        <div className="pane chart-pane">
          <Empty title="Loading markets">Waiting for the lane API.</Empty>
        </div>
        <div className="pane book-pane" />
        <div className="pane ticket-pane" />
        <div className="pane bottom-pane" />
      </div>
    );
  }

  return (
    <>
      <div className="terminal">
        <MarketBar market={market} markets={markets} book={book} />
        <section className="pane chart-pane" aria-label="Chart">
          <div className="chart-head">
            <b>Oracle price</b>
            <span>Signed by the Caravel team's oracle key, from Coinbase market data. Sampled while this page is open.</span>
          </div>
          <PriceChart points={points} digits={digitsFor(perUnit(market.oracle_price, market))} />
        </section>
        <BookPane market={market} book={book} fills={fills} onPick={(px) => setPicked((p) => ({ px, n: (p?.n ?? 0) + 1 }))} />
        <section className="pane ticket-pane" aria-label="Order ticket">
          <Ticket market={market} ids={ids} address={address} sessionKey={key} account={account} book={book} side={side} setSide={setSide} picked={picked} />
          <AccountBox account={account} address={address} ids={ids} hasKey={key !== null} keyExpiry={key?.expiresAtMs ?? null} />
        </section>
        <BottomPane market={market} markets={markets} account={account} fills={fills} address={address} ids={ids} sessionKey={key} />
      </div>
      {address && account && (
        <div className="mobile-bar">
          <button className="btn buy lg" onClick={() => { setSide(Side.Buy); document.getElementById("size")?.focus(); }}>
            Long
          </button>
          <button className="btn sell lg" onClick={() => { setSide(Side.Sell); document.getElementById("size")?.focus(); }}>
            Short
          </button>
        </div>
      )}
    </>
  );
}

/* ------------------------------------------------------------ market bar */

function Coin({ symbol }: { symbol: string }) {
  const b = base(symbol);
  return (
    <span className={`coin ${b.toLowerCase()}`} aria-hidden>
      {b.slice(0, 1)}
    </span>
  );
}

function MarketBar({ market, markets, book }: { market: Market; markets: Market[]; book: Book | null }) {
  const now = useNow(1000);
  const oracle = perUnit(market.oracle_price, market);
  const flash = useFlash(oracle);
  const age = now - Number(market.oracle_time_ms);
  const bestBid = book?.bids[0]?.price ?? market.best_bid;
  const bestAsk = book?.asks[0]?.price ?? market.best_ask;
  const bid = bestBid ? perUnit(bestBid, market) : null;
  const ask = bestAsk ? perUnit(bestAsk, market) : null;
  const spread = bid !== null && ask !== null ? ask - bid : null;
  const oi = market.open_interest_lots;
  return (
    <div className="marketbar">
      <MarketPicker market={market} markets={markets} />
      <div className="stats">
        <div className="stat big">
          <span className="k" title="The price the lane marks positions at: the latest oracle update in a lane block">
            Mark (oracle)
          </span>
          <span className={`v num ${flash}`}>{price(oracle)}</span>
        </div>
        <div className="stat">
          <span className="k">Oracle update</span>
          <span className="v num">{age > STALE_MS ? <Chip kind="warn">stale · {ago(Number(market.oracle_time_ms), now)}</Chip> : ago(Number(market.oracle_time_ms), now)}</span>
        </div>
        <div className="stat">
          <span className="k">Best bid / ask</span>
          <span className="v num">
            <span className="up">{bid !== null ? price(bid) : "–"}</span>
            <span className="faint"> / </span>
            <span className="down">{ask !== null ? price(ask) : "–"}</span>
          </span>
        </div>
        <div className="stat">
          <span className="k">Spread</span>
          <span className="v num">{spread !== null ? `${spread.toFixed(digitsFor(oracle))} (${((spread / oracle) * 100).toFixed(3)}%)` : "–"}</span>
        </div>
        <div className="stat">
          <span className="k" title="Long lots open, which equals short lots open">
            Open interest
          </span>
          <span className="v num">
            {baseAmount(oi, market)} {base(market.symbol)}
            <span className="faint"> · ${Math.round(oi * unitsPerLot(market) * oracle).toLocaleString("en-US")}</span>
          </span>
        </div>
        <div className="stat">
          <span className="k" title={`Initial margin ${market.imf_bps / 100}%, maintenance ${market.mmf_bps / 100}%`}>
            Max leverage
          </span>
          <span className="v num">{Math.floor(10_000 / market.imf_bps)}×</span>
        </div>
        <div className="stat">
          <span className="k">Maker / taker fee</span>
          <span className="v num">
            {market.maker_fee_bps / 100}% / {market.taker_fee_bps / 100}%
          </span>
        </div>
        <div className="stat">
          <span className="k" title="Orders priced further than this from the oracle price are rejected">
            Price band
          </span>
          <span className="v num">±{market.band_bps / 100}%</span>
        </div>
      </div>
    </div>
  );
}

function MarketPicker({ market, markets }: { market: Market; markets: Market[] }) {
  const { go } = useRoute();
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState({ top: 0, left: 0 });
  const btn = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    if (!open || !btn.current) return;
    const r = btn.current.getBoundingClientRect();
    setPos({ top: r.bottom + 6, left: r.left });
    menu.current?.querySelector<HTMLButtonElement>("button")?.focus();
  }, [open]);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent | KeyboardEvent) => {
      if (e instanceof KeyboardEvent && e.key !== "Escape") return;
      if (e instanceof MouseEvent && (menu.current?.contains(e.target as Node) || btn.current?.contains(e.target as Node))) return;
      setOpen(false);
      if (e instanceof KeyboardEvent) btn.current?.focus();
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", close);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", close);
    };
  }, [open]);
  return (
    <div className="picker">
      <button ref={btn} aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        <Coin symbol={market.symbol} />
        <span className="sym">{market.symbol}</span>
        <svg className="caret" width="10" height="10" viewBox="0 0 10 10" aria-hidden>
          <path d="M1.5 3.5 5 7l3.5-3.5" fill="none" stroke="currentColor" strokeWidth="1.5" />
        </svg>
      </button>
      {open && (
        <div className="menu" role="menu" ref={menu} style={{ top: pos.top, left: pos.left }}>
          {markets.map((m) => (
            <button
              key={m.market_id}
              role="menuitem"
              aria-current={m.market_id === market.market_id}
              onClick={() => {
                setOpen(false);
                go(`/trade/${m.symbol}`);
              }}
            >
              <Coin symbol={m.symbol} />
              <span>
                <span className="sym">{m.symbol}</span> <span className="faint">{Math.floor(10_000 / m.imf_bps)}×</span>
              </span>
              <span className="px">{price(perUnit(m.oracle_price, m))}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/* ------------------------------------------------------------ book and trades */

function BookPane({ market, book, fills, onPick }: { market: Market; book: Book | null; fills: Fill[] | null; onPick: (px: string) => void }) {
  const [tab, setTab] = useState<"book" | "trades">("book");
  return (
    <section className="pane book-pane" aria-label="Order book and trades">
      <Tabs
        label="Book and trades"
        value={tab}
        onChange={setTab}
        tabs={[
          { id: "book", label: "Order book" },
          { id: "trades", label: "Trades" },
        ]}
      />
      {tab === "book" ? <BookView market={market} book={book} onPick={onPick} /> : <TradesView market={market} fills={fills} />}
    </section>
  );
}

function BookView({ market, book, onPick }: { market: Market; book: Book | null; onPick: (px: string) => void }) {
  const rows = 14;
  const cum = (levels: Level[]) => {
    let t = 0;
    return levels.slice(0, rows).map((l) => ({ ...l, total: (t += l.lots) }));
  };
  const asks = cum(book?.asks ?? []);
  const bids = cum(book?.bids ?? []);
  const max = Math.max(1, asks.at(-1)?.total ?? 0, bids.at(-1)?.total ?? 0);
  const b = base(market.symbol);
  const row = (l: Level & { total: number }, side: "bid" | "ask") => {
    const p = price(perUnit(l.price, market));
    return (
      <button
        type="button"
        className={`book-row ${side}`}
        key={`${side}${l.price}`}
        onClick={() => onPick(p.replace(/,/g, ""))}
        title={`Use ${p} as the order price`}
        aria-label={`${side === "bid" ? "Bid" : "Ask"} ${p}, size ${baseAmount(l.lots, market)} ${b}`}
      >
        <i className="bar" style={{ width: `${(l.total / max) * 100}%` }} />
        <span className="p">{p}</span>
        <span>{baseAmount(l.lots, market)}</span>
        <span className="faint">{baseAmount(l.total, market)}</span>
      </button>
    );
  };
  const bid = bids[0] ? perUnit(bids[0].price, market) : null;
  const ask = asks[0] ? perUnit(asks[0].price, market) : null;
  const oracle = perUnit(market.oracle_price, market);
  return (
    <div className="book">
      <div className="book-cols">
        <span>Price</span>
        <span>Size ({b})</span>
        <span>Total</span>
      </div>
      <div className="book-side asks">
        {!book ? <BookSkel /> : asks.length ? [...asks].reverse().map((l) => row(l, "ask")) : <p className="book-empty">No asks resting</p>}
      </div>
      <div className="book-mid">
        <span className="px num" title="Oracle price">
          {price(oracle)}
        </span>
        <span className="faint num">{bid !== null && ask !== null ? `Spread ${(ask - bid).toFixed(digitsFor(oracle))} · ${(((ask - bid) / oracle) * 100).toFixed(3)}%` : "No two-sided market"}</span>
      </div>
      <div className="book-side bids">{!book ? <BookSkel /> : bids.length ? bids.map((l) => row(l, "bid")) : <p className="book-empty">No bids resting</p>}</div>
    </div>
  );
}

function BookSkel() {
  return (
    <div style={{ display: "grid", gap: 8, padding: "8px 12px" }} aria-hidden>
      {Array.from({ length: 6 }, (_, i) => (
        <Skel key={i} w="100%" h={10} />
      ))}
    </div>
  );
}

function TradesView({ market, fills }: { market: Market; fills: Fill[] | null }) {
  return (
    <div className="pane-body">
      <div className="book-cols">
        <span>Price</span>
        <span>Size ({base(market.symbol)})</span>
        <span>Time</span>
      </div>
      {fills === null ? (
        <BookSkel />
      ) : fills.length === 0 ? (
        <Empty title="No trades yet">Trades on {market.symbol} show here as the lane matches them.</Empty>
      ) : (
        fills.map((f, i) => {
          const buy = f.taker_side === "buy";
          return (
            <div className="trades-row" key={`${f.height}-${i}`}>
              <span className={buy ? "up" : "down"}>
                <span className="sr-only">{buy ? "Buy" : "Sell"} </span>
                <span aria-hidden>{buy ? "▲ " : "▼ "}</span>
                {price(perUnit(f.price, market))}
              </span>
              <span>{baseAmount(f.lots, market)}</span>
              <span className="faint">{new Date(Number(f.timestamp_ms)).toLocaleTimeString("en-GB")}</span>
            </div>
          );
        })
      )}
    </div>
  );
}

/* ------------------------------------------------------------ ticket */

type Kind = "limit" | "market" | "post";

function Ticket({
  market,
  ids,
  address,
  sessionKey,
  account,
  book,
  side,
  setSide,
  picked,
}: {
  market: Market;
  ids: LaneIds | null;
  address: string | null;
  sessionKey: SessionKey | null;
  account: Account | null | undefined;
  book: Book | null;
  side: Side;
  setSide: (s: Side) => void;
  picked: { px: string; n: number } | null;
}) {
  const { connect, connecting } = useApp();
  const toast = useToast();
  const [kind, setKind] = useState<Kind>("limit");
  const [reduceOnly, setReduceOnly] = useState(false);
  const [px, setPx] = useState("");
  const [size, setSize] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const touched = useRef(false);
  const oracle = perUnit(market.oracle_price, market);
  const b = base(market.symbol);
  const lot = unitsPerLot(market);
  const long = side === Side.Buy;

  useEffect(() => {
    touched.current = false;
    setPx("");
    setSize("");
    setErr(null);
  }, [market.market_id]);
  useEffect(() => {
    if (!touched.current && oracle > 0) setPx(price(oracle).replace(/,/g, ""));
  }, [oracle]);
  useEffect(() => {
    if (!picked) return;
    touched.current = true;
    setPx(picked.px);
    if (kind === "market") setKind("limit");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [picked]);

  const lots = useMemo(() => {
    const s = Number(size);
    return Number.isFinite(s) && s > 0 ? Math.round(s / lot) : 0;
  }, [size, lot]);
  const limitPerLot = Number(px) > 0 ? toPricePerLot(Number(px), market) : 0n;
  const orderPerLot = kind === "market" ? marketLimit(market, side) : limitPerLot;
  // What the order is worth: a market order at the far side's best price, else its limit.
  const touch = long ? book?.asks[0]?.price : book?.bids[0]?.price;
  const refPerLot = kind === "market" ? BigInt(touch ?? market.oracle_price) : limitPerLot;
  const value = BigInt(lots) * refPerLot;
  const margin = (value * BigInt(market.imf_bps)) / 10_000n;
  const feeBps = kind === "post" ? market.maker_fee_bps : market.taker_fee_bps;
  const fee = (value * BigInt(feeBps)) / 10_000n;
  const free = account ? BigInt(account.free_collateral) : 0n;
  const perLotCost = (refPerLot * BigInt(market.imf_bps + feeBps)) / 10_000n;
  const maxLots = perLotCost > 0n ? Number(free / perLotCost) : 0;
  const capLots = Math.min(maxLots, market.max_position_lots);
  const rounded = lots > 0 && Math.abs(lots * lot - Number(size)) > lot / 1e6;
  const outOfBand = kind !== "market" && limitPerLot > 0n && Math.abs(Number(limitPerLot) - Number(market.oracle_price)) * 10_000 > market.band_bps * Number(market.oracle_price);

  const ready = !!ids && !!address && !!account;
  const canSubmit = ready && lots > 0 && orderPerLot > 0n && !busy && !outOfBand;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!canSubmit || !ids || !address) return;
    setBusy(true);
    setErr(null);
    const what = `${long ? "Long" : "Short"} ${baseAmount(lots, market)} ${b}`;
    try {
      const hash = await send(
        ids,
        address,
        { kind: "place_order", marketId: market.market_id, side, tif: kind === "market" ? Tif.Ioc : kind === "post" ? Tif.PostOnly : Tif.Gtc, reduceOnly: kind === "market" && reduceOnly, price: orderPerLot, lots: BigInt(lots), clientOrderId: BigInt(Date.now()) },
        sessionKey,
      );
      toast({ kind: "soft", title: `${what} sent`, body: `${kind === "market" ? "Market" : `Limit ${price(perUnit(orderPerLot, market))}`}. Queued in the lane (${short(hash, 6)}); it shows below within a block.` });
      setSize("");
    } catch (e2) {
      const text = explainReject(e2);
      setErr(text);
      toast({ kind: "err", title: `${what} rejected`, body: text });
    } finally {
      setBusy(false);
    }
  };

  const setPct = (pct: number) => {
    const l = Math.floor((capLots * pct) / 100);
    setSize(l > 0 ? baseAmount(l, market).replace(/,/g, "") : "");
  };

  return (
    <form className="ticket" onSubmit={(e) => void submit(e)} aria-label={`Order on ${market.symbol}`}>
      <div className="seg side" role="group" aria-label="Side">
        <button type="button" className="long" aria-pressed={long} onClick={() => setSide(Side.Buy)}>
          Long<span className="kbd" aria-hidden>B</span>
        </button>
        <button type="button" className="short" aria-pressed={!long} onClick={() => setSide(Side.Sell)}>
          Short<span className="kbd" aria-hidden>S</span>
        </button>
      </div>
      <div className="types" role="group" aria-label="Order type">
        {(
          [
            ["limit", "Limit", "Rests on the book until filled or cancelled"],
            ["market", "Market", "Fills now against the book; any rest is cancelled"],
            ["post", "Post-only", "Rejected if it would fill on arrival, so it always pays the maker fee"],
          ] as const
        ).map(([k, label, title]) => (
          <button type="button" key={k} aria-pressed={kind === k} title={title} onClick={() => setKind(k)}>
            {label}
          </button>
        ))}
      </div>

      {Date.now() - Number(market.oracle_time_ms) > STALE_MS && (
        <p className="msg warn">
          The oracle price has not updated for {ago(Number(market.oracle_time_ms)).replace(" ago", "")}. Orders are still checked against it, and positions are marked at it.
        </p>
      )}
      {kind === "market" ? (
        <p className="hint">
          Fills now at the best prices on the book, up to {price(perUnit(orderPerLot, market))} ({market.band_bps / 100}% band). Whatever does not fill is cancelled.
        </p>
      ) : (
        <div className="field">
          <span className="flabel">
            <label htmlFor="px">Price (USD)</label>
            <button
              type="button"
              className="btn ghost sm"
              style={{ height: 18, padding: "0 4px" }}
              onClick={() => {
                touched.current = false;
                setPx(price(oracle).replace(/,/g, ""));
              }}
            >
              Oracle
            </button>
          </span>
          <div className="input">
            <input
              id="px"
              inputMode="decimal"
              autoComplete="off"
              value={px}
              aria-invalid={outOfBand}
              onChange={(e) => {
                touched.current = true;
                setPx(e.target.value);
              }}
            />
            <span className="unit">USD</span>
          </div>
          {outOfBand && <span className="hint down">More than {market.band_bps / 100}% from the oracle price: the lane would reject it.</span>}
        </div>
      )}

      <div className="field">
        <span className="flabel">
          <label htmlFor="size">Size</label>
          <span className="faint num">
            Lot {lot} {b}
          </span>
        </span>
        <div className="input">
          <input id="size" inputMode="decimal" autoComplete="off" placeholder="0.00" value={size} onChange={(e) => setSize(e.target.value)} />
          <span className="unit">{b}</span>
        </div>
        <div className="pcts" role="group" aria-label="Size as a share of what you can open">
          {[25, 50, 75, 100].map((p) => (
            <button type="button" key={p} disabled={capLots <= 0} onClick={() => setPct(p)}>
              {p === 100 ? "Max" : `${p}%`}
            </button>
          ))}
        </div>
        {rounded && <span className="hint">Rounded to {lots} lots: {baseAmount(lots, market)} {b}.</span>}
      </div>

      {kind === "market" && (
        <label className="check">
          <input type="checkbox" checked={reduceOnly} onChange={(e) => setReduceOnly(e.target.checked)} />
          Reduce only: never opens or grows a position
        </label>
      )}

      <dl className="kv summary">
        <dt>Order value</dt>
        <dd className="ink">{lots > 0 ? `${usdc(value)} USDC` : "–"}</dd>
        <dt>Margin required</dt>
        <dd>{lots > 0 ? `${usdc(margin)} USDC` : "–"}</dd>
        <dt>{kind === "limit" ? "Fee (at most)" : "Fee"}</dt>
        <dd>{lots > 0 ? `${usdc(fee)} USDC` : `${feeBps / 100}%`}</dd>
        <dt>Available to trade</dt>
        <dd>{account ? `${usdc(free)} USDC` : "–"}</dd>
      </dl>

      {!address ? (
        <button type="button" className="btn stellar lg block" disabled={connecting} onClick={() => void connect()}>
          {connecting ? "Connecting…" : "Connect wallet to trade"}
        </button>
      ) : account === null ? (
        <Link to="/portfolio" className="btn stellar lg block" style={{ textDecoration: "none" }}>
          Deposit USDC to trade
        </Link>
      ) : (
        <button type="submit" className={`btn lg block ${long ? "buy" : "sell"}`} disabled={!canSubmit}>
          {busy ? (sessionKey ? "Sending…" : "Sign in your wallet…") : `${long ? "Long" : "Short"} ${lots > 0 ? `${baseAmount(lots, market)} ` : ""}${b}`}
        </button>
      )}
      {address && account && !sessionKey && !busy && <p className="hint">Each order asks your wallet to sign. Enable fast trading below to skip that.</p>}
      {err && <p className="msg err">{err}</p>}
    </form>
  );
}

/* ------------------------------------------------------------ account */

function AccountBox({ account, address, ids, hasKey, keyExpiry }: { account: Account | null | undefined; address: string | null; ids: LaneIds | null; hasKey: boolean; keyExpiry: number | null }) {
  const { setKey, connect, connecting } = useApp();
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const enable = async () => {
    if (!ids || !address) return;
    setBusy(true);
    setErr(null);
    try {
      setKey(await enableSessionKey(ids, address));
    } catch (e) {
      setErr(explainReject(e));
    } finally {
      setBusy(false);
    }
  };

  // First run: the four steps to a first trade, each done in place.
  if (!address || !account) {
    const step = !address ? 0 : account === null ? 1 : -1;
    return (
      <div className="account-box">
        <span className="label">Start trading</span>
        <ol className="steps">
          <li className={`step ${address ? "done" : step === 0 ? "now" : ""}`}>
            <div>
              <b>Connect a Stellar wallet</b>
              <span>Freighter, xBull, Lobstr and others, on testnet.</span>
              {step === 0 && (
                <div>
                  <button className="btn stellar sm" disabled={connecting} onClick={() => void connect()}>
                    {connecting ? "Connecting…" : "Connect wallet"}
                  </button>
                </div>
              )}
            </div>
          </li>
          <li className={`step ${step === 1 ? "now" : ""}`}>
            <div>
              <b>Deposit testnet USDC on Stellar</b>
              <span>It goes into the settlement contract; the lane credits it in seconds.</span>
              {step === 1 && (
                <div>
                  <Link to="/portfolio" className="btn stellar sm" style={{ textDecoration: "none" }}>
                    Deposit
                  </Link>
                </div>
              )}
            </div>
          </li>
          <li className="step">
            <div>
              <b>Enable fast trading</b>
              <span>Optional. One signature lets a key on this device sign orders for 24 h.</span>
            </div>
          </li>
          <li className="step">
            <div>
              <b>Place an order</b>
              <span>It fills on the lane at once, and settles on Stellar with the next checkpoint.</span>
            </div>
          </li>
        </ol>
        {address && account === undefined && <Skel w="100%" h={12} />}
      </div>
    );
  }

  const equity = BigInt(account.equity);
  const collateral = BigInt(account.collateral);
  const mm = BigInt(account.maintenance_margin);
  const ratio = equity > 0n ? Number((mm * 10_000n) / equity) / 100 : mm > 0n ? 100 : 0;
  return (
    <div className="account-box">
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        <span className="label">Account</span>
        <Chip kind="soft" title="Balances on the lane. They settle on Stellar with each accepted checkpoint.">
          soft
        </Chip>
      </div>
      <dl className="kv">
        <dt>Equity</dt>
        <dd className="ink">{usdc(equity)} USDC</dd>
        <dt>Unrealized PnL</dt>
        <dd>
          <Signed value={equity - collateral}>{usdc(equity - collateral)}</Signed>
        </dd>
        <dt>Free collateral</dt>
        <dd>{usdc(account.free_collateral)}</dd>
        <dt>Initial margin used</dt>
        <dd>{usdc(account.initial_margin)}</dd>
        <dt title="Maintenance margin over equity. At 100% the account can be liquidated.">Margin ratio</dt>
        <dd className={ratio >= 80 ? "down" : undefined}>{ratio.toFixed(2)}%</dd>
      </dl>
      <div className={`meter ${ratio >= 80 ? "crit" : ratio >= 50 ? "hot" : ""}`} role="meter" aria-label="Margin ratio" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.min(100, ratio)}>
        <i style={{ width: `${Math.min(100, ratio)}%` }} />
      </div>
      <div className="row">
        <Link to="/portfolio" className="btn sm" style={{ textDecoration: "none" }}>
          Deposit
        </Link>
        <Link to="/portfolio" className="btn sm" style={{ textDecoration: "none" }}>
          Withdraw
        </Link>
      </div>
      {hasKey ? (
        <p className="hint">
          <Chip kind="plain">Fast trading on</Chip> A key on this device signs orders until {keyExpiry ? new Date(keyExpiry).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) : "it expires"}.
        </p>
      ) : (
        <div style={{ display: "grid", gap: 6 }}>
          <button className="btn sm block" disabled={busy || !ids} onClick={() => void enable()}>
            {busy ? "Sign in your wallet…" : "Enable fast trading"}
          </button>
          <span className="hint">One wallet signature; a trading key on this device then signs orders and cancels for 24 h. Withdrawals always need your wallet.</span>
        </div>
      )}
      {err && <p className="msg err">{err}</p>}
    </div>
  );
}

/* ------------------------------------------------------------ positions, orders, your trades */

function BottomPane({ market, markets, account, fills, address, ids, sessionKey }: { market: Market; markets: Market[]; account: Account | null | undefined; fills: Fill[] | null; address: string | null; ids: LaneIds | null; sessionKey: SessionKey | null }) {
  const [tab, setTab] = useState<"positions" | "orders" | "trades">("positions");
  const toast = useToast();
  const [busy, setBusy] = useState<string | null>(null);
  const positions = account?.positions ?? [];
  const orders = account?.open_orders ?? [];
  const mine = (fills ?? []).filter((f) => address && (f.maker === address || f.taker === address));

  const run = async (id: string, label: string, body: Parameters<typeof send>[2]) => {
    if (!ids || !address) return;
    setBusy(id);
    try {
      const hash = await send(ids, address, body, sessionKey);
      toast({ kind: "soft", title: label, body: `Queued in the lane (${short(hash, 6)}).` });
    } catch (e) {
      toast({ kind: "err", title: `${label}: rejected`, body: explainReject(e) });
    } finally {
      setBusy(null);
    }
  };

  const signedOut = !address ? (
    <Empty title="Connect a wallet">Your positions, orders and trades show here.</Empty>
  ) : account === null ? (
    <Empty title="No lane account yet" action={<Link to="/portfolio" className="btn sm stellar" style={{ textDecoration: "none" }}>Deposit USDC</Link>}>
      Deposit testnet USDC on Stellar to open one.
    </Empty>
  ) : account === undefined ? (
    <div style={{ padding: 16, display: "grid", gap: 10 }}>
      <Skel w="100%" />
      <Skel w="80%" />
    </div>
  ) : null;

  return (
    <section className="pane bottom-pane" aria-label="Your positions and orders">
      <Tabs
        label="Your account"
        value={tab}
        onChange={setTab}
        tabs={[
          { id: "positions", label: "Positions", count: positions.length },
          { id: "orders", label: "Open orders", count: orders.length },
          { id: "trades", label: "Your trades" },
        ]}
        end={
          tab === "orders" && orders.some((o) => o.market_id === market.market_id) ? (
            <button className="btn ghost sm" disabled={busy !== null} onClick={() => void run("all", `Cancel all on ${market.symbol}`, { kind: "cancel_all", marketId: market.market_id })}>
              Cancel all on {market.symbol}
            </button>
          ) : undefined
        }
      />
      <div className="pane-body">
        {signedOut ??
          (tab === "positions" ? (
            positions.length === 0 ? (
              <Empty title="No open positions">Place an order to open one. Positions are marked at the oracle price.</Empty>
            ) : (
              <table className="table">
                <thead>
                  <tr>
                    <th>Market</th>
                    <th className="r">Size</th>
                    <th className="r">Value</th>
                    <th className="r">Entry</th>
                    <th className="r">Mark</th>
                    <th className="r">Unrealized PnL</th>
                    <th className="r">Liq. price (est.)</th>
                    <th className="r">
                      <span className="sr-only">Close</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {positions.map((p) => {
                    const m = markets.find((x) => x.market_id === p.market_id);
                    if (!m) return null;
                    const lots = Math.abs(p.lots);
                    const long = p.lots > 0;
                    const id = `close-${p.market_id}`;
                    return (
                      <tr key={p.market_id}>
                        <td>
                          <Link to={`/trade/${m.symbol}`} style={{ textDecoration: "none", fontWeight: 600 }}>
                            {p.symbol}
                          </Link>{" "}
                          <SidePill long={long} />
                        </td>
                        <td className="r">
                          {baseAmount(lots, m)} {base(m.symbol)}
                        </td>
                        <td className="r">{usdc(BigInt(lots) * BigInt(p.mark_price))}</td>
                        <td className="r">{p.entry_price ? price(perUnit(p.entry_price, m)) : "–"}</td>
                        <td className="r">{price(perUnit(p.mark_price, m))}</td>
                        <td className="r">
                          <Signed value={BigInt(p.upnl)}>{usdc(p.upnl)}</Signed>
                        </td>
                        <td className="r dim">{p.liq_price ? price(perUnit(p.liq_price, m)) : "–"}</td>
                        <td className="r">
                          <button
                            className="btn sm"
                            disabled={busy !== null || !ids}
                            title="A reduce-only market order for the whole position"
                            onClick={() =>
                              void run(id, `Close ${p.symbol} ${long ? "long" : "short"}`, {
                                kind: "place_order",
                                marketId: m.market_id,
                                side: long ? Side.Sell : Side.Buy,
                                tif: Tif.Ioc,
                                reduceOnly: true,
                                price: marketLimit(m, long ? Side.Sell : Side.Buy),
                                lots: BigInt(lots),
                                clientOrderId: BigInt(Date.now()),
                              })
                            }
                          >
                            {busy === id ? "Closing…" : "Close"}
                          </button>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            )
          ) : tab === "orders" ? (
            orders.length === 0 ? (
              <Empty title="No open orders">Limit and post-only orders rest here until they fill or you cancel them.</Empty>
            ) : (
              <table className="table">
                <thead>
                  <tr>
                    <th>Market</th>
                    <th>Side</th>
                    <th className="r">Price</th>
                    <th className="r">Remaining</th>
                    <th className="r">Value</th>
                    <th className="r">Order id</th>
                    <th className="r">
                      <span className="sr-only">Cancel</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {orders.map((o) => {
                    const m = markets.find((x) => x.market_id === o.market_id);
                    if (!m) return null;
                    const id = `cancel-${o.order_id}`;
                    return (
                      <tr key={o.order_id}>
                        <td>{m.symbol}</td>
                        <td>
                          <SidePill long={o.side === "buy"} />
                        </td>
                        <td className="r">{price(perUnit(o.price, m))}</td>
                        <td className="r">
                          {baseAmount(o.lots_remaining, m)} {base(m.symbol)}
                        </td>
                        <td className="r">{usdc(BigInt(o.lots_remaining) * BigInt(o.price))}</td>
                        <td className="r faint">{o.order_id}</td>
                        <td className="r">
                          <button className="btn ghost sm" disabled={busy !== null} onClick={() => void run(id, `Cancel order ${o.order_id}`, { kind: "cancel_order", marketId: m.market_id, orderId: BigInt(o.order_id) })}>
                            {busy === id ? "Cancelling…" : "Cancel"}
                          </button>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            )
          ) : mine.length === 0 ? (
            <Empty title={`No trades of yours among the latest ${market.symbol} trades`}>Your fills show here as the lane matches them.</Empty>
          ) : (
            <table className="table">
              <thead>
                <tr>
                  <th>Time</th>
                  <th>Side</th>
                  <th className="r">Price</th>
                  <th className="r">Size</th>
                  <th>Role</th>
                  <th className="r">Lane block</th>
                </tr>
              </thead>
              <tbody>
                {mine.map((f, i) => {
                  const taker = f.taker === address;
                  const bought = taker ? f.taker_side === "buy" : f.taker_side === "sell";
                  return (
                    <tr key={`${f.height}-${i}`}>
                      <td className="dim">{new Date(Number(f.timestamp_ms)).toLocaleTimeString("en-GB")}</td>
                      <td>
                        <span className={bought ? "up" : "down"}>{bought ? "Buy" : "Sell"}</span>
                      </td>
                      <td className="r">{price(perUnit(f.price, market))}</td>
                      <td className="r">
                        {baseAmount(f.lots, market)} {base(market.symbol)}
                      </td>
                      <td className="dim">{taker ? "Taker" : "Maker"}</td>
                      <td className="r">
                        <Chip kind="soft">#{f.height}</Chip>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          ))}
      </div>
    </section>
  );
}
