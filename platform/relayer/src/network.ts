/** Stellar testnet network passphrase (spec §3.1). M0 is testnet only (spec §0.3 rule 6). */
export const TESTNET_PASSPHRASE = "Test SDF Network ; September 2015";

/** The passphrase of a local `stellar/quickstart` network, used by scripts/e2e-local.sh. */
export const LOCAL_PASSPHRASE = "Standalone Network ; February 2017";

/** Throws unless `passphrase` is testnet or a local quickstart network. Call it before signing anything. */
export function assertTestnet(passphrase: string): void {
  if (passphrase !== TESTNET_PASSPHRASE && passphrase !== LOCAL_PASSPHRASE) {
    throw new Error(`refusing to run on network "${passphrase}": Caravel M0 is testnet only`);
  }
}
