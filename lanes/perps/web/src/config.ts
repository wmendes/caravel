/**
 * Where the app talks to. Defaults are the testnet deployment (T-012); a
 * local lane sets the VITE_* variables (see .env.local.example).
 */
const env = import.meta.env;
const origin = typeof window === "undefined" ? "http://localhost" : window.location.origin;

function url(v: string | undefined, fallback: string): string {
  const s = v && v.length > 0 ? v : fallback;
  return new URL(s, origin).toString().replace(/\/$/, "");
}

export const config = {
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
} as const;
