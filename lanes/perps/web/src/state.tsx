/** App-wide state: the lane status, markets, the connected wallet and its session key. */
import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";

import { lane, type Account, type Market, type Status } from "./api/lane";
import { stellar, type LastCheckpoint } from "./api/stellar";
import * as wallet from "./api/wallet";
import * as session from "./session";

interface AppState {
  status: Status | null;
  statusError: string | null;
  markets: Market[];
  onChain: LastCheckpoint | null;
  frozen: boolean;
  address: string | null;
  connecting: boolean;
  walletError: string | null;
  key: session.SessionKey | null;
  /** The connected account on the lane: undefined while loading, null before its first deposit. */
  account: Account | null | undefined;
  setAccount: (a: Account | null) => void;
  connect: () => Promise<void>;
  disconnect: () => void;
  setKey: (k: session.SessionKey | null) => void;
}

const Ctx = createContext<AppState | null>(null);

export function usePoll(f: () => Promise<void>, ms: number, deps: unknown[] = []) {
  const ref = useRef(f);
  ref.current = f;
  useEffect(() => {
    let live = true;
    const tick = async () => {
      if (!live) return;
      try {
        await ref.current();
      } finally {
        if (live) setTimeout(tick, ms);
      }
    };
    void tick();
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
}

export function AppProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<Status | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [markets, setMarkets] = useState<Market[]>([]);
  const [onChain, setOnChain] = useState<LastCheckpoint | null>(null);
  const [frozen, setFrozen] = useState(false);
  const [address, setAddress] = useState<string | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [walletError, setWalletError] = useState<string | null>(null);
  const [key, setKey] = useState<session.SessionKey | null>(null);
  const [account, setAccount] = useState<Account | null | undefined>(undefined);

  usePoll(async () => {
    try {
      const [s, m] = await Promise.all([lane.status(), lane.markets()]);
      setStatus(s);
      setMarkets(m);
      setStatusError(null);
    } catch (e) {
      setStatusError(e instanceof Error ? e.message : String(e));
    }
  }, 2000);

  usePoll(async () => {
    try {
      const [l, f] = await Promise.all([stellar.lastCheckpoint(), stellar.frozen()]);
      setOnChain(l);
      setFrozen(f);
    } catch {
      /* Stellar RPC hiccups are shown by stale values, not errors */
    }
  }, 15000);

  useEffect(() => {
    void wallet.current().then((a) => a && setAddress(a)).catch(() => {});
  }, []);

  useEffect(() => setAccount(undefined), [address]);
  usePoll(async () => {
    if (!address) return;
    try {
      setAccount(await lane.account(address));
    } catch {
      /* shown by the status banner */
    }
  }, 5000, [address]);

  useEffect(() => {
    setKey(null);
    if (address) void session.load(address).then(setKey);
  }, [address]);

  const connect = useCallback(async () => {
    setConnecting(true);
    setWalletError(null);
    try {
      setAddress(await wallet.connect());
    } catch (e) {
      setWalletError(e instanceof Error ? e.message : String(e));
    } finally {
      setConnecting(false);
    }
  }, []);

  const disconnect = useCallback(() => {
    setAddress(null);
    void wallet.disconnect();
  }, []);

  return <Ctx.Provider value={{ status, statusError, markets, onChain, frozen, address, connecting, walletError, key, account, setAccount, connect, disconnect, setKey }}>{children}</Ctx.Provider>;
}

export function useApp(): AppState {
  const v = useContext(Ctx);
  if (!v) throw new Error("useApp outside AppProvider");
  return v;
}
