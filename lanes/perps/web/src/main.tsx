// The Stellar SDK and wallet libraries pass bytes as Node Buffers; browsers have none.
import { Buffer } from "buffer";
(globalThis as { Buffer?: typeof Buffer }).Buffer ??= Buffer;

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { loadConfig } from "./config";
import "./styles.css";

const root = document.getElementById("root");
if (!root) throw new Error("missing #root");
// The deployment's config first: the app's modules read it as they load.
void loadConfig().then(async () => {
  const { App } = await import("./App");
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
});
