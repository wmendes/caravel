/**
 * Relayer configuration: a JSON file for everything public, environment
 * variables for every secret (spec §0.3 rule 8).
 *
 * - CARAVEL_INTERNAL_TOKEN: the sequencer's internal API token;
 * - CARAVEL_RELAYER_SECRET: S... key that pays for checkpoint transactions;
 * - CARAVEL_ORACLE_SECRET: S... oracle key listed in the lane config.
 */
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";

import { Keypair, StrKey } from "@stellar/stellar-sdk";

import { assertTestnet } from "./network.js";

/** One price source, in priority order per market. */
export type SourceSpec = { reflector: string } | { coinbase: string } | { fixed: string };

export interface RelayerFile {
  rpcUrl: string;
  networkPassphrase: string;
  settlementContract: string;
  sequencerUrl: string;
  metricsFile: string;
  loops: { inbox: boolean; checkpoints: boolean; oracle: boolean };
  intervalsMs?: { inbox?: number; checkpoints?: number; oracle?: number };
  oracle?: {
    maxSourceAgeSecs: number;
    reflectorContract?: string;
    markets: Record<string, SourceSpec[]>;
  };
}

export interface RelayerConfig {
  file: RelayerFile;
  metricsPath: string;
  token: string;
  relayer: Keypair | null;
  oracle: Keypair | null;
}

function secret(name: string): Keypair {
  const v = process.env[name];
  if (!v) throw new Error(`set ${name}`);
  return Keypair.fromSecret(v.trim());
}

export async function loadConfig(path: string): Promise<RelayerConfig> {
  const file = JSON.parse(await readFile(path, "utf8")) as RelayerFile;
  assertTestnet(file.networkPassphrase);
  if (!StrKey.isValidContract(file.settlementContract)) throw new Error("settlementContract must be a C... contract id");
  const token = process.env.CARAVEL_INTERNAL_TOKEN;
  if (!token) throw new Error("set CARAVEL_INTERNAL_TOKEN");
  if (file.loops.oracle && !file.oracle) throw new Error("loops.oracle needs an oracle section");
  return {
    file,
    metricsPath: resolve(dirname(path), file.metricsFile),
    token,
    relayer: file.loops.checkpoints ? secret("CARAVEL_RELAYER_SECRET") : null,
    oracle: file.loops.oracle ? secret("CARAVEL_ORACLE_SECRET") : null,
  };
}
