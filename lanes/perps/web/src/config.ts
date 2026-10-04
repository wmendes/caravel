/**
 * Where the app talks to. A deployment serves its own at `/config.json`
 * (caravel's `[env.<name>.web] config`, C-25), read once at boot; without
 * one, the VITE_* variables (see .env.local.example), else the testnet
 * deployment (T-012).
 */
const env = import.meta.env;
const origin = typeof window === "undefined" ? "http://localhost" : window.location.origin;

function url(v: string | undefined, fallback: string): string {
  const s = v && v.length > 0 ? v : fallback;
  return new URL(s, origin).toString().replace(/\/$/, "");
}

export type Config = {
  sequencerUrl: string;
  validatorUrls: string[];
  rpcUrl: string;
  networkPassphrase: string;
  networkName: string;
  settlementContract: string;
  usdcContract: string;
  usdcAsset: string;
  explorerUrl: string;
  faucetUrl: string;
  /** Horizon, for the DEX quote of the "Get test USDC" flow; empty turns the flow off. */
  horizonUrl: string;
  /** Friendbot, which funds a new testnet account with XLM; empty turns that step off. */
  friendbotUrl: string;
};

export const config: Config = {
  sequencerUrl: url(env.VITE_SEQUENCER_URL, origin),
  validatorUrls: (env.VITE_VALIDATOR_URLS ?? "/validators/1,/validators/2,/validators/3").split(",").map((v) => url(v, v)),
  rpcUrl: env.VITE_RPC_URL ?? "https://soroban-testnet.stellar.org",
  networkPassphrase: env.VITE_NETWORK_PASSPHRASE ?? "Test SDF Network ; September 2015",
  networkName: env.VITE_NETWORK_NAME ?? "testnet",
  settlementContract: env.VITE_SETTLEMENT_CONTRACT ?? "CBIHBEUZYFZQZEQPBJH2ID6CDRDZFEDI6XHAXVOCHG6FO5XWUIGPONWO",
  usdcContract: env.VITE_USDC_CONTRACT ?? "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA",
  usdcAsset: env.VITE_USDC_ASSET ?? "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
  explorerUrl: env.VITE_EXPLORER_URL ?? "https://stellar.expert/explorer/testnet",
  faucetUrl: "https://faucet.circle.com",
  horizonUrl: env.VITE_HORIZON_URL ?? "https://horizon-testnet.stellar.org",
  friendbotUrl: env.VITE_FRIENDBOT_URL ?? "https://friendbot.stellar.org",
};

const URLS: (keyof Config)[] = ["sequencerUrl", "rpcUrl", "explorerUrl", "faucetUrl", "horizonUrl", "friendbotUrl"];

/**
 * Lays a deployment's config over `target`: known keys of the right type
 * only (URLs resolved against this page); the rest is ignored. Returns the
 * keys it set.
 */
export function applyConfig(target: Config, json: unknown): (keyof Config)[] {
  if (typeof json !== "object" || json === null || Array.isArray(json)) return [];
  const set: (keyof Config)[] = [];
  for (const [k, v] of Object.entries(json as Record<string, unknown>)) {
    if (!(k in target)) continue;
    const key = k as keyof Config;
    if (key === "validatorUrls") {
      if (Array.isArray(v) && v.every((x) => typeof x === "string")) {
        target.validatorUrls = v.map((x: string) => url(x, x));
        set.push(key);
      }
    } else if (typeof v === "string" && v.length > 0) {
      target[key] = URLS.includes(key) ? url(v, v) : v;
      set.push(key);
    }
  }
  return set;
}

/** Reads `/config.json` once, before anything talks to the lane. */
export async function loadConfig(fetchFn: typeof fetch = fetch): Promise<void> {
  try {
    const r = await fetchFn("/config.json", { cache: "no-store" });
    if (!r.ok || !(r.headers.get("content-type") ?? "").includes("json")) return;
    applyConfig(config, await r.json());
  } catch {
    // No deployment config: the defaults stand.
  }
}
