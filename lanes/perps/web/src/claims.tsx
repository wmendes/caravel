/**
 * Withdrawals, from the lane to your Stellar wallet (DEC-107): a withdrawal
 * leaves the lane at once, becomes claimable once its checkpoint is accepted
 * on Stellar, and is paid when you claim it. This keeps the account's
 * withdrawals in flight and its claimable ones in one place, for every page.
 */
import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";

import { lane, type Proof } from "./api/lane";
import { explain, stellar } from "./api/stellar";
import { useApp, usePoll } from "./state";

export type Claim = Proof & { claimed: boolean };

/** A withdrawal sent to the lane, not claimable yet. */
export interface Pending {
  amount: string;
  at: number;
  tx: string;
}

interface ClaimsState {
  /** Withdrawals ready to claim on Stellar; null while loading. */
  ready: Claim[] | null;
  readyTotal: bigint;
  pending: Pending[];
  /** Records a withdrawal the lane just queued. */
  withdrew: (amount: bigint, tx: string) => void;
  /** Claims every ready withdrawal, one wallet signature each. */
  claimAll: () => Promise<void>;
  claiming: { done: number; of: number } | null;
  claimError: string | null;
  lastClaimed: { amount: bigint; tx: string } | null;
}

const Ctx = createContext<ClaimsState | null>(null);
const KEY = (g: string) => `caravel.pending.${g}`;
/** A pending withdrawal older than this is dropped: it is claimable or failed. */
const PENDING_TTL_MS = 15 * 60_000;

function loadPending(g: string): Pending[] {
  try {
    const v = JSON.parse(localStorage.getItem(KEY(g)) ?? "[]") as Pending[];
    return v.filter((p) => Date.now() - p.at < PENDING_TTL_MS);
  } catch {
    return [];
  }
}

function savePending(g: string, p: Pending[]) {
  try {
    localStorage.setItem(KEY(g), JSON.stringify(p));
  } catch {
    /* a convenience: the list is rebuilt from the lane anyway */
  }
}

export function ClaimsProvider({ children }: { children: ReactNode }) {
  const { address, onChain } = useApp();
  const [all, setAll] = useState<Claim[] | null>(null);
  const [pending, setPending] = useState<Pending[]>([]);
  const seen = useRef<Set<string> | null>(null);
  const [claiming, setClaiming] = useState<{ done: number; of: number } | null>(null);
  const [claimError, setClaimError] = useState<string | null>(null);
  const [lastClaimed, setLastClaimed] = useState<{ amount: bigint; tx: string } | null>(null);

  useEffect(() => {
    setAll(null);
    seen.current = null;
    setPending(address ? loadPending(address) : []);
  }, [address]);

  const refresh = useCallback(async () => {
    if (!address) return;
    const p = await lane.withdrawalProofs(address).catch(() => ({ withdrawals: [] as Proof[] }));
    const list = await Promise.all(p.withdrawals.map(async (x) => ({ ...x, claimed: await stellar.isClaimed(x.seq, x.index).catch(() => false) })));
    setAll(list);
    // A withdrawal that became claimable is no longer pending: match new leaves by amount.
    const prev = seen.current;
    seen.current = new Set(list.map((c) => `${c.seq}-${c.index}`));
    // On the first load, an unclaimed one may be what a pending entry was waiting for.
    const fresh = prev ? list.filter((c) => !prev.has(`${c.seq}-${c.index}`)) : list.filter((c) => !c.claimed);
    if (fresh.length) {
      const left = [...loadPending(address)];
      for (const c of fresh) {
        const i = left.findIndex((x) => x.amount === c.amount);
        if (i >= 0) left.splice(i, 1);
      }
      savePending(address, left);
      setPending(left);
    }
  }, [address]);

  // Often while something is in flight; otherwise every 15 s.
  usePoll(refresh, pending.length ? 4000 : 15000, [address, pending.length, onChain?.seq]);

  const withdrew = useCallback(
    (amount: bigint, tx: string) => {
      if (!address) return;
      setPending((ps) => {
        const next = [...ps, { amount: amount.toString(), at: Date.now(), tx }];
        savePending(address, next);
        return next;
      });
    },
    [address],
  );

  const ready = all?.filter((c) => !c.claimed) ?? null;
  const readyTotal = (ready ?? []).reduce((s, c) => s + BigInt(c.amount ?? "0"), 0n);

  const claimAll = useCallback(async () => {
    if (!address || !ready?.length) return;
    setClaimError(null);
    let total = 0n;
    let tx = "";
    try {
      for (const [i, c] of ready.entries()) {
        setClaiming({ done: i, of: ready.length });
        tx = await stellar.claimWithdrawal(address, { seq: c.seq, index: c.index, amount: c.amount ?? "0", proof: c.proof });
        total += BigInt(c.amount ?? "0");
      }
      setLastClaimed({ amount: total, tx });
    } catch (e) {
      setClaimError(explain(e));
      if (total > 0n) setLastClaimed({ amount: total, tx });
    } finally {
      setClaiming(null);
      await refresh();
    }
  }, [address, ready, refresh]);

  return <Ctx.Provider value={{ ready, readyTotal, pending, withdrew, claimAll, claiming, claimError, lastClaimed }}>{children}</Ctx.Provider>;
}

export function useClaims(): ClaimsState {
  const v = useContext(Ctx);
  if (!v) throw new Error("useClaims outside ClaimsProvider");
  return v;
}
