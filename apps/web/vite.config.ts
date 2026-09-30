import { defineConfig, loadEnv } from "vite";
import react from "@vitejs/plugin-react";

// The app calls the lane API on its own origin, as on the VM (DEC-047). So
// `npm run dev` and `npm run preview` proxy /v1 and /validators to the testnet
// lane by default, or to CARAVEL_PROXY=<url>. A local lane sets
// VITE_SEQUENCER_URL instead (env.local.example), and then nothing is proxied.
const TESTNET_LANE = "https://35-224-76-64.sslip.io";

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), "VITE_");
  const target = process.env.CARAVEL_PROXY ?? (env.VITE_SEQUENCER_URL ? undefined : TESTNET_LANE);
  const proxy = target
    ? {
        "/v1": { target, changeOrigin: true, ws: true },
        "/validators": { target, changeOrigin: true },
      }
    : undefined;
  return {
    plugins: [react()],
    server: { proxy },
    preview: { proxy },
  };
});
