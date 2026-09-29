/**
 * Oracle feeder (spec §17.3): every 2 s per market, a USD price from the
 * first fresh source, converted to stroops per lot, snapped to the tick,
 * signed with the oracle key and posted to the sequencer. A publish is
 * skipped if the price moved less than one tick and less than 10 s passed.
 */
import { Keypair } from "@stellar/stellar-sdk";

import { encodeOracleUpdate, fromHex, oracleSigningPreimage, sha256, toHex } from "./codec.js";
import { firstFresh, pricePerLot, snapToTick, type PriceSource } from "./prices.js";
import type { MarketInfo, SequencerApi } from "./sequencer.js";

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
    private readonly seq: SequencerApi,
    private readonly key: Keypair,
    private readonly laneId: Uint8Array,
    private readonly markets: FeedMarket[],
    private readonly maxSourceAgeSecs: number,
    private readonly now: () => number = Date.now,
  ) {}

  /** One round over every market; returns what was published. Errors are per market. */
  async tick(): Promise<{ published: Published[]; errors: string[] }> {
    const published: Published[] = [];
    const errors: string[] = [];
    for (const m of this.markets) {
      try {
        const p = await this.one(m);
        if (p) published.push(p);
      } catch (e) {
        errors.push(`market ${m.info.market_id}: ${String(e)}`);
      }
    }
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
    await this.seq.postOracle(toHex(update));
    this.last.set(m.info.market_id, { price, atMs: nowMs });
    return { ...fields, source: q.source };
  }
}

export function laneIdFromHex(hex: string): Uint8Array {
  const b = fromHex(hex);
  if (b.length !== 32) throw new Error("lane_id must be 32 bytes");
  return b;
}
