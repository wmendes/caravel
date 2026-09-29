import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// CARAVEL_PROXY=https://35-224-76-64.sslip.io npm run dev (or preview) serves
// the app against a remote lane from the same origin, like the VM does.
const target = process.env.CARAVEL_PROXY;
const proxy = target
  ? {
      "/v1": { target, changeOrigin: true, ws: true },
      "/validators": { target, changeOrigin: true },
    }
  : undefined;

export default defineConfig({
  plugins: [react()],
  server: { proxy },
  preview: { proxy },
});
