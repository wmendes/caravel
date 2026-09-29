/** Stellar testnet network passphrase (spec §3.1). M0 is testnet only (spec §0.3 rule 9). */
export const TESTNET_PASSPHRASE = "Test SDF Network ; September 2015";

/** Throws unless `passphrase` is the Stellar testnet passphrase. Call it before signing anything. */
export function assertTestnet(passphrase: string): void {
  if (passphrase !== TESTNET_PASSPHRASE) {
    throw new Error(`refusing to run on network "${passphrase}": Caravel M0 is testnet only`);
  }
}
