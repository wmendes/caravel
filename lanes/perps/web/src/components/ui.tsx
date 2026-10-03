/** The terminal's small parts: tabs, chips, signed numbers, skeletons, empty states, toasts. */
import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";

export function Tabs<T extends string>({ tabs, value, onChange, label, end }: { tabs: { id: T; label: ReactNode; count?: number }[]; value: T; onChange: (v: T) => void; label: string; end?: ReactNode }) {
  return (
    <div className="tabs" role="tablist" aria-label={label}>
      {tabs.map((t) => (
        <button
          key={t.id}
          role="tab"
          className="tab"
          aria-selected={t.id === value}
          onClick={() => onChange(t.id)}
          onKeyDown={(e) => {
            if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return;
            const i = tabs.findIndex((x) => x.id === value);
            const next = tabs[(i + (e.key === "ArrowRight" ? 1 : tabs.length - 1)) % tabs.length];
            if (next) onChange(next.id);
          }}
        >
          {t.label}
          {t.count ? <span className="count num">{t.count}</span> : null}
        </button>
      ))}
      {end && <div className="end">{end}</div>}
    </div>
  );
}

/** Where a number stands: on the lane (soft) or on Stellar (settled). */
export function Chip({ kind, children, title }: { kind: "soft" | "settled" | "warn" | "danger" | "plain"; children: ReactNode; title?: string }) {
  return (
    <span className={`chip ${kind}`} title={title}>
      {children}
    </span>
  );
}

/** A signed amount: the sign always shows, colour follows it. */
export function Signed({ value, children }: { value: bigint | number; children: ReactNode }) {
  const v = typeof value === "bigint" ? (value > 0n ? 1 : value < 0n ? -1 : 0) : Math.sign(value);
  return <span className={`num ${v > 0 ? "up" : v < 0 ? "down" : ""}`}>{v > 0 ? "+" : ""}{children}</span>;
}

export function SidePill({ long }: { long: boolean }) {
  return <span className={`side-pill ${long ? "long" : "short"}`}>{long ? "LONG" : "SHORT"}</span>;
}

export function Skel({ w = "60%", h }: { w?: string | number; h?: number }) {
  return <span className="skel" style={{ width: w, height: h }} aria-hidden />;
}

export function Empty({ title, children, action }: { title: string; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className="empty">
      <b>{title}</b>
      {children && <span>{children}</span>}
      {action}
    </div>
  );
}

/** A value that briefly takes the direction colour when it moves. */
export function useFlash(v: number): "" | "flash-up" | "flash-down" {
  const prev = useRef(v);
  const [cls, setCls] = useState<"" | "flash-up" | "flash-down">("");
  useEffect(() => {
    if (v === prev.current || prev.current === 0) {
      prev.current = v;
      return;
    }
    setCls(v > prev.current ? "flash-up" : "flash-down");
    prev.current = v;
    const t = setTimeout(() => setCls(""), 200);
    return () => clearTimeout(t);
  }, [v]);
  return cls;
}

/** Re-renders every `ms`, for "3s ago" labels. */
export function useNow(ms = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(t);
  }, [ms]);
  return now;
}

interface Toast {
  id: number;
  kind: "ok" | "soft" | "err";
  title: string;
  body?: string;
}

const ToastCtx = createContext<(t: Omit<Toast, "id">) => void>(() => {});

export function Toasts({ children }: { children: ReactNode }) {
  const [list, setList] = useState<Toast[]>([]);
  const next = useRef(1);
  const push = useCallback((t: Omit<Toast, "id">) => {
    const id = next.current++;
    setList((l) => [...l.slice(-3), { ...t, id }]);
    setTimeout(() => setList((l) => l.filter((x) => x.id !== id)), t.kind === "err" ? 9000 : 5000);
  }, []);
  return (
    <ToastCtx.Provider value={push}>
      {children}
      <div className="toasts" role="status" aria-live="polite">
        {list.map((t) => (
          <div className="toast" key={t.id}>
            <b className={t.kind === "err" ? "down" : undefined}>{t.title}</b>
            {t.body && <span className="dim">{t.body}</span>}
            {t.kind === "soft" && (
              <span>
                <Chip kind="soft">soft</Chip> <span className="faint">on the lane until the next checkpoint settles it on Stellar</span>
              </span>
            )}
          </div>
        ))}
      </div>
    </ToastCtx.Provider>
  );
}

export const useToast = () => useContext(ToastCtx);
