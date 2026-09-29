// The Stellar SDK and Freighter pass bytes as Node Buffers; browsers have none.
import { Buffer } from "buffer";
(globalThis as { Buffer?: typeof Buffer }).Buffer ??= Buffer;

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./styles.css";

const root = document.getElementById("root");
if (!root) throw new Error("missing #root");
createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
