/**
 * The way back to your wallet (DEC-107): each withdrawal from leaving the
 * lane to the claim on Stellar, and the button that claims it.
 */
import { useClaims, type Pending } from "../claims";
import { short, usdc } from "../format";
import { Link } from "../router";
import { useApp } from "../state";
import { Chip, useNow } from "./ui";

/** Withdrawals in flight and ready, with the Claim button; nothing when there are none. */
export function WithdrawalTracker() {
  const { ready, readyTotal, pending, claimAll, claiming, claimError, lastClaimed } = useClaims();
  const now = useNow(1000);
  const n = ready?.length ?? 0;
  if (!pending.length && !n && !lastClaimed && !claimError) return null;
  return (
    <section className="section tracker" aria-label="Withdrawals on their way to your wallet">
      <div className="section-head">
        <h3>On the way to your wallet</h3>
        {n > 0 && <Chip kind="settled">ready</Chip>}
      </div>
      <div className="section-body">
        {pending.map((p) => (
          <InFlight key={p.tx} p={p} now={now} />
        ))}
        {n > 0 && (
          <div className="ready">
            <ol className="track">
              <li className="done">Left the lane</li>
              <li className="done">Checkpoint accepted on Stellar</li>
              <li className="now">Claim to your wallet</li>
            </ol>
            <button className="btn stellar lg block" disabled={!!claiming} onClick={() => void claimAll()}>
              {claiming ? `Sign in your wallet… (${claiming.done + 1} of ${claiming.of})` : `Claim ${usdc(readyTotal)} USDC on Stellar`}
            </button>
            <p className="hint">
              {n === 1 ? "One withdrawal, one wallet signature." : `${n} withdrawals: one wallet signature each.`} The USDC goes from the settlement contract to your Stellar wallet.
            </p>
          </div>
        )}
        {lastClaimed && (
          <p className="msg ok">
            Claimed {usdc(lastClaimed.amount)} USDC to your wallet ({short(lastClaimed.tx, 6)}).
          </p>
        )}
        {claimError && <p className="msg err">{claimError}</p>}
      </div>
    </section>
  );
}

function InFlight({ p, now }: { p: Pending; now: number }) {
  const s = Math.max(0, Math.round((now - p.at) / 1000));
  return (
    <div className="inflight">
      <div className="inflight-head">
        <b className="num">{usdc(p.amount)} USDC</b>
        <span className="faint num">{s < 60 ? `${s} s` : `${Math.floor(s / 60)} min ${s % 60} s`}</span>
      </div>
      <ol className="track">
        <li className="done">Left the lane</li>
        <li className="now">Waiting for the next checkpoint on Stellar, usually under a minute</li>
        <li>Claim to your wallet</li>
      </ol>
    </div>
  );
}

/** The top bar's reminder: Claim when something is ready, else the count in flight. */
export function ClaimChip() {
  const { address } = useApp();
  const { ready, readyTotal, pending, claimAll, claiming } = useClaims();
  if (!address) return null;
  if (ready?.length) {
    return (
      <button className="btn sm stellar" disabled={!!claiming} onClick={() => void claimAll()} title="Withdrawals whose checkpoint Stellar accepted">
        {claiming ? "Sign in your wallet…" : `Claim ${usdc(readyTotal)} USDC`}
      </button>
    );
  }
  if (pending.length) {
    return (
      <Link to="/portfolio" className="btn sm ghost" style={{ textDecoration: "none" }} title="Claimable after the next checkpoint on Stellar">
        <span className="live-dot">Withdrawal on its way</span>
      </Link>
    );
  }
  return null;
}
