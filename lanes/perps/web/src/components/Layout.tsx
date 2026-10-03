import type { ReactNode } from "react";

import { ago, short, usdc } from "../format";
import { Link, useRoute } from "../router";
import { useApp } from "../state";
import { Chip, useNow } from "./ui";

export function Layout({ children }: { children: ReactNode }) {
  const { path } = useRoute();
  const { account, status, statusError, live, onChain, frozen, address, connecting, walletError, connect, disconnect } = useApp();
  const now = useNow(1000);
  const acceptedAt = onChain ? Number(onChain.accepted_at) * 1000 : null;
  const lastBlock = status ? Number(status.last_block_timestamp_ms) : null;
  // A lane that has not produced a block for 10 block times is stalled, whatever its API says.
  const stalled = status && lastBlock ? now - lastBlock > Math.max(10_000, status.block_time_ms * 10) : false;
  return (
    <div className="app">
      <a href="#main" className="sr-only">
        Skip to content
      </a>
      <header className="topbar">
        <Link to="/trade/BTC-PERP" className="brand" aria-label="Caravel Perps, trade">
          <img src="/logo.svg" alt="Caravel" height={22} width={98} />
          <span className="tag">PERPS · TESTNET</span>
        </Link>
        <nav className="nav" aria-label="Main">
          <Link to="/trade/BTC-PERP" aria-current={path.startsWith("/trade") || path === "/" ? "page" : undefined}>
            Trade
          </Link>
          <Link to="/portfolio">Portfolio</Link>
          <Link to="/explorer">Explorer</Link>
          {frozen && (
            <Link to="/escape" className="escape">
              Escape
            </Link>
          )}
          <Link to="/about">About</Link>
        </nav>
        <div className="wallet">
          {address && account !== undefined && (
            <div className="eq">
              <span className="label">Equity</span>
              <b className="num">{account === null ? "No deposit yet" : `${usdc(account.equity)} USDC`}</b>
            </div>
          )}
          {address ? (
            <button className="btn sm" onClick={disconnect} title="Forget this account in the app">
              <span className="mono">{short(address, 4)}</span>
              <span className="faint" aria-hidden>
                ×
              </span>
            </button>
          ) : (
            <button className="btn sm stellar" onClick={() => void connect()} disabled={connecting}>
              {connecting ? "Connecting…" : "Connect wallet"}
            </button>
          )}
        </div>
      </header>
      <div>
        {frozen && (
          <div className="banner danger" role="alert">
            <b>The settlement contract is frozen.</b> The lane no longer settles. Claim your last checkpointed equity on Stellar. <Link to="/escape">Go to Escape</Link>
          </div>
        )}
        {statusError && (
          <div className="banner warn" role="alert">
            <b>Lane API unreachable.</b> Prices and balances below may be out of date. {statusError}
          </div>
        )}
        {!statusError && status?.halted && (
          <div className="banner warn" role="alert">
            <b>The sequencer halted:</b> {status.halted}. New orders are not accepted. Your funds stay claimable on Stellar.
          </div>
        )}
        {!statusError && !status?.halted && stalled && (
          <div className="banner warn" role="alert">
            <b>No new lane block for {ago(lastBlock!, now).replace(" ago", "")}.</b> Orders may wait until the sequencer catches up.
          </div>
        )}
        {walletError && (
          <div className="banner warn" role="alert">
            <b>Wallet:</b> {walletError}
          </div>
        )}
      </div>
      <main id="main">{children}</main>
      <footer className="statusbar" role="status" aria-live="off">
        <span className="legend">
          <Chip kind="soft" title="Executed on the lane; not yet in an accepted checkpoint">
            soft
          </Chip>
          Lane block <b className="num">#{status?.height ?? "…"}</b>
          {status && <span>· {status.block_time_ms >= 1000 ? `${status.block_time_ms / 1000}s` : `${status.block_time_ms}ms`} blocks</span>}
        </span>
        <span className="legend">
          <Chip kind="settled" title="Accepted by the settlement contract on Stellar">
            settled
          </Chip>
          Checkpoint <b className="num">#{onChain ? onChain.seq.toString() : "…"}</b>
          {acceptedAt ? <span>on Stellar {ago(acceptedAt, now)}</span> : null}
        </span>
        <span className="sp" />
        <span>
          {statusError ? (
            <span className="down">● Lane API unreachable</span>
          ) : live ? (
            <span className="live-dot" title="Prices and blocks arrive on every block">
              Live · {status?.lane_name ?? "lane"}
            </span>
          ) : (
            <Chip kind="warn">Reconnecting to the stream…</Chip>
          )}
        </span>
        <span>Testnet only · not audited</span> {/* claims-ok: states it is not audited */}
      </footer>
    </div>
  );
}
