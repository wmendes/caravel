/**
 * The account: where every dollar is (on Stellar or on the lane), what it is
 * doing, and the actions that move it: get test USDC, deposit, withdraw,
 * claim (DEC-106).
 */
import { useState, type ReactNode } from "react";

import { lane, type Account, type Proof } from "../api/lane";
import { explain, stellar } from "../api/stellar";
import { FundPanel } from "../components/Fund";
import { Chip, Empty, SidePill, Signed, Skel, Tabs, useToast } from "../components/ui";
import { config } from "../config";
import { base, baseAmount, parseUsdc, perUnit, price, short, usdc } from "../format";
import { Link } from "../router";
import { useApp, usePoll } from "../state";
import { explainReject, laneIds, send, type LaneIds } from "../trade";

type Action = "fund" | "deposit" | "withdraw";
type Claim = Proof & { claimed: boolean };

export function Portfolio() {
  const { address, connect, connecting, status, account } = useApp();
  const [wallet, setWallet] = useState<bigint | null | undefined>(undefined);
  const [claims, setClaims] = useState<Claim[] | null>(null);
  const [action, setAction] = useState<Action | null>(null);
  const [depositAmount, setDepositAmount] = useState("");

  usePoll(async () => {
    if (!address) return;
    const [w, p] = await Promise.all([stellar.usdcBalance(address), lane.withdrawalProofs(address).catch(() => ({ withdrawals: [] as Proof[] }))]);
    setWallet(w);
    setClaims(await Promise.all(p.withdrawals.map(async (x) => ({ ...x, claimed: await stellar.isClaimed(x.seq, x.index).catch(() => false) }))));
  }, 5000, [address]);

  if (!address) {
    return (
      <div className="page">
        <header className="page-head">
          <div>
            <h1>Portfolio</h1>
            <p>Your USDC on Stellar, your account on the lane, and withdrawals ready to claim.</p>
          </div>
        </header>
        <section className="section">
          <Empty
            title="Connect a wallet to see your account"
            action={
              <button className="btn stellar" disabled={connecting} onClick={() => void connect()}>
                {connecting ? "Connecting…" : "Connect wallet"}
              </button>
            }
          >
            Any Stellar wallet on testnet. New to testnet? Once connected, Get test USDC funds you in one signature.
          </Empty>
        </section>
      </div>
    );
  }

  // Default action: fund an empty wallet, otherwise deposit.
  const current: Action = action ?? ((wallet === null || wallet === 0n) && !account ? "fund" : "deposit");
  const open = claims?.filter((c) => !c.claimed) ?? [];
  const claimable = open.reduce((s, c) => s + BigInt(c.amount ?? "0"), 0n);
  const equity = account ? BigInt(account.equity) : null;
  const upnl = account ? BigInt(account.equity) - BigInt(account.collateral) : null;
  const mm = account ? BigInt(account.maintenance_margin) : 0n;
  const ratio = equity && equity > 0n ? Number((mm * 10_000n) / equity) / 100 : 0;

  return (
    <div className="page">
      <header className="acct-head">
        <div className="acct-id">
          <span className="label">Account</span>
          <AddressLine address={address} />
        </div>
        <div className="acct-figs">
          <Fig k="Equity" chip="soft" v={account === undefined ? <Skel w={110} h={22} /> : account === null ? "No deposit yet" : `${usdc(equity)} USDC`} big />
          <Fig k="Unrealized PnL" v={upnl === null ? "–" : <Signed value={upnl}>{usdc(upnl)}</Signed>} />
          <Fig k="Margin ratio" v={account ? `${ratio.toFixed(2)}%` : "–"} hint="Maintenance margin over equity; at 100% the account can be liquidated." />
          <Fig k="Ready to claim" chip="settled" v={claims === null ? <Skel w={70} h={18} /> : `${usdc(claimable)} USDC`} />
        </div>
      </header>

      <div className="acct-grid">
        <div className="acct-main">
          <section className="section" aria-label="Balances">
            <div className="section-head">
              <h3>Where your USDC is</h3>
              <span className="hint">Settled is on Stellar; soft is on the lane until the next checkpoint.</span>
            </div>
            <div className="table-wrap">
              <table className="table ledger">
                <tbody>
                  <Row where="Your Stellar wallet" chip="settled" amount={wallet === undefined ? undefined : wallet} note={wallet === null ? "No USDC trustline yet" : "Yours to deposit"} />
                  <Row where="Lane collateral" chip="soft" amount={account === undefined ? undefined : account ? BigInt(account.collateral) : null} note="Deposits, less withdrawals, plus realized PnL and fees" />
                  <Row where="Held as margin" chip="soft" amount={account === undefined ? undefined : account ? BigInt(account.initial_margin) : null} note="Initial margin of positions and open orders" />
                  <Row where="Free to withdraw" chip="soft" amount={account === undefined ? undefined : account ? BigInt(account.free_collateral) : null} note="Claimable on Stellar after the next checkpoint" strong />
                  <Row where="Claimable on Stellar" chip="settled" amount={claims === null ? undefined : claimable} note={open.length ? `${open.length} withdrawal${open.length > 1 ? "s" : ""} ready` : "Nothing waiting"} />
                </tbody>
              </table>
            </div>
          </section>

          <Holdings account={account} claims={claims} address={address} />

          <details className="section">
            <summary>Forced withdrawal through Stellar</summary>
            <ForcedWithdrawal address={address} />
          </details>
        </div>

        <aside className="acct-side" aria-label="Move funds">
          <section className="section">
            <Tabs
              label="Move funds"
              value={current}
              onChange={setAction}
              tabs={[
                { id: "fund", label: "Get test USDC" },
                { id: "deposit", label: "Deposit" },
                { id: "withdraw", label: "Withdraw" },
              ]}
            />
            <div className="section-body">
              {current === "fund" && (
                <FundPanel
                  address={address}
                  wallet={wallet}
                  onFunded={(v) => {
                    setWallet((w) => (typeof w === "bigint" ? w + v : v));
                    setDepositAmount(usdc(v, 0).replace(/,/g, ""));
                    setAction("deposit");
                  }}
                />
              )}
              {current === "deposit" && <Deposit key={depositAmount} address={address} wallet={wallet} initial={depositAmount} onNeedFunds={() => setAction("fund")} />}
              {current === "withdraw" && <Withdraw address={address} ids={laneIds(status)} account={account ?? null} />}
            </div>
          </section>
          <p className="hint side-note">
            Money moves in three steps: deposit on Stellar, trade on the lane, then withdraw and claim on Stellar once a checkpoint is accepted. <Link to="/escape">If the lane stops</Link>, you can still exit on Stellar.
          </p>
        </aside>
      </div>
    </div>
  );
}

