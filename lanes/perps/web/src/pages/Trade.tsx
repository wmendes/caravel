import { useEffect, useMemo, useRef, useState } from "react";

import { lane, stream, type Account, type Book, type Fill, type Market } from "../api/lane";
import { Side, Tif, type TxBody } from "../codec/tx";
import { PriceChart } from "../components/Chart";
import { base, baseAmount, perUnit, price, short, toPricePerLot, usdc } from "../format";
import { Link } from "../router";
import { useApp, usePoll } from "../state";
import { enableSessionKey, explainReject, laneIds, send } from "../trade";

export function Trade({ symbol }: { symbol: string }) {
  const { markets, status, address, key, setKey } = useApp();
  const market = markets.find((m) => m.symbol === symbol) ?? markets[0];
  const [book, setBook] = useState<Book | null>(null);
  const [fills, setFills] = useState<Fill[]>([]);
  const [account, setAccount] = useState<Account | null>(null);
  const [points, setPoints] = useState<{ t: number; v: number }[]>([]);
  const marketId = market?.market_id;

  // Price history for the chart, from the oracle price the lane holds.
  useEffect(() => {
    if (!market) return;
    const v = perUnit(market.oracle_price, market);
    if (v > 0) setPoints((p) => [...p.slice(-600), { t: Date.now(), v }]);
  }, [market?.oracle_price, market]);
  useEffect(() => setPoints([]), [marketId]);

  useEffect(() => {
    if (marketId === undefined) return;
    void lane.book(marketId).then(setBook).catch(() => {});
    void lane.trades(marketId, 40).then(setFills).catch(() => {});
    return stream({ markets: [marketId], account: address }, (m) => {
      if (m.type === "book" && m.market_id === marketId) setBook(m as unknown as Book);
      if (m.type === "fill" && m.market_id === marketId) setFills((f) => [m as unknown as Fill, ...f].slice(0, 40));
      if (m.type === "account") setAccount(m as unknown as Account);
    });
  }, [marketId, address]);

  usePoll(async () => {
    if (address) setAccount(await lane.account(address));
  }, 5000, [address]);

  if (!market) return <p className="muted">Loading markets…</p>;
  const mark = perUnit(market.oracle_price, market);

  return (
    <>
      <div className="page-head">
        <div className="markets" role="tablist">
          {markets.map((m) => (
            <Link key={m.market_id} to={`/trade/${m.symbol}`} className="market-tab">
              <b>{m.symbol}</b>
              <span className="num">${price(perUnit(m.oracle_price, m))}</span>
            </Link>
          ))}
        </div>
        <SessionControl ids={laneIds(status)} address={address} hasKey={key !== null} onKey={setKey} />
      </div>
      <div className="trade">
        <div className="stack">
          <section className="panel">
            <div className="spread" style={{ marginBottom: 8 }}>
              <h2>
                {market.symbol} <span className="num">${price(mark)}</span>
              </h2>
              <span className="note">Oracle price, signed by the Caravel team's oracle key</span>
            </div>
            <PriceChart points={points} />
          </section>
          <Positions account={account} markets={markets} />
          <OpenOrders account={account} market={market} ids={laneIds(status)} address={address} sessionKey={key} />
        </div>
        <div className="stack">
          <BookView book={book} market={market} />
          <Fills fills={fills} market={market} />
        </div>
        <div className="stack form-col">
          <OrderForm market={market} ids={laneIds(status)} address={address} sessionKey={key} account={account} />
        </div>
      </div>
    </>
  );
}

