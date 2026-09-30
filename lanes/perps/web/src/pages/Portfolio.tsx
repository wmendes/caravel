import { useState } from "react";

import { lane, type Account, type Proof } from "../api/lane";
import { explain, stellar } from "../api/stellar";
import { config } from "../config";
import { parseUsdc, short, usdc } from "../format";
import { useApp, usePoll } from "../state";
import { explainReject, laneIds, send } from "../trade";

export function Portfolio() {
  const { address, connect, status } = useApp();
  const [wallet, setWallet] = useState<bigint | null | undefined>(undefined);
  const [account, setAccount] = useState<Account | null | undefined>(undefined);
  const [claims, setClaims] = useState<(Proof & { claimed: boolean })[]>([]);

  usePoll(async () => {
    if (!address) return;
    const [w, a, p] = await Promise.all([stellar.usdcBalance(address), lane.account(address), lane.withdrawalProofs(address).catch(() => ({ withdrawals: [] as Proof[] }))]);
    setWallet(w);
    setAccount(a);
    const withStatus = await Promise.all(p.withdrawals.map(async (x) => ({ ...x, claimed: await stellar.isClaimed(x.seq, x.index).catch(() => false) })));
    setClaims(withStatus);
  }, 5000, [address]);

  if (!address) {
    return (
      <div className="prose">
        <h1>Portfolio</h1>
        <p className="muted">Connect a wallet to see your USDC on Stellar and your lane account.</p>
        <div>
          <button className="btn harbor" onClick={() => void connect()}>
            Connect a wallet
          </button>
        </div>
      </div>
    );
  }

  return (
    <>
      <div className="page-head">
        <div>
          <h1>Portfolio</h1>
          <p>
            <span className="mono">{short(address, 8)}</span>. USDC lives in the settlement contract on Stellar; the lane tracks your share.
          </p>
        </div>
      </div>
      <section className="panel facts" style={{ marginBottom: 16 }}>
        <div className="fact">
          <span>USDC in your Stellar wallet</span>
          <b>{wallet === undefined ? "…" : wallet === null ? "No trustline" : usdc(wallet)}</b>
        </div>
        <div className="fact">
          <span>Lane collateral</span>
          <b>{account ? usdc(account.collateral) : "–"}</b>
        </div>
        <div className="fact">
          <span>Equity</span>
          <b>{account ? usdc(account.equity) : "–"}</b>
        </div>
        <div className="fact">
          <span>Free collateral</span>
          <b>{account ? usdc(account.free_collateral) : "–"}</b>
          <small> can be withdrawn</small>
        </div>
      </section>
      {wallet === null && (
        <section className="panel" style={{ marginBottom: 16 }}>
          <h3>Get testnet USDC</h3>
          <p>
            Add the USDC asset (<span className="mono">{config.usdcAsset}</span>) in your wallet, then get testnet USDC from the{" "}
            <a href={config.faucetUrl} target="_blank" rel="noreferrer">
              Circle faucet
            </a>{" "}
            (choose Stellar testnet).
          </p>
        </section>
      )}
      <div className="two">
        <Deposit address={address} />
        <Withdraw address={address} ids={laneIds(status)} account={account ?? null} />
      </div>
      <Claims address={address} claims={claims} />
      <ForcedWithdrawal address={address} />
    </>
  );
}

function useAction() {
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const run = async (f: () => Promise<string>, explainErr: (e: unknown) => string = explain) => {
    setBusy(true);
    setMsg(null);
    try {
      setMsg({ ok: true, text: await f() });
    } catch (e) {
      setMsg({ ok: false, text: explainErr(e) });
    } finally {
      setBusy(false);
    }
  };
  return { busy, msg, run };
}