function AddressLine({ address }: { address: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <span className="addr">
      <span className="mono">{short(address, 8)}</span>
      <button
        type="button"
        className="btn ghost sm"
        onClick={() => {
          void navigator.clipboard?.writeText(address).then(() => {
            setCopied(true);
            setTimeout(() => setCopied(false), 1200);
          });
        }}
      >
        {copied ? "Copied" : "Copy"}
      </button>
      <a className="btn ghost sm" href={`${config.explorerUrl}/account/${address}`} target="_blank" rel="noreferrer">
        stellar.expert ↗
      </a>
    </span>
  );
}

function Fig({ k, v, chip, hint, big }: { k: string; v: ReactNode; chip?: "soft" | "settled"; hint?: string; big?: boolean }) {
  return (
    <div className={`fig ${big ? "big" : ""}`}>
      <span className="k" title={hint}>
        {k} {chip && <Chip kind={chip}>{chip}</Chip>}
      </span>
      <span className="v num">{v}</span>
    </div>
  );
}

function Row({ where, chip, amount, note, strong }: { where: string; chip: "soft" | "settled"; amount: bigint | null | undefined; note: string; strong?: boolean }) {
  return (
    <tr>
      <td>
        <span className="where">
          <Chip kind={chip}>{chip}</Chip> <span className={strong ? "ink" : undefined}>{where}</span>
        </span>
      </td>
      <td className={`r num ${strong ? "ink" : ""}`}>{amount === undefined ? <Skel w={80} /> : amount === null ? "–" : `${usdc(amount)} USDC`}</td>
      <td className="faint note">{note}</td>
    </tr>
  );
}

