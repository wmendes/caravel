import { Component, type ReactNode } from "react";

import { Layout } from "./components/Layout";
import { About } from "./pages/About";
import { Escape } from "./pages/Escape";
import { Explorer } from "./pages/Explorer";
import { Portfolio } from "./pages/Portfolio";
import { Trade } from "./pages/Trade";
import { Router, useRoute } from "./router";
import { AppProvider } from "./state";

function Routes() {
  const { path } = useRoute();
  const trade = /^\/trade\/([^/]+)$/.exec(path);
  let page;
  if (trade?.[1]) page = <Trade symbol={decodeURIComponent(trade[1])} />;
  else if (path === "/portfolio") page = <Portfolio />;
  else if (path === "/explorer") page = <Explorer />;
  else if (path === "/escape") page = <Escape />;
  else if (path === "/about") page = <About />;
  else page = <Trade symbol="BTC-PERP" />;
  return (
    <Layout>
      <PageBoundary key={path}>{page}</PageBoundary>
    </Layout>
  );
}

/** One page failing must not blank the app: the frame and status bar stay. */
class PageBoundary extends Component<{ children: ReactNode }, { error: string | null }> {
  state = { error: null as string | null };

  static getDerivedStateFromError(e: unknown) {
    return { error: e instanceof Error ? e.message : String(e) };
  }

  render() {
    if (this.state.error) {
      return (
        <div className="prose">
          <h1>This page failed to load</h1>
          <p className="error">{this.state.error}</p>
          <p className="muted">Reload the page. If it keeps happening, the lane API may be out of date or unreachable.</p>
        </div>
      );
    }
    return this.props.children;
  }
}

export function App() {
  return (
    <AppProvider>
      <Router>
        <Routes />
      </Router>
    </AppProvider>
  );
}
