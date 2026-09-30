/** The sequencer's public and internal API (spec §14.4, DEC-036). */

export interface SequencerStatus {
  lane_id: string;
  height: string;
  inbox: { reported: string; reported_acc: string; processed: string; halted: boolean };
}

export interface PendingCheckpoint {
  seq: bigint;
  header: Uint8Array;
  batch: Uint8Array;
  epoch: bigint;
  sigs: { signer_index: number; signature: Uint8Array }[];
}

export type InboxReply = { status: "added" | "known" } | { status: "gap"; expected: bigint } | { status: "mismatch" };

export interface SequencerApi {
  status(): Promise<SequencerStatus>;
  /** Any public GET path, for feed modules. */
  getJson(path: string): Promise<unknown>;
  postInbox(index: bigint, msgHex: string, accAfterHex: string): Promise<InboxReply>;
  /** `POST /internal/{route}`: an app's feed update (DEC-053). */
  postFeed(route: string, updateHex: string): Promise<void>;
  pendingCheckpoint(): Promise<PendingCheckpoint | null>;
  reportAccepted(seq: bigint, stellarTxHash: string, ledger: number): Promise<void>;
}

function hexBytes(s: unknown, what: string): Uint8Array {
  if (typeof s !== "string" || !/^([0-9a-f]{2})*$/.test(s)) throw new Error(`${what} is not hex`);
  return new Uint8Array(Buffer.from(s, "hex"));
}

export class HttpSequencer implements SequencerApi {
  constructor(
    private readonly baseUrl: string,
    private readonly token: string,
    private readonly fetchImpl: typeof fetch = fetch,
  ) {}

  private async request(path: string, init: RequestInit = {}, internal = false): Promise<Response> {
    const headers = new Headers(init.headers);
    if (internal) headers.set("authorization", `Bearer ${this.token}`);
    if (init.body !== undefined) headers.set("content-type", "application/json");
    return this.fetchImpl(`${this.baseUrl.replace(/\/$/, "")}${path}`, { ...init, headers, signal: AbortSignal.timeout(10_000) });
  }

  private async json<T>(r: Response, what: string): Promise<T> {
    if (!r.ok) throw new Error(`${what}: HTTP ${r.status} ${await r.text()}`);
    return (await r.json()) as T;
  }

  async status(): Promise<SequencerStatus> {
    return this.json(await this.request("/v1/status"), "status");
  }

  async getJson(path: string): Promise<unknown> {
    if (!path.startsWith("/v1/")) throw new Error(`not a public path: ${path}`);
    return this.json(await this.request(path), path);
  }

  async postInbox(index: bigint, msgHex: string, accAfterHex: string): Promise<InboxReply> {
    const r = await this.request("/internal/inbox", { method: "POST", body: JSON.stringify({ index: index.toString(), msg_hex: msgHex, acc_after_hex: accAfterHex }) }, true);
    if (r.status === 409) {
      const body = (await r.json()) as { code?: string; error?: string; expected?: string };
      if (body.code === "INBOX_GAP") {
        if (body.expected === undefined) throw new Error(`inbox gap without an index: ${body.error}`);
        return { status: "gap", expected: BigInt(body.expected) };
      }
      if (body.code === "INBOX_MISMATCH") return { status: "mismatch" };
      throw new Error(`inbox ${index}: ${body.code} ${body.error}`);
    }
    const body = await this.json<{ status: string }>(r, `inbox ${index}`);
    return { status: body.status === "added" ? "added" : "known" };
  }

  async postFeed(route: string, updateHex: string): Promise<void> {
    if (!/^[a-z][a-z0-9-]*$/.test(route)) throw new Error(`bad feed route: ${route}`);
    await this.json(await this.request(`/internal/${route}`, { method: "POST", body: JSON.stringify({ update: updateHex }) }, true), route);
  }

  async pendingCheckpoint(): Promise<PendingCheckpoint | null> {
    const r = await this.request("/internal/checkpoints/pending", {}, true);
    if (r.status === 204) return null;
    const v = await this.json<{ seq: string; header: string; batch: string; epoch: string; sigs: { signer_index: number; signature: string }[] }>(r, "pending checkpoint");
    return {
      seq: BigInt(v.seq),
      header: hexBytes(v.header, "header"),
      batch: hexBytes(v.batch, "batch"),
      epoch: BigInt(v.epoch),
      sigs: v.sigs.map((s) => ({ signer_index: s.signer_index, signature: hexBytes(s.signature, "signature") })),
    };
  }

  async reportAccepted(seq: bigint, stellarTxHash: string, ledger: number): Promise<void> {
    await this.json(
      await this.request(`/internal/checkpoints/${seq}/accepted`, { method: "POST", body: JSON.stringify({ stellar_tx_hash: stellarTxHash, ledger }) }, true),
      `accepted ${seq}`,
    );
  }
}
