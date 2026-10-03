/**
 * Relayer configuration: a JSON file for everything public, environment
 * variables for every secret (spec §0.3 rule 8).
 *
 * - CARAVEL_INTERNAL_TOKEN: the sequencer's internal API token;
 * - CARAVEL_RELAYER_SECRET: S... key that pays for checkpoint transactions.
 *
 * Feed modules read their own keys (perps: CARAVEL_ORACLE_SECRET).
 */
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";

import { Keypair, StrKey } from "@stellar/stellar-sdk";

import type { FeedSpec } from "./feeds.js";
import { assertTestnet } from "./network.js";

export interface RelayerFile {
  rpcUrl: string;
  networkPassphrase: string;
  settlementContract: string;
  sequencerUrl: string;
  metricsFile: string;
  loops: { inbox: boolean; checkpoints: boolean };
  intervalsMs?: { inbox?: number; checkpoints?: number };
  /** The app's feed modules (DEC-053), e.g. the perps oracle. */
  feeds?: FeedSpec[];
}

export interface RelayerConfig {
  file: RelayerFile;
  /** The config file's directory: feed module paths are relative to it. */
  dir: string;
  metricsPath: string;
  token: string;
  relayer: Keypair | null;
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
  const m0 = file as unknown as { loops: Record<string, unknown>; oracle?: unknown };
  if ("oracle" in m0.loops || m0.oracle !== undefined) {
    throw new Error("loops.oracle and oracle moved to a feed module: feeds: [{module: \"<perps relayer-feeds>/dist/index.js\", options: {...}}] (DEC-053)");
  }
  for (const f of file.feeds ?? []) {
    if (typeof f.module !== "string") throw new Error("every feeds[] entry needs a module");
    if (f.deadlineMs !== undefined && !(Number.isInteger(f.deadlineMs) && f.deadlineMs > 0)) throw new Error("feeds[].deadlineMs must be a positive integer");
  }
  return {
    file,
    dir: dirname(path),
    metricsPath: resolve(dirname(path), file.metricsFile),
    token,
    relayer: file.loops.checkpoints ? secret("CARAVEL_RELAYER_SECRET") : null,
  };
}
