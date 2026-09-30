/** A small pathname router: the app has five routes (spec §18.2). */
import { createContext, useContext, useEffect, useState, type AnchorHTMLAttributes, type ReactNode } from "react";

const Ctx = createContext<{ path: string; go: (to: string) => void }>({ path: "/", go: () => {} });

export function Router({ children }: { children: ReactNode }) {
  const [path, setPath] = useState(() => window.location.pathname);
  useEffect(() => {
    const on = () => setPath(window.location.pathname);
    window.addEventListener("popstate", on);
    return () => window.removeEventListener("popstate", on);
  }, []);
  const go = (to: string) => {
    if (to !== window.location.pathname) window.history.pushState(null, "", to);
    setPath(to);
    window.scrollTo(0, 0);
  };
  return <Ctx.Provider value={{ path, go }}>{children}</Ctx.Provider>;
}

export const useRoute = () => useContext(Ctx);

export function Link({ to, children, ...rest }: { to: string; children: ReactNode } & AnchorHTMLAttributes<HTMLAnchorElement>) {
  const { path, go } = useRoute();
  const active = path === to || (to !== "/" && path.startsWith(to));
  return (
    <a
      href={to}
      aria-current={active ? "page" : undefined}
      onClick={(e) => {
        if (e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
        e.preventDefault();
        go(to);
      }}
      {...rest}
    >
      {children}
    </a>
  );
}
