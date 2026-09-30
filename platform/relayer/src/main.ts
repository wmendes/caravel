/**
 * The relayer (spec §17): three loops in one process. Each loop step is
 * idempotent, so the process can stop at any point and restart.
 *
 *   node dist/main.js --config relayer.local.json
 */
import { syncInbox, InboxMismatch } from "./inbox.js";
import { submitNext, CheckpointDivergence } from "./checkpoints.js";
import { loadConfig, type SourceSpec } from "./config.js";
import { JsonlMetrics } from "./metrics.js";
import { OracleFeeder, laneIdFromHex, type FeedMarket } from "./oracle.js";
import { CoinbaseSpot, FixedPrice, ReflectorPrice, type PriceSource } from "./prices.js";
import { HttpSequencer } from "./sequencer.js";
import { RpcReflector, RpcSettlement } from "./stellar.js";

function log(level: "info" | "warn" | "error", msg: string, extra: Record<string, unknown> = {}): void {
  const line = JSON.stringify({ t: new Date().toISOString(), level, msg, ...extra }, (_k, v) => (typeof v === "bigint" ? v.toString() : v));
  (level === "info" ? console.log : console.error)(line);
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

let stopping = false;
for (const sig of ["SIGINT", "SIGTERM"] as const) {
  process.on(sig, () => {
    stopping = true;
    log("info", `${sig}: stopping after the current steps`);
  });
}

/** Runs `step` every `everyMs`, never overlapping; `fatal` errors stop the loop. */
async function loop(name: string, everyMs: number, step: () => Promise<void>, fatal: (e: unknown) => boolean): Promise<void> {
  let backoff = everyMs;
  while (!stopping) {
    try {
      await step();
      backoff = everyMs;
    } catch (e) {
      if (fatal(e)) {
        log("error", `${name}: stopped`, { error: String(e) });
        return;
      }
      log("warn", `${name}: ${String(e)}`);
      backoff = Math.min(backoff * 2, 30_000);
    }
    await sleep(backoff);
  }
}

async function main(): Promise<void> {
  const i = process.argv.indexOf("--config");
  const path = i >= 0 ? process.argv[i + 1] : process.env.RELAYER_CONFIG;
  if (!path) throw new Error("usage: relayer --config <file.json>");
  const cfg = await loadConfig(path);
  const f = cfg.file;
  const seq = new HttpSequencer(f.sequencerUrl, cfg.token);
  const st = new RpcSettlement(f.rpcUrl, f.settlementContract, f.networkPassphrase, cfg.relayer ?? (await import("@stellar/stellar-sdk")).Keypair.random());
  const loops: Promise<void>[] = [];

  if (f.loops.inbox) {
    loops.push(
      loop(
        "inbox",
        f.intervalsMs?.inbox ?? 2_000,
        async () => {
          const n = await syncInbox(seq, st);
          if (n > 0) log("info", "inbox forwarded", { messages: n });
        },
        (e) => e instanceof InboxMismatch,
      ),
    );
  }

  if (f.loops.checkpoints) {
    const metrics = new JsonlMetrics(cfg.metricsPath);
    loops.push(
      loop(
        "checkpoints",
        f.intervalsMs?.checkpoints ?? 2_000,
        async () => {
          for (;;) {
            const r = await submitNext(seq, st, metrics);
            if (r.kind === "idle") return;
            log("info", `checkpoint ${r.kind}`, { seq: r.seq, tx: r.hash });
          }
        },
        (e) => e instanceof CheckpointDivergence,
      ),
    );
  }

  if (f.loops.oracle && f.oracle && cfg.oracle) {
    const o = f.oracle;
    const status = await seq.status();
    const markets = await seq.markets();
    const reflector = o.reflectorContract ? new RpcReflector(f.rpcUrl, o.reflectorContract, f.networkPassphrase) : null;
    const decimals = reflector ? await reflector.decimals() : 0;
    const source = (s: SourceSpec): PriceSource => {
      if ("reflector" in s) {
        if (!reflector) throw new Error("a reflector source needs oracle.reflectorContract");
        return new ReflectorPrice(reflector, s.reflector, decimals);
      }
      if ("coinbase" in s) return new CoinbaseSpot(s.coinbase);
      return new FixedPrice(s.fixed);
    };
    const feed: FeedMarket[] = markets
      .filter((m) => o.markets[String(m.market_id)])
      .map((m) => ({ info: m, sources: (o.markets[String(m.market_id)] ?? []).map(source) }));
    const feeder = new OracleFeeder(seq, cfg.oracle, laneIdFromHex(status.lane_id), feed, o.maxSourceAgeSecs);
    log("info", "oracle feeder", { oracle_key: cfg.oracle.publicKey(), markets: feed.map((m) => ({ id: m.info.market_id, sources: m.sources.map((s) => s.name) })) });
    loops.push(
      loop(
        "oracle",
        f.intervalsMs?.oracle ?? 2_000,
        async () => {
          const r = await feeder.tick();
          for (const e of r.errors) log("warn", `oracle: ${e}`);
        },
        () => false,
      ),
    );
  }

  log("info", "relayer started", { sequencer: f.sequencerUrl, rpc: f.rpcUrl, settlement: f.settlementContract, loops: f.loops });
  await Promise.all(loops);
}

main().catch((e) => {
  log("error", String(e));
  process.exit(1);
});