function Holdings({ account, claims, address }: { account: Account | null | undefined; claims: Claim[] | null; address: string }) {
  const { markets } = useApp();
  const [tab, setTab] = useState<"positions" | "orders" | "claims">("positions");
  const positions = account?.positions ?? [];
  const orders = account?.open_orders ?? [];
  const open = claims?.filter((c) => !c.claimed) ?? [];
  const { busy, msg, run } = useAction();
  return (
    <section className="section">
      <Tabs
        label="Holdings"
        value={tab}
        onChange={setTab}
        tabs={[
          { id: "positions", label: "Positions", count: positions.length },
          { id: "orders", label: "Open orders", count: orders.length },
          { id: "claims", label: "Claims", count: open.length },
        ]}
      />
      {tab === "positions" &&
        (positions.length === 0 ? (
          <Empty
            title="No open positions"
            action={
              <Link to="/trade/BTC-PERP" className="btn sm" style={{ textDecoration: "none" }}>
                Trade
              </Link>
            }
          />
        ) : (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Market</th>
                  <th className="r">Size</th>
                  <th className="r">Entry</th>
                  <th className="r">Mark</th>
                  <th className="r">Unrealized PnL</th>
                  <th className="r">Liq. (est.)</th>
                </tr>
              </thead>
              <tbody>
                {positions.map((p) => {
                  const m = markets.find((x) => x.market_id === p.market_id);
                  if (!m) return null;
                  return (
                    <tr key={p.market_id}>
                      <td>
                        <Link to={`/trade/${m.symbol}`} style={{ textDecoration: "none", fontWeight: 600 }}>
                          {p.symbol}
                        </Link>{" "}
                        <SidePill long={p.lots > 0} />
                      </td>
                      <td className="r">
                        {baseAmount(Math.abs(p.lots), m)} {base(m.symbol)}
                      </td>
                      <td className="r">{p.entry_price ? price(perUnit(p.entry_price, m)) : "–"}</td>
                      <td className="r">{price(perUnit(p.mark_price, m))}</td>
                      <td className="r">
                        <Signed value={BigInt(p.upnl)}>{usdc(p.upnl)}</Signed>
                      </td>
                      <td className="r dim">{p.liq_price ? price(perUnit(p.liq_price, m)) : "–"}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        ))}
      {tab === "orders" &&
        (orders.length === 0 ? (
          <Empty title="No open orders">Limit and post-only orders rest here until they fill or you cancel them on the Trade page.</Empty>
        ) : (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Market</th>
                  <th>Side</th>
                  <th className="r">Price</th>
                  <th className="r">Remaining</th>
                </tr>
              </thead>
              <tbody>
                {orders.map((o) => {
                  const m = markets.find((x) => x.market_id === o.market_id);
                  if (!m) return null;
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
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        ))}
      {tab === "claims" &&
        (claims === null ? (
          <div className="section-body">
            <Skel w="100%" />
          </div>
        ) : open.length === 0 ? (
          <Empty title="Nothing to claim">A withdrawal shows here once its checkpoint is accepted on Stellar, about a minute after you make it.</Empty>
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>Checkpoint</th>
                <th className="r">Amount</th>
                <th className="r">
                  <span className="sr-only">Claim</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {open.map((c) => (
                <tr key={`${c.seq}-${c.index}`}>
                  <td>
                    <Chip kind="settled">#{c.seq}</Chip>
                  </td>
                  <td className="r">{usdc(c.amount ?? "0")} USDC</td>
                  <td className="r">
                    <button
                      className="btn sm stellar"
                      disabled={busy}
                      onClick={() =>
                        void run(async () => {
                          const hash = await stellar.claimWithdrawal(address, { seq: c.seq, index: c.index, amount: c.amount ?? "0", proof: c.proof });
                          return `Claimed on Stellar (${short(hash, 6)}).`;
                        })
                      }
                    >
                      Claim
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        ))}
      {msg && (
        <div className="section-body">
          <p className={`msg ${msg.ok ? "ok" : "err"}`}>{msg.text}</p>
        </div>
      )}
    </section>
  );
}

function useAction() {
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const run = async (f: () => Promise<string>, explainErr: (e: unknown) => string = explain) => {
    setBusy(true);
    setMsg(null);
    try {
      const text = await f();
      setMsg({ ok: true, text });
      toast({ kind: "ok", title: "Done", body: text });
    } catch (e) {
      setMsg({ ok: false, text: explainErr(e) });
    } finally {
      setBusy(false);
    }
  };
  return { busy, msg, run };
}

function Amount({ id, value, onChange, placeholder, max }: { id: string; value: string; onChange: (v: string) => void; placeholder?: string; max?: bigint | null }) {
  return (
    <div className="input">
      <input id={id} inputMode="decimal" autoComplete="off" value={value} onChange={(e) => onChange(e.target.value)} placeholder={placeholder ?? "0.00"} />
      {max !== undefined && max !== null && max > 0n && (
        <button type="button" className="btn ghost sm" onClick={() => onChange(usdc(max, 7).replace(/,/g, "").replace(/\.?0+$/, ""))}>
          Max
        </button>
      )}
      <span className="unit">USDC</span>
    </div>
  );
}

function Deposit({ address, wallet, initial, onNeedFunds }: { address: string; wallet: bigint | null | undefined; initial: string; onNeedFunds: () => void }) {
  const [amount, setAmount] = useState(initial);
  const { busy, msg, run } = useAction();
  const v = parseUsdc(amount);
  const tooMuch = v !== null && typeof wallet === "bigint" && v > wallet;
  return (
    <div className="action">
      <div className="field">
        <span className="flabel">
          <label htmlFor="dep">Deposit into the lane</label>
          <span className="faint num">Wallet: {typeof wallet === "bigint" ? `${usdc(wallet)} USDC` : "–"}</span>
        </span>
        <Amount id="dep" value={amount} onChange={setAmount} placeholder="100" max={wallet} />
        {tooMuch && <span className="hint down">More than your wallet holds.</span>}
      </div>
      <button
        className="btn stellar lg block"
        disabled={busy || v === null || v < 10_000_000n || tooMuch}
        onClick={() =>
          void run(async () => {
            const hash = await stellar.deposit(address, v!);
            setAmount("");
            return `Deposited on Stellar (${short(hash, 6)}). The lane credits it within seconds.`;
          })
        }
      >
        {busy ? "Sign in your wallet…" : "Deposit on Stellar"}
      </button>
      {(wallet === null || wallet === 0n) && (
        <p className="hint">
          No USDC in your wallet yet.{" "}
          <button type="button" className="linkish" onClick={onNeedFunds}>
            Get test USDC
          </button>
        </p>
      )}
      <p className="hint">At least 1 USDC. It moves into the settlement contract on Stellar; the relayer reports it to the lane, which credits it within seconds.</p>
      {msg && <p className={`msg ${msg.ok ? "ok" : "err"}`}>{msg.text}</p>}
    </div>
  );
}

function Withdraw({ address, ids, account }: { address: string; ids: LaneIds | null; account: Account | null }) {
  const [amount, setAmount] = useState("");
  const { busy, msg, run } = useAction();
  const v = parseUsdc(amount);
  const free = account ? BigInt(account.free_collateral) : null;
  const tooMuch = v !== null && free !== null && v > free;
  return (
    <div className="action">
      <div className="field">
        <span className="flabel">
          <label htmlFor="wd">Withdraw from the lane</label>
          <span className="faint num">Free: {free !== null ? `${usdc(free)} USDC` : "–"}</span>
        </span>
        <Amount id="wd" value={amount} onChange={setAmount} max={free} />
        {tooMuch && <span className="hint down">More than your free collateral.</span>}
      </div>
      <button
        className="btn lg block"
        disabled={busy || !ids || !account || v === null || v <= 0n || tooMuch}
        onClick={() =>
          void run(async () => {
            const hash = await send(ids!, address, { kind: "withdraw", amount: v! }, null);
            setAmount("");
            return `Withdrawal queued in the lane (${short(hash, 6)}). It shows under Claims once its checkpoint is accepted on Stellar.`;
          }, explainReject)
        }
      >
        {busy ? "Sign in your wallet…" : "Withdraw"}
      </button>
      <p className="hint">Signed in your wallet. The amount leaves your lane collateral now and becomes claimable on Stellar after the next accepted checkpoint, about a minute.</p>
      {msg && <p className={`msg ${msg.ok ? "ok" : "err"}`}>{msg.text}</p>}
    </div>
  );
}

function ForcedWithdrawal({ address }: { address: string }) {
  const [amount, setAmount] = useState("");
  const { busy, msg, run } = useAction();
  const v = parseUsdc(amount);
  return (
    <div className="section-body" style={{ maxWidth: 560 }}>
      <p className="hint">
        Asks for a withdrawal through Stellar instead of the lane. The lane must process it within the force-inclusion window, or anyone can freeze the settlement contract. It only releases free collateral and does not close positions.
      </p>
      <div className="field">
        <label htmlFor="fw" className="flabel">
          Amount
        </label>
        <Amount id="fw" value={amount} onChange={setAmount} />
      </div>
      <div>
        <button
          className="btn stellar"
          disabled={busy || v === null || v <= 0n}
          onClick={() =>
            void run(async () => {
              const hash = await stellar.requestForcedWithdrawal(address, v!);
              return `Requested on Stellar (${short(hash, 6)}).`;
            })
          }
        >
          {busy ? "Sign in your wallet…" : "Request on Stellar"}
        </button>
      </div>
      {msg && <p className={`msg ${msg.ok ? "ok" : "err"}`}>{msg.text}</p>}
    </div>
  );
}
