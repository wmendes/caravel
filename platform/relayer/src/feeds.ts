/**
 * Feed modules (M0.5, DEC-053): an app's signed feeds, such as the perps
 * oracle, run in the relayer as modules the config names. The relayer loads
 * each one, gives it a host, and calls its `tick` on an interval, like its
 * own loops. The platform knows nothing of what a feed carries: a module
 * posts opaque update bytes to the sequencer's feed route
 * (`POST /internal/{route}`), where the app's node checks them.
 *
 * A module is an ES module whose `createFeed(host, options)` returns a
 * `Feed`. Secrets are never in the config: a module reads its own keys from
 * environment variables (spec §0.3 rule 8).
 */
import { isAbsolute, resolve } from "node:path";
import { pathToFileURL } from "node:url";

/** What the relayer gives a feed module. */
export interface FeedHost {
  /** The lane's id, from the sequencer's `/v1/status`. */
  laneId: Uint8Array;
  rpcUrl: string;
  networkPassphrase: string;
  /** GET a public sequencer path, e.g. `/v1/markets`. */
  getJson(path: string): Promise<unknown>;
  /** `POST /internal/{route}` with `{update: hex}` and the internal token. */
  postFeed(route: string, updateHex: string): Promise<void>;
  log(level: "info" | "warn" | "error", msg: string, extra?: Record<string, unknown>): void;
}

export interface Feed {
  /** Names the loop in logs. */
  readonly name: string;
  /** What the module runs, for the start-up log. */
  describe(): Record<string, unknown>;
  /** One round; errors are reported, and the loop goes on. */
  tick(): Promise<{ errors: string[] }>;
}

export interface FeedModule {
  createFeed(host: FeedHost, options: unknown): Promise<Feed>;
}

/** One `feeds[]` entry of the relayer config. */
export interface FeedSpec {
  /** The module file, relative to the config file or absolute. */
  module: string;
  intervalMs?: number;
  /** A tick that takes longer is abandoned and reported, and the loop goes on (default 30 s). */
  deadlineMs?: number;
  options?: unknown;
}

export const FEED_DEADLINE_MS = 30_000;

/**
 * `p`, or a rejection once `ms` pass. A feed tick that never settles would
 * otherwise stop its loop for good without a word: on 2026-10-03 the testnet
 * oracle stopped for 19 hours behind an RPC call without a timeout. The
 * abandoned tick is not cancelled; feed updates carry their own publish
 * time, so a late one is harmless.
 */
export function withDeadline<T>(p: Promise<T>, ms: number, what: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${what}: no answer in ${ms} ms`)), ms);
  });
  return Promise.race([p, late]).finally(() => clearTimeout(timer));
}

/** Loads a feed module; `baseDir` is the config file's directory. */
export async function loadFeedModule(spec: FeedSpec, baseDir: string): Promise<FeedModule> {
  if (typeof spec.module !== "string" || spec.module === "") throw new Error("feeds[].module must name a module file");
  const path = isAbsolute(spec.module) ? spec.module : resolve(baseDir, spec.module);
  const m = (await import(pathToFileURL(path).href)) as Partial<FeedModule>;
  if (typeof m.createFeed !== "function") throw new Error(`${path} is not a feed module: it exports no createFeed(host, options)`);
  return m as FeedModule;
}