function Deposit({ address }: { address: string }) {
  const [amount, setAmount] = useState("");
  const { busy, msg, run } = useAction();
  const v = parseUsdc(amount);
  return (
    <section className="panel stack">
      <h3>Deposit</h3>
      <p className="note">Moves USDC from your wallet into the settlement contract on Stellar. The lane credits it after the relayer sees it, usually within a few seconds.</p>
      <div className="field">
        <label htmlFor="dep">Amount (USDC), at least 1</label>
        <input id="dep" inputMode="decimal" value={amount} onChange={(e) => setAmount(e.target.value)} placeholder="100" />
      </div>
      <button
        className="btn harbor"
        disabled={busy || v === null || v < 10_000_000n}
        onClick={() =>
          void run(async () => {
            const hash = await stellar.deposit(address, v!);
            return `Deposited on Stellar (${short(hash, 6)}). The lane will credit it shortly.`;
          })
        }
      >
        {busy ? "Waiting for your wallet…" : "Deposit on Stellar"}
      </button>
      {msg && <p className={msg.ok ? "ok" : "error"}>{msg.text}</p>}
    </section>
  );
}

function Withdraw({ address, ids, account }: { address: string; ids: ReturnType<typeof laneIds>; account: Account | null }) {
  const [amount, setAmount] = useState("");
  const { busy, msg, run } = useAction();
  const v = parseUsdc(amount);
  return (
    <section className="panel stack">
      <h3>Withdraw</h3>
      <p className="note">Signed in your wallet. The amount leaves your lane collateral now and becomes claimable on Stellar after the next accepted checkpoint (about a minute).</p>
      <div className="field">
        <label htmlFor="wd">Amount (USDC)</label>
        <input id="wd" inputMode="decimal" value={amount} onChange={(e) => setAmount(e.target.value)} placeholder={account ? usdc(account.free_collateral).replace(/,/g, "") : "0"} />
      </div>
      <button
        className="btn primary"
        disabled={busy || !ids || !account || v === null || v <= 0n}
        onClick={() =>
          void run(async () => {
            const hash = await send(ids!, address, { kind: "withdraw", amount: v! }, null);
            return `Withdrawal queued in the lane (${short(hash, 6)}). Claim it below once its checkpoint is accepted.`;
          }, explainReject)
        }
      >
        {busy ? "Waiting for your wallet…" : "Withdraw from the lane"}
      </button>
      {msg && <p className={msg.ok ? "ok" : "error"}>{msg.text}</p>}
    </section>
  );
}

function Claims({ address, claims }: { address: string; claims: (Proof & { claimed: boolean })[] }) {
  const { busy, msg, run } = useAction();
  const open = claims.filter((c) => !c.claimed);
  return (
    <section className="panel" style={{ marginTop: 16 }}>
      <h3>Ready to claim on Stellar</h3>
      {open.length === 0 ? (
        <p className="empty">Nothing to claim. Withdrawals appear here after their checkpoint is accepted on Stellar.</p>
      ) : (
        <table>
          <thead>
            <tr>
              <th>Checkpoint</th>
              <th className="r">Amount (USDC)</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {open.map((c) => (
              <tr key={`${c.seq}-${c.index}`}>
                <td>
                  <span className="badge harbor">#{c.seq}</span>
                </td>
                <td className="r">{usdc(c.amount ?? "0")}</td>
                <td className="r">
                  <button
                    className="btn small harbor"
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
      {msg && <p className={msg.ok ? "ok" : "error"}>{msg.text}</p>}
    </section>
  );
}

function ForcedWithdrawal({ address }: { address: string }) {
  const [amount, setAmount] = useState("");
  const { busy, msg, run } = useAction();
  const v = parseUsdc(amount);
  return (
    <details className="panel" style={{ marginTop: 16 }}>
      <summary>Forced withdrawal (advanced)</summary>
      <div className="stack" style={{ marginTop: 12, maxWidth: 520 }}>
        <p className="note">
          Asks for a withdrawal through Stellar instead of the lane. The lane must process it within the force-inclusion window, or anyone can freeze the settlement contract. It only releases free collateral and does not close positions.
        </p>
        <div className="field">
          <label htmlFor="fw">Amount (USDC)</label>
          <input id="fw" inputMode="decimal" value={amount} onChange={(e) => setAmount(e.target.value)} />
        </div>
        <button
          className="btn harbor"
          disabled={busy || v === null || v <= 0n}
          onClick={() =>
            void run(async () => {
              const hash = await stellar.requestForcedWithdrawal(address, v!);
              return `Requested on Stellar (${short(hash, 6)}).`;
            })
          }
        >
          Request on Stellar
        </button>
        {msg && <p className={msg.ok ? "ok" : "error"}>{msg.text}</p>}
      </div>
    </details>
  );
}