function SessionControl({ ids, address, hasKey, onKey }: { ids: ReturnType<typeof laneIds>; address: string | null; hasKey: boolean; onKey: (k: Awaited<ReturnType<typeof enableSessionKey>>) => void }) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  if (!address) return null;
  if (hasKey) return <span className="badge lane" title="Orders are signed on this device without a wallet prompt">Fast trading on: trading key on this device</span>;
  return (
    <div className="row">
      <button
        className="btn small"
        disabled={busy || !ids}
        onClick={async () => {
          if (!ids) return;
          setBusy(true);
          setErr(null);
          try {
            onKey(await enableSessionKey(ids, address));
          } catch (e) {
            setErr(explainReject(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        {busy ? "Waiting for your wallet…" : "Enable fast trading"}
      </button>
      {err ? <span className="error">{err}</span> : <span className="note">One wallet signature; a trading key on this device signs orders for 24 h.</span>}
    </div>
  );
}

function BookView({ book, market }: { book: Book | null; market: Market }) {
  const max = Math.max(1, ...(book?.bids ?? []).map((l) => l.lots), ...(book?.asks ?? []).map((l) => l.lots));
  const row = (l: { price: string; lots: number }, side: "bid" | "ask") => (
    <div className={`book-row ${side}`} key={`${side}${l.price}`}>
      <span className="depth" style={{ width: `${(l.lots / max) * 100}%` }} />
      <span>{price(perUnit(l.price, market))}</span>
      <span>{baseAmount(l.lots, market)}</span>
    </div>
  );
  return (
    <section className="panel book" aria-label="Order book">
      <h3>Order book</h3>
      <div className="book-row muted small">
        <span>Price (USD)</span>
        <span>Size ({base(market.symbol)})</span>
      </div>
      {book?.asks.length ? [...book.asks].slice(0, 12).reverse().map((l) => row(l, "ask")) : <p className="empty">No asks</p>}
      <div className="book-mid num">${price(perUnit(market.oracle_price, market))} oracle</div>
      {book?.bids.length ? book.bids.slice(0, 12).map((l) => row(l, "bid")) : <p className="empty">No bids</p>}
    </section>
  );
}

function Fills({ fills, market }: { fills: Fill[]; market: Market }) {
  return (
    <section className="panel">
      <h3>Recent fills</h3>
      {fills.length === 0 ? (
        <p className="empty">No fills yet</p>
      ) : (
        <table>
          <tbody>
            {fills.slice(0, 15).map((f, i) => (
              <tr key={`${f.height}-${i}`}>
                <td>{price(perUnit(f.price, market))}</td>
                <td className="muted small">{f.taker_side === "buy" ? "buy" : "sell"}</td>
                <td className="r">{baseAmount(f.lots, market)}</td>
                <td className="r muted small">#{f.height}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

function Positions({ account, markets }: { account: Account | null; markets: Market[] }) {
  return (
    <section className="panel">
      <h3>Positions</h3>
      {!account || account.positions.length === 0 ? (
        <p className="empty">{account ? "No open positions" : "Connect and deposit to trade"}</p>
      ) : (
        <table>
          <thead>
            <tr>
              <th>Market</th>
              <th className="r">Size</th>
              <th className="r">Entry</th>
              <th className="r">Mark</th>
              <th className="r">uPnL (USDC)</th>
              <th className="r">Liq. (est.)</th>
            </tr>
          </thead>
          <tbody>
            {account.positions.map((p) => {
              const m = markets.find((x) => x.market_id === p.market_id);
              if (!m) return null;
              return (
                <tr key={p.market_id}>
                  <td>{p.symbol}</td>
                  <td className="r">
                    {p.lots > 0 ? "long " : "short "}
                    {baseAmount(Math.abs(p.lots), m)}
                  </td>
                  <td className="r">{p.entry_price ? price(perUnit(p.entry_price, m)) : "–"}</td>
                  <td className="r">{price(perUnit(p.mark_price, m))}</td>
                  <td className="r">{BigInt(p.upnl) > 0n ? "+" : ""}{usdc(p.upnl)}</td>
                  <td className="r muted">{p.liq_price ? price(perUnit(p.liq_price, m)) : "–"}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </section>
  );
}

function OpenOrders({ account, market, ids, address, sessionKey }: { account: Account | null; market: Market; ids: ReturnType<typeof laneIds>; address: string | null; sessionKey: Parameters<typeof send>[3] }) {
  const [err, setErr] = useState<string | null>(null);
  const orders = account?.open_orders.filter((o) => o.market_id === market.market_id) ?? [];
  const cancel = async (body: TxBody) => {
    if (!ids || !address) return;
    setErr(null);
    try {
      await send(ids, address, body, sessionKey);
    } catch (e) {
      setErr(explainReject(e));
    }
  };
  return (
    <section className="panel">
      <div className="spread">
        <h3>Open orders</h3>
        {orders.length > 0 && (
          <button className="btn small" onClick={() => void cancel({ kind: "cancel_all", marketId: market.market_id })}>
            Cancel all
          </button>
        )}
      </div>
      {err && <p className="error">{err}</p>}
      {orders.length === 0 ? (
        <p className="empty">No open orders on {market.symbol}</p>
      ) : (
        <table>
          <tbody>
            {orders.map((o) => (
              <tr key={o.order_id}>
                <td>{o.side}</td>
                <td className="r">{price(perUnit(o.price, market))}</td>
                <td className="r">{baseAmount(o.lots_remaining, market)}</td>
                <td className="r">
                  <button className="btn small" onClick={() => void cancel({ kind: "cancel_order", marketId: market.market_id, orderId: BigInt(o.order_id) })}>
                    Cancel
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

function OrderForm({ market, ids, address, sessionKey, account }: { market: Market; ids: ReturnType<typeof laneIds>; address: string | null; sessionKey: Parameters<typeof send>[3]; account: Account | null }) {
  const [side, setSide] = useState<Side>(Side.Buy);
  const [tif, setTif] = useState<Tif>(Tif.Gtc);
  const [reduceOnly, setReduceOnly] = useState(false);
  const [px, setPx] = useState("");
  const [size, setSize] = useState("");
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const touched = useRef(false);
  const mark = perUnit(market.oracle_price, market);
  useEffect(() => {
    touched.current = false;
    setPx("");
  }, [market.market_id]);
  useEffect(() => {
    if (!touched.current && mark > 0) setPx(price(mark).replace(/,/g, ""));
  }, [mark]);
  const unitsPerLot = market.display_lot_base_units / 10 ** market.display_base_decimals;
  const lots = useMemo(() => {
    const s = Number(size);
    return Number.isFinite(s) && s > 0 ? Math.round(s / unitsPerLot) : 0;
  }, [size, unitsPerLot]);
  const pricePerLot = Number(px) > 0 ? toPricePerLot(Number(px), market) : 0n;
  const notional = BigInt(lots) * pricePerLot;
  const canSubmit = !!ids && !!address && !!account && lots > 0 && pricePerLot > 0n && !busy;

  const submit = async () => {
    if (!ids || !address) return;
    setBusy(true);
    setMsg(null);
    try {
      const hash = await send(ids, address, { kind: "place_order", marketId: market.market_id, side, tif, reduceOnly: tif === Tif.Ioc && reduceOnly, price: pricePerLot, lots: BigInt(lots), clientOrderId: BigInt(Date.now()) }, sessionKey);
      setMsg({ ok: true, text: `Queued in the lane (${short(hash, 6)}). It shows in positions or open orders within a block.` });
    } catch (e) {
      setMsg({ ok: false, text: explainReject(e) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="panel stack">
      <h3>Place order</h3>
      <div className="seg" role="group" aria-label="Side">
        <button className="buy" aria-pressed={side === Side.Buy} onClick={() => setSide(Side.Buy)}>
          Buy / long
        </button>
        <button className="sell" aria-pressed={side === Side.Sell} onClick={() => setSide(Side.Sell)}>
          Sell / short
        </button>
      </div>
      <div className="seg" role="group" aria-label="Order type">
        {([
          [Tif.Gtc, "Limit"],
          [Tif.Ioc, "IOC"],
          [Tif.PostOnly, "Post-only"],
        ] as const).map(([t, label]) => (
          <button key={t} aria-pressed={tif === t} onClick={() => setTif(t)}>
            {label}
          </button>
        ))}
      </div>
      <div className="field">
        <label htmlFor="px">Price (USD per {base(market.symbol)})</label>
        <input
          id="px"
          inputMode="decimal"
          value={px}
          onChange={(e) => {
            touched.current = true;
            setPx(e.target.value);
          }}
        />
      </div>
      <div className="field">
        <label htmlFor="size">Size ({base(market.symbol)}), lot {unitsPerLot}</label>
        <input id="size" inputMode="decimal" value={size} onChange={(e) => setSize(e.target.value)} placeholder={String(unitsPerLot)} />
      </div>
      {tif === Tif.Ioc && (
        <label className="row small">
          <input type="checkbox" checked={reduceOnly} onChange={(e) => setReduceOnly(e.target.checked)} /> Reduce only
        </label>
      )}
      <dl className="kv">
        <dt>Lots</dt>
        <dd className="num">{lots}</dd>
        <dt>Notional</dt>
        <dd className="num">{usdc(notional)} USDC</dd>
        <dt>Initial margin</dt>
        <dd className="num">{usdc((notional * BigInt(market.imf_bps)) / 10_000n)} USDC</dd>
        <dt>Free collateral</dt>
        <dd className="num">{account ? `${usdc(account.free_collateral)} USDC` : "–"}</dd>
      </dl>
      <button className="btn primary" disabled={!canSubmit} onClick={() => void submit()}>
        {busy ? (sessionKey ? "Sending…" : "Waiting for your wallet…") : `${side === Side.Buy ? "Buy" : "Sell"} ${lots > 0 ? baseAmount(lots, market) : ""} ${base(market.symbol)}`}
      </button>
      {!address && <p className="note">Connect a wallet to trade.</p>}
      {address && !account && (
        <p className="note">
          No lane account yet. <Link to="/portfolio">Deposit USDC</Link> first.
        </p>
      )}
      {msg && <p className={msg.ok ? "ok" : "error"}>{msg.text}</p>}
      <p className="note">Fees: taker {market.taker_fee_bps} bps, maker {market.maker_fee_bps} bps. Orders must be within {market.band_bps / 100}% of the oracle price.</p>
    </section>
  );
}
