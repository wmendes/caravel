/**
 * The Caravel Perps feed module for the platform relayer (spec §17.3,
 * DEC-053): the signed oracle feed. The relayer config names it:
 *
 *   "feeds": [{
 *     "module": "<path>/lanes/perps/relayer-feeds/dist/index.js",
 *     "intervalMs": 1000,
 *     "options": {
 *       "maxSourceAgeSecs": 900,
 *       "reflectorContract": "C...",            // optional
 *       "markets": { "1": [{ "coinbaseStream": "BTC-USD" }, { "coinbase": "BTC-USD" }, { "reflector": "BTC" }] }
 *     }
 *   }]
 *
 * The oracle key is read from `options.keyEnv` (default
 * `CARAVEL_ORACLE_SECRET`), never from the config. Updates go to the perps
 * node's feed route, `POST /internal/oracle`.
 */
import { Keypair } from "@stellar/stellar-sdk";

import { CoinbaseStream, type Connect } from "./coinbase.js";
import { OracleFeeder, type FeedMarket, type MarketInfo } from "./oracle.js";
import { CoinbaseSpot, FixedPrice, ReflectorPrice, type PriceSource } from "./prices.js";
import { RpcReflector } from "./reflector.js";

/** The platform relayer's host, as `platform/relayer/src/feeds.ts` defines it. */
export interface FeedHost {
  laneId: Uint8Array;
  rpcUrl: string;
  networkPassphrase: string;
  getJson(path: string): Promise<unknown>;
  postFeed(route: string, updateHex: string): Promise<void>;
  log(level: "info" | "warn" | "error", msg: string, extra?: Record<string, unknown>): void;
}

export interface Feed {
  readonly name: string;
  describe(): Record<string, unknown>;
  tick(): Promise<{ errors: string[] }>;
}

/** One price source, in priority order per market (DEC-058: the Coinbase stream first). */
export type SourceSpec =
  | { coinbaseStream: string; maxAgeSecs?: number }
  | { coinbase: string }
  | { reflector: string }
  | { fixed: string };

export interface OracleOptions {
  maxSourceAgeSecs: number;
  reflectorContract?: string;
  markets: Record<string, SourceSpec[]>;
  /** The environment variable holding the oracle's S... key. */
  keyEnv?: string;
}

/** The perps node's feed route (`PerpsApp::feed_api`). */
export const ROUTE = "oracle";

function options(raw: unknown): OracleOptions {
  const o = raw as Partial<OracleOptions> | null;
  if (!o || typeof o !== "object") throw new Error("perps oracle feed: options must be an object");
  if (typeof o.maxSourceAgeSecs !== "number") throw new Error("perps oracle feed: options.maxSourceAgeSecs must be a number");
  if (!o.markets || typeof o.markets !== "object") throw new Error("perps oracle feed: options.markets is required");
  return o as OracleOptions;
}

/** Test seams; the relayer passes none. */
export interface FeedDeps {
  connect?: Connect;
}

export async function createFeed(host: FeedHost, raw: unknown, deps: FeedDeps = {}): Promise<Feed> {
  const o = options(raw);
  const keyEnv = o.keyEnv ?? "CARAVEL_ORACLE_SECRET";
  const secret = process.env[keyEnv];
  if (!secret) throw new Error(`set ${keyEnv}`);
  const key = Keypair.fromSecret(secret.trim());
  const markets = (await host.getJson("/v1/markets")) as MarketInfo[];
  const reflector = o.reflectorContract ? new RpcReflector(host.rpcUrl, o.reflectorContract, host.networkPassphrase) : null;
  const decimals = reflector ? await reflector.decimals() : 0;
  const streamed = [
    ...new Set(
      Object.values(o.markets)
        .flat()
        .flatMap((s) => ("coinbaseStream" in s ? [s.coinbaseStream] : [])),
    ),
  ];
  const stream = streamed.length
    ? new CoinbaseStream(streamed, deps.connect, undefined, (msg) => host.log("warn", msg))
    : null;
  const source = (s: SourceSpec): PriceSource => {
    if ("coinbaseStream" in s) return stream!.source(s.coinbaseStream, s.maxAgeSecs);
    if ("reflector" in s) {
      if (!reflector) throw new Error("a reflector source needs options.reflectorContract");
      return new ReflectorPrice(reflector, s.reflector, decimals);
    }
    if ("coinbase" in s) return new CoinbaseSpot(s.coinbase);
    return new FixedPrice(s.fixed);
  };
  const feed: FeedMarket[] = markets
    .filter((m) => o.markets[String(m.market_id)])
    .map((m) => ({ info: m, sources: (o.markets[String(m.market_id)] ?? []).map(source) }));
  const feeder = new OracleFeeder((hex) => host.postFeed(ROUTE, hex), key, host.laneId, feed, o.maxSourceAgeSecs);
  stream?.start();
  return {
    name: "oracle",
    describe: () => ({
      oracle_key: key.publicKey(),
      markets: feed.map((m) => ({ id: m.info.market_id, sources: m.sources.map((s) => s.name) })),
      ...(stream ? { coinbase_stream: stream.status().products } : {}),
    }),
    tick: async () => ({ errors: (await feeder.tick()).errors }),
  };
}
