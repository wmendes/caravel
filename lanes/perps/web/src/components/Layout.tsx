import type { ReactNode } from "react";

import { ago, short } from "../format";
import { Link } from "../router";
import { useApp } from "../state";

export function Layout({ children }: { children: ReactNode }) {
  const { status, statusError, onChain, frozen, address, connecting, walletError, connect, disconnect } = useApp();
  const acceptedAt = onChain ? Number(onChain.accepted_at) * 1000 : null;
  return (
    <>
      <header className="top">
        <Link to="/trade/BTC-PERP" className="brand">
          <b>Caravel</b>
          <span>testnet</span>
        </Link>
        <nav aria-label="Main">
          <Link to="/trade/BTC-PERP">Trade</Link>
          <Link to="/portfolio">Portfolio</Link>
          <Link to="/explorer">Explorer</Link>
          {frozen && (
            <Link to="/escape" className="alert">
              Escape
            </Link>
          )}
          <Link to="/about">About</Link>
        </nav>
        {address ? (
          <button className="btn small" onClick={disconnect} title="Forget this account in the app">
            <span className="mono">{short(address, 5)}</span>
          </button>
        ) : (
          <button className="btn small harbor" onClick={() => void connect()} disabled={connecting}>
            {connecting ? "Connecting…" : "Connect Freighter"}
          </button>
        )}
      </header>
      <div className="statusbar" role="status">
        {statusError ? (
          <span className="error">Lane API unreachable: {statusError}</span>
        ) : (
          <>
            <span>
              Lane block <b className="num">#{status?.height ?? "…"}</b> <span className="badge lane"><span className="dot" />soft</span>
            </span>
            <span>
              Checkpoint <b className="num">#{onChain ? onChain.seq.toString() : "…"}</b>{" "}
              <span className="badge harbor"><span className="dot" />settled on Stellar</span>
              {acceptedAt ? <span className="muted"> {ago(acceptedAt)}</span> : null}
            </span>
            {status?.halted && <span className="badge warn">sequencer halted</span>}
            {frozen && <span className="badge warn">settlement frozen: use Escape</span>}
          </>
        )}
        {walletError && <span className="error">{walletError}</span>}
      </div>
      <main>{children}</main>
    </>
  );
}
