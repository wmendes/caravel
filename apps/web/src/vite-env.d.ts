/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_SEQUENCER_URL?: string;
  readonly VITE_VALIDATOR_URLS?: string;
  readonly VITE_RPC_URL?: string;
  readonly VITE_NETWORK_PASSPHRASE?: string;
  readonly VITE_NETWORK_NAME?: string;
  readonly VITE_SETTLEMENT_CONTRACT?: string;
  readonly VITE_USDC_CONTRACT?: string;
  readonly VITE_USDC_ASSET?: string;
  readonly VITE_EXPLORER_URL?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
