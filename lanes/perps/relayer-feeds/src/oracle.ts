/**
 * Oracle feeder (spec §17.3): on every tick of the relayer's feed loop
 * (`intervalMs`, 500 ms on testnet), for every market at once, a USD price
 * from the first fresh source, converted to stroops per lot, snapped to the tick,
 * signed with the oracle key and posted to the sequencer's feed route. A
 * publish is skipped if the price moved less than one tick and less than
 * 10 s passed.
 */
import { Keypair } from "@stellar/stellar-sdk";

import { encodeOracleUpdate, oracleSigningPreimage, sha256, toHex } from "./codec.js";
import { firstFresh, pricePerLot, snapToTick, type PriceSource } from "./prices.js";

/** A market as `GET /v1/markets` lists it. */
export interface MarketInfo {
  market_id: number;
  symbol: string;
  tick: string;
  display_lot_base_units: number;
  display_base_decimals: number;
}

/** Posts one encoded update to the sequencer. */
export type PostUpdate = (updateHex: string) => Promise<void>;

export const HEARTBEAT_MS = 10_000;

export interface FeedMarket {
  info: MarketInfo;
  sources: PriceSource[];
}

export interface Published {
  marketId: number;
  price: bigint;
  publishTimeMs: bigint;
  source: string;
}

export class OracleFeeder {
  private readonly last = new Map<number, { price: bigint; atMs: number }>();

  constructor(
    private readonly post: PostUpdate,
    private readonly key: Keypair,
    private readonly laneId: Uint8Array,
    private readonly markets: FeedMarket[],
    private readonly maxSourceAgeSecs: number,
    private readonly now: () => number = Date.now,
  ) {}

  /** One round over every market; returns what was published. Errors are per market. */
  async tick(): Promise<{ published: Published[]; errors: string[] }> {
    // Markets in parallel: a slow fallback source for one does not hold up the others.
    const results = await Promise.allSettled(this.markets.map((m) => this.one(m)));
    const published: Published[] = [];
    const errors: string[] = [];
    results.forEach((r, i) => {
      if (r.status === "fulfilled") {
        if (r.value) published.push(r.value);
      } else {
        errors.push(`market ${this.markets[i]!.info.market_id}: ${String(r.reason)}`);
      }
    });
    return { published, errors };
  }

  private async one(m: FeedMarket): Promise<Published | null> {
    const nowMs = this.now();
    const q = await firstFresh(m.sources, this.maxSourceAgeSecs, Math.floor(nowMs / 1000));
    const tick = BigInt(m.info.tick);
    const price = snapToTick(pricePerLot(q.usd, BigInt(m.info.display_lot_base_units), m.info.display_base_decimals), tick);
    const prev = this.last.get(m.info.market_id);
    const diff = prev ? (price > prev.price ? price - prev.price : prev.price - price) : tick;
    if (prev && diff < tick && nowMs - prev.atMs < HEARTBEAT_MS) return null;
    const fields = { marketId: m.info.market_id, price, publishTimeMs: BigInt(nowMs) };
    const signature = this.key.sign(Buffer.from(sha256(oracleSigningPreimage(this.laneId, fields))));
    const update = encodeOracleUpdate({ ...fields, oracleKey: this.key.rawPublicKey(), signature });
    await this.post(toHex(update));
    this.last.set(m.info.market_id, { price, atMs: nowMs });
    return { ...fields, source: q.source };
  }
}
