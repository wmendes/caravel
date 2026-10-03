import { useState } from "react";

import { lane, type Account, type Proof } from "../api/lane";
import { explain, stellar } from "../api/stellar";
import { Chip, Empty, Signed, Skel, useToast } from "../components/ui";
import { config } from "../config";
import { parseUsdc, short, usdc } from "../format";
import { useApp, usePoll } from "../state";
import { explainReject, laneIds, send, type LaneIds } from "../trade";

export function Portfolio() {
  const { address, connect, connecting, status, account } = useApp();
  const [wallet, setWallet] = useState<bigint | null | undefined>(undefined);
  const [claims, setClaims] = useState<(Proof & { claimed: boolean })[] | null>(null);

  usePoll(async () => {
    if (!address) return;
    const [w, p] = await Promise.all([stellar.usdcBalance(address), lane.withdrawalProofs(address).catch(() => ({ withdrawals: [] as Proof[] }))]);
    setWallet(w);
    const withStatus = await Promise.all(p.withdrawals.map(async (x) => ({ ...x, claimed: await stellar.isClaimed(x.seq, x.index).catch(() => false) })));
    setClaims(withStatus);
  }, 5000, [address]);

  if (!address) {
    return (
      <div className="page">
        <div className="page-head">
          <div>
            <h1>Portfolio</h1>
            <p>Your USDC on Stellar, your lane account, and withdrawals ready to claim.</p>
          </div>
        </div>
        <div className="section">
          <Empty
            title="Connect a wallet to see your balances"
            action={
              <button className="btn stellar" disabled={connecting} onClick={() => void connect()}>
                {connecting ? "Connecting…" : "Connect wallet"}
              </button>
            }
          >
            Any Stellar wallet on testnet.
          </Empty>
        </div>
      </div>
    );
  }

  const v = (x: string | undefined) => (account === undefined ? <Skel w={90} h={18} /> : account === null ? "–" : usdc(x));
  return (
    <div className="page">
      <div className="page-head">
        <div>
          <h1>Portfolio</h1>
          <p>
            <span className="mono">{short(address, 6)}</span>. Your USDC sits in the settlement contract on Stellar; the lane keeps the account of your share.
          </p>
        </div>
      </div>

      <div className="board">
        <div className="statrow">
          <div className="stat">
            <span className="k">
              USDC in your wallet <Chip kind="settled">Stellar</Chip>
            </span>
            <span className="v num">{wallet === undefined ? <Skel w={90} h={18} /> : wallet === null ? "No trustline" : usdc(wallet)}</span>
            <span className="sub">Ready to deposit</span>
          </div>
          <div className="stat">
            <span className="k">
              Lane collateral <Chip kind="soft">soft</Chip>
            </span>
            <span className="v num">{v(account?.collateral)}</span>
            <span className="sub">Deposits, less withdrawals, plus realized PnL</span>
          </div>
          <div className="stat">
            <span className="k">
              Equity <Chip kind="soft">soft</Chip>
            </span>
            <span className="v num">{v(account?.equity)}</span>
            <span className="sub">{account ? <Signed value={BigInt(account.equity) - BigInt(account.collateral)}>{usdc(BigInt(account.equity) - BigInt(account.collateral))}</Signed> : "–"} unrealized</span>
          </div>
          <div className="stat">
            <span className="k">
              Free collateral <Chip kind="soft">soft</Chip>
            </span>
            <span className="v num">{v(account?.free_collateral)}</span>
            <span className="sub">What you can withdraw now</span>
          </div>
        </div>
        <div className="flow">
          <div>
            <b>1 · Deposit on Stellar</b>
            <span className="faint">Your wallet signs a Stellar transaction. The lane credits it within seconds.</span>
          </div>
          <div>
            <b>2 · Trade on the lane</b>
            <span className="faint">Fills are instant and soft until a checkpoint carries them to Stellar.</span>
          </div>
          <div>
            <b>3 · Withdraw, then claim</b>
            <span className="faint">After the next accepted checkpoint, claim the USDC on Stellar with a proof.</span>
          </div>
        </div>
      </div>

      {wallet === null && (
        <div className="section">
          <div className="section-head">
            <h3>Get testnet USDC</h3>
          </div>
          <div className="section-body">
            <p className="dim">
              Add the USDC asset (<span className="mono">{config.usdcAsset}</span>) in your wallet, then get testnet USDC from the{" "}
              <a href={config.faucetUrl} target="_blank" rel="noreferrer">
                Circle faucet
              </a>{" "}
              (choose Stellar testnet).
            </p>
          </div>
        </div>
      )}

      <div className="cols">
        <Deposit address={address} wallet={wallet} />
        <Withdraw address={address} ids={laneIds(status)} account={account ?? null} />
      </div>
      <Claims address={address} claims={claims} />
      <ForcedWithdrawal address={address} />
    </div>
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

function Deposit({ address, wallet }: { address: string; wallet: bigint | null | undefined }) {
  const [amount, setAmount] = useState("");
  const { busy, msg, run } = useAction();
  const v = parseUsdc(amount);
  const tooMuch = v !== null && typeof wallet === "bigint" && v > wallet;
  return (
    <section className="section">
      <div className="section-head">
        <h3>Deposit</h3>
        <Chip kind="settled">on Stellar</Chip>
      </div>
      <div className="section-body">
        <div className="field">
          <span className="flabel">
            <label htmlFor="dep">Amount</label>
            <span className="faint num">Wallet: {typeof wallet === "bigint" ? `${usdc(wallet)} USDC` : "–"}</span>
          </span>
          <Amount id="dep" value={amount} onChange={setAmount} placeholder="100" max={wallet} />
          {tooMuch && <span className="hint down">More than your wallet holds.</span>}
        </div>
        <button
          className="btn stellar block"
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
        <p className="hint">At least 1 USDC. It moves from your wallet into the settlement contract; the relayer reports it to the lane.</p>
        {msg && <p className={`msg ${msg.ok ? "ok" : "err"}`}>{msg.text}</p>}
      </div>
    </section>
  );
}

function Withdraw({ address, ids, account }: { address: string; ids: LaneIds | null; account: Account | null }) {
  const [amount, setAmount] = useState("");
  const { busy, msg, run } = useAction();
  const v = parseUsdc(amount);
  const free = account ? BigInt(account.free_collateral) : null;
  const tooMuch = v !== null && free !== null && v > free;
  return (
    <section className="section">
      <div className="section-head">
        <h3>Withdraw</h3>
        <Chip kind="soft">from the lane</Chip>
      </div>
      <div className="section-body">
        <div className="field">
          <span className="flabel">
            <label htmlFor="wd">Amount</label>
            <span className="faint num">Free: {free !== null ? `${usdc(free)} USDC` : "–"}</span>
          </span>
          <Amount id="wd" value={amount} onChange={setAmount} max={free} />
          {tooMuch && <span className="hint down">More than your free collateral.</span>}
        </div>
        <button
          className="btn block"
          disabled={busy || !ids || !account || v === null || v <= 0n || tooMuch}
          onClick={() =>
            void run(async () => {
              const hash = await send(ids!, address, { kind: "withdraw", amount: v! }, null);
              setAmount("");
              return `Withdrawal queued in the lane (${short(hash, 6)}). Claim it below once its checkpoint is accepted on Stellar.`;
            }, explainReject)
          }
        >
          {busy ? "Sign in your wallet…" : "Withdraw from the lane"}
        </button>
        <p className="hint">Signed in your wallet. It leaves your lane collateral now and becomes claimable on Stellar after the next accepted checkpoint.</p>
        {msg && <p className={`msg ${msg.ok ? "ok" : "err"}`}>{msg.text}</p>}
      </div>
    </section>
  );
}

function Claims({ address, claims }: { address: string; claims: (Proof & { claimed: boolean })[] | null }) {
  const { busy, msg, run } = useAction();
  const open = claims?.filter((c) => !c.claimed) ?? [];
  return (
    <section className="section">
      <div className="section-head">
        <h3>Ready to claim on Stellar</h3>
        {open.length > 0 && <Chip kind="settled">{open.length} ready</Chip>}
      </div>
      {claims === null ? (
        <div className="section-body">
          <Skel w="100%" />
        </div>
      ) : open.length === 0 ? (
        <Empty title="Nothing to claim">Withdrawals appear here once their checkpoint is accepted on Stellar.</Empty>
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
      )}
      {msg && (
        <div className="section-body">
          <p className={`msg ${msg.ok ? "ok" : "err"}`}>{msg.text}</p>
        </div>
      )}
    </section>
  );
}

function ForcedWithdrawal({ address }: { address: string }) {
  const [amount, setAmount] = useState("");
  const { busy, msg, run } = useAction();
  const v = parseUsdc(amount);
  return (
    <details className="section">
      <summary>Forced withdrawal through Stellar</summary>
      <div className="section-body" style={{ maxWidth: 560 }}>
        <p className="hint">
          Asks for a withdrawal through Stellar instead of the lane. The lane must process it within the force-inclusion window, or anyone can freeze the settlement contract. It only releases free collateral and does not close positions.
        </p>
        <div className="field">
          <label htmlFor="fw">Amount</label>
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
    </details>
  );
}
