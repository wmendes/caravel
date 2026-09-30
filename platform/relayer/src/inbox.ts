/**
 * Inbox watcher (spec §17.1, DEC-040): forwards every inbox message Stellar
 * recorded to the sequencer, in index order. The cursor is the sequencer's
 * own count of reported messages, so a restart resumes exactly where the
 * sequencer is and nothing is lost or sent twice out of order.
 */
import { encodeInboxMsg, toHex } from "./codec.js";
import type { SequencerApi } from "./sequencer.js";
import type { SettlementApi } from "./stellar.js";

export class InboxMismatch extends Error {}

/** Forwards up to `limit` messages; returns how many the sequencer took. */
export async function syncInbox(seq: SequencerApi, st: SettlementApi, limit = 200): Promise<number> {
  let next = BigInt((await seq.status()).inbox.reported);
  const count = await st.inboxCount();
  let sent = 0;
  while (next < count && sent < limit) {
    const m = await st.inbox(next);
    if (!m) throw new Error(`inbox(${next}) is empty but inbox_count() is ${count}`);
    const msg = encodeInboxMsg({ kind: m.kind, index: next, laneAccount: m.laneAccount, amount: m.amount, enqueuedAt: m.enqueuedAt });
    const reply = await seq.postInbox(next, toHex(msg), toHex(m.accAfter));
    switch (reply.status) {
      case "added":
      case "known":
        next += 1n;
        sent += 1;
        break;
      case "gap":
        // The sequencer needs an earlier message first (e.g. it restarted from an older store).
        if (reply.expected >= next) throw new Error(`sequencer asked for ${reply.expected} while at ${next}`);
        next = reply.expected;
        break;
      case "mismatch":
        throw new InboxMismatch(`inbox ${next}: the sequencer's acc chain differs from Stellar's; inbox inclusion is halted`);
    }
  }
  return sent;
}
