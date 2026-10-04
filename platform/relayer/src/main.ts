/**
 * The relayer (spec §17): the inbox and checkpoint loops, and one loop per
 * feed module the config names (DEC-053). Each loop step is idempotent, so
 * the process can stop at any point and restart.
 *
 *   node dist/main.js --config relayer.local.json
 */
import { syncInbox, InboxMismatch } from "./inbox.js";
import { submitNext, CheckpointDivergence } from "./checkpoints.js";
import { fromHex } from "./codec.js";
import { loadConfig } from "./config.js";
import { FEED_DEADLINE_MS, loadFeedModule, withDeadline, type FeedHost } from "./feeds.js";
import { JsonlMetrics } from "./metrics.js";
import { HttpSequencer } from "./sequencer.js";
import { RpcSettlement } from "./stellar.js";

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

/**
 * Runs `step` every `everyMs`, never overlapping: a step that took part of
 * the interval waits only for the rest, so a 500 ms feed publishes every
 * 500 ms. `fatal` errors stop the loop; others back off up to 30 s.
 */
async function loop(name: string, everyMs: number, step: () => Promise<void>, fatal: (e: unknown) => boolean): Promise<void> {
  let backoff = everyMs;
  while (!stopping) {
    const started = Date.now();
    try {
      await step();
      backoff = everyMs;
      await sleep(Math.max(0, everyMs - (Date.now() - started)));
      continue;
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

  if (f.feeds?.length) {
    const status = await seq.status();
    const host: FeedHost = {
      laneId: fromHex(status.lane_id),
      rpcUrl: f.rpcUrl,
      networkPassphrase: f.networkPassphrase,
      getJson: (path) => seq.getJson(path),
      postFeed: (route, updateHex) => seq.postFeed(route, updateHex),
      log,
    };
    for (const spec of f.feeds) {
      const feed = await (await loadFeedModule(spec, cfg.dir)).createFeed(host, spec.options ?? {});
      log("info", `feed ${feed.name}`, { module: spec.module, ...feed.describe() });
      loops.push(
        loop(
          feed.name,
          spec.intervalMs ?? 2_000,
          async () => {
            const r = await withDeadline(feed.tick(), spec.deadlineMs ?? FEED_DEADLINE_MS, `${feed.name} tick`);
            for (const e of r.errors) log("warn", `${feed.name}: ${e}`);
          },
          () => false,
        ),
      );
    }
  }

  log("info", "relayer started", { sequencer: f.sequencerUrl, rpc: f.rpcUrl, settlement: f.settlementContract, loops: f.loops, feeds: (f.feeds ?? []).map((x) => x.module) });
  await Promise.all(loops);
}

main().catch((e) => {
  log("error", String(e));
  process.exit(1);
});
