/**
 * The settlement contract and USDC on Stellar (spec §13, §18.3). Views are
 * simulations from a null source account; writes are built here, signed in
 * the connected wallet (Stellar Wallets Kit) and sent through RPC.
 */
import { Buffer } from "buffer";
import { Account, Address, Asset, BASE_FEE, Contract, Operation, StrKey, TransactionBuilder, nativeToScVal, rpc, scValToNative, xdr } from "@stellar/stellar-sdk";

import { config } from "../config";
import { fromHex } from "../codec/bytes";
import { signTx } from "./wallet";

const NULL_ACCOUNT = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
const server = new rpc.Server(config.rpcUrl, { allowHttp: config.rpcUrl.startsWith("http://") });
const settlement = new Contract(config.settlementContract);
const usdc = new Contract(config.usdcContract);

export const u64 = (v: bigint | number) => nativeToScVal(BigInt(v), { type: "u64" });
export const u32 = (v: number) => nativeToScVal(v, { type: "u32" });
export const i128 = (v: bigint | string) => nativeToScVal(BigInt(v), { type: "i128" });
export const addr = (g: string) => new Address(g).toScVal();
export const bytes = (b: Uint8Array) => xdr.ScVal.scvBytes(Buffer.from(b));
export const hashes = (hs: string[]) => xdr.ScVal.scvVec(hs.map((h) => bytes(fromHex(h))));
/** A G... account's raw key, as the contract's `lane_account`. */
export const laneAccount = (g: string) => bytes(StrKey.decodeEd25519PublicKey(g));

async function view(c: Contract, method: string, ...args: xdr.ScVal[]): Promise<unknown> {
  const tx = new TransactionBuilder(new Account(NULL_ACCOUNT, "0"), { fee: BASE_FEE, networkPassphrase: config.networkPassphrase }).addOperation(c.call(method, ...args)).setTimeout(30).build();
  const sim = await server.simulateTransaction(tx);
  if (rpc.Api.isSimulationError(sim)) throw new Error(sim.error);
  return sim.result ? scValToNative(sim.result.retval) : undefined;
}

/** Contract error codes (DEC-032) that users can hit, in plain words. */
const ERRORS: Record<number, string> = {
  10: "the settlement contract is frozen",
  11: "below the minimum deposit",
  12: "the amount must be positive",
  13: "only the account's owner can request this",
  39: "not enough funds in the vault for this checkpoint",
  50: "this proof is for another account",
  51: "unknown checkpoint",
  53: "already claimed",
  54: "the proof does not match the checkpoint",
  60: "the contract is not frozen",
  61: "freezing is not allowed yet",
  62: "this deposit cannot be refunded",
  64: "the escape payout does not compute, so the claim stays open",
};

export function explain(e: unknown): string {
  const s = String(e instanceof Error ? e.message : e);
  const m = /Error\(Contract, #(\d+)\)/.exec(s);
  const code = m?.[1] ? Number(m[1]) : null;
  if (code !== null && ERRORS[code]) return `Stellar refused it: ${ERRORS[code]} (error ${code}).`;
  if (/trustline|TrustLine/i.test(s)) return "Your account has no USDC trustline. Add USDC in your wallet first.";
  return s.length > 240 ? `${s.slice(0, 240)}…` : s;
}

/** Signs a built transaction in the wallet, sends it and waits. Returns its hash. */
async function signAndSend(tx: ReturnType<TransactionBuilder["build"]>, source: string): Promise<string> {
  const signed = TransactionBuilder.fromXDR(await signTx(tx.toXDR(), source), config.networkPassphrase);
  const sent = await server.sendTransaction(signed);
  if (sent.status === "ERROR" || sent.status === "TRY_AGAIN_LATER") throw new Error(`Stellar did not take the transaction (${sent.status}).`);
  const done = await server.pollTransaction(sent.hash, { attempts: 60 });
  if (done.status !== rpc.Api.GetTransactionStatus.SUCCESS) throw new Error(`The transaction failed on Stellar (${done.status}).`);
  return sent.hash;
}

/** Builds, simulates, signs in your wallet, sends and waits. Returns the transaction hash. */
async function invoke(source: string, c: Contract, method: string, ...args: xdr.ScVal[]): Promise<string> {
  const account = await server.getAccount(source);
  const raw = new TransactionBuilder(account, { fee: BASE_FEE, networkPassphrase: config.networkPassphrase }).addOperation(c.call(method, ...args)).setTimeout(180).build();
  const prepared = await server.prepareTransaction(raw);
  const signed = TransactionBuilder.fromXDR(await signTx(prepared.toXDR(), source), config.networkPassphrase);
  const sent = await server.sendTransaction(signed);
  if (sent.status === "ERROR" || sent.status === "TRY_AGAIN_LATER") throw new Error(`Stellar did not take the transaction (${sent.status}).`);
  const done = await server.pollTransaction(sent.hash, { attempts: 60 });
  if (done.status !== rpc.Api.GetTransactionStatus.SUCCESS) throw new Error(`The transaction failed on Stellar (${done.status}).`);
  return sent.hash;
}

/** The settlement contract's exit timers (`config().params`), in seconds. */
export interface ExitParams {
  escape_timeout_secs: bigint;
  force_inclusion_window_secs: bigint;
}

/** A DEX quote: `xlm` (7-decimal string) buys `usdc` along `path`. */
export interface UsdcQuote {
  usdc: string;
  xlm: string;
  path: { asset_type: string; asset_code?: string; asset_issuer?: string }[];
}

function usdcAsset(): Asset {
  const [code, issuer] = config.usdcAsset.split(":");
  return new Asset(code!, issuer!);
}

function stroops(s: string): bigint {
  const [w, f = ""] = s.split(".");
  return BigInt(w!) * 10_000_000n + BigInt((f + "0000000").slice(0, 7));
}

function amount(v: bigint): string {
  return `${v / 10_000_000n}.${(v % 10_000_000n).toString().padStart(7, "0")}`;
}

export interface LastCheckpoint {
  seq: bigint;
  accepted_at: bigint;
  inbox_through: bigint;
}

export const stellar = {
  /** USDC balance in stroops, or null without a trustline. */
  async usdcBalance(g: string): Promise<bigint | null> {
    try {
      return BigInt(String(await view(usdc, "balance", addr(g))));
    } catch {
      return null;
    }
  },
  async lastCheckpoint(): Promise<LastCheckpoint> {
    const l = (await view(settlement, "last_checkpoint")) as Record<string, unknown>;
    return { seq: BigInt(String(l.seq)), accepted_at: BigInt(String(l.accepted_at)), inbox_through: BigInt(String(l.inbox_through)) };
  },
  async params(): Promise<ExitParams> {
    const c = (await view(settlement, "config")) as { params: Record<string, unknown> };
    return { escape_timeout_secs: BigInt(String(c.params.escape_timeout_secs)), force_inclusion_window_secs: BigInt(String(c.params.force_inclusion_window_secs)) };
  },
  /** When the oldest message the lane has not processed was queued on Stellar (Unix s), if any. */
  async oldestUnprocessed(inboxThrough: bigint): Promise<bigint | null> {
    if ((await this.inboxCount()) <= inboxThrough) return null;
    const m = (await view(settlement, "inbox", u64(inboxThrough))) as Record<string, unknown> | null | undefined;
    return m ? BigInt(String(m.enqueued_at)) : null;
  },
  async frozen(): Promise<boolean> {
    return Boolean(await view(settlement, "frozen"));
  },
  async frozenInfo(): Promise<{ payout_num: bigint; payout_den: bigint } | null> {
    const f = (await view(settlement, "frozen_info")) as Record<string, unknown> | null | undefined;
    return f ? { payout_num: BigInt(String(f.payout_num)), payout_den: BigInt(String(f.payout_den)) } : null;
  },
  async isClaimed(seq: string | bigint, index: number): Promise<boolean> {
    return Boolean(await view(settlement, "is_claimed", u64(BigInt(seq)), u32(index)));
  },
  async escapeClaimed(g: string): Promise<boolean> {
    return Boolean(await view(settlement, "escape_claimed", laneAccount(g)));
  },
  async inboxCount(): Promise<bigint> {
    return BigInt(String(await view(settlement, "inbox_count")));
  },
  async inbox(index: bigint): Promise<{ kind: number; from: string; amount: bigint; refunded: boolean } | null> {
    const m = (await view(settlement, "inbox", u64(index))) as Record<string, unknown> | null | undefined;
    return m ? { kind: Number(m.kind), from: String(m.from), amount: BigInt(String(m.amount)), refunded: Boolean(m.refunded) } : null;
  },
  /** Whether the account exists on Stellar, and its XLM in stroops. */
  async xlm(g: string): Promise<bigint | null> {
    try {
      const r = await fetch(`${config.horizonUrl}/accounts/${g}`);
      if (r.status === 404) return null;
      if (!r.ok) throw new Error(`Horizon answered ${r.status}`);
      const a = (await r.json()) as { balances: { asset_type: string; balance: string }[] };
      return stroops(a.balances.find((b) => b.asset_type === "native")?.balance ?? "0");
    } catch (e) {
      if (e instanceof Error && /Horizon answered/.test(e.message)) throw e;
      throw new Error("Horizon is unreachable; try again in a moment.");
    }
  },
  /** Funds a new testnet account with XLM from friendbot. */
  async friendbot(g: string): Promise<void> {
    const r = await fetch(`${config.friendbotUrl}/?addr=${encodeURIComponent(g)}`);
    if (!r.ok && r.status !== 400) throw new Error(`Friendbot answered ${r.status}; try again in a moment, or fund the account from your wallet.`);
  },
  /** The XLM the testnet DEX wants now for exactly `usdc` USDC (stroops). */
  async quoteUsdc(usdc: bigint): Promise<UsdcQuote> {
    const a = usdcAsset();
    const q = new URLSearchParams({ source_assets: "native", destination_asset_type: "credit_alphanum4", destination_asset_code: a.getCode(), destination_asset_issuer: a.getIssuer() ?? "", destination_amount: amount(usdc) });
    const r = await fetch(`${config.horizonUrl}/paths/strict-receive?${q}`);
    if (!r.ok) throw new Error(`Horizon answered ${r.status} for a quote.`);
    const recs = ((await r.json()) as { _embedded: { records: { source_amount: string; path: UsdcQuote["path"] }[] } })._embedded.records;
    if (!recs.length) throw new Error("The testnet DEX has no XLM to USDC liquidity for that amount right now. Try a smaller amount, or the Circle faucet.");
    const best = recs.reduce((x, y) => (stroops(y.source_amount) < stroops(x.source_amount) ? y : x));
    return { usdc: amount(usdc), xlm: best.source_amount, path: best.path };
  },
  /**
   * One transaction, one signature: the USDC trustline if the account has
   * none, then a strict-receive path payment of exactly `q.usdc` for at most
   * the quoted XLM plus `slippageBps`.
   */
  async buyUsdc(g: string, q: UsdcQuote, needsTrustline: boolean, slippageBps = 200): Promise<string> {
    const account = await server.getAccount(g);
    const b = new TransactionBuilder(account, { fee: (Number(BASE_FEE) * 10).toString(), networkPassphrase: config.networkPassphrase });
    if (needsTrustline) b.addOperation(Operation.changeTrust({ asset: usdcAsset() }));
    const sendMax = (stroops(q.xlm) * BigInt(10_000 + slippageBps)) / 10_000n;
    b.addOperation(
      Operation.pathPaymentStrictReceive({
        sendAsset: Asset.native(),
        sendMax: amount(sendMax),
        destination: g,
        destAsset: usdcAsset(),
        destAmount: q.usdc,
        path: q.path.map((p) => (p.asset_type === "native" ? Asset.native() : new Asset(p.asset_code!, p.asset_issuer!))),
      }),
    );
    return signAndSend(b.setTimeout(180).build(), g);
  },
  deposit: (g: string, amount: bigint) => invoke(g, settlement, "deposit", addr(g), i128(amount), laneAccount(g)),
  requestForcedWithdrawal: (g: string, amount: bigint) => invoke(g, settlement, "request_forced_withdrawal", addr(g), laneAccount(g), i128(amount)),
  claimWithdrawal: (g: string, p: { seq: string; index: number; amount: string; proof: string[] }) =>
    invoke(g, settlement, "claim_withdrawal", addr(g), laneAccount(g), u64(BigInt(p.seq)), u32(p.index), i128(p.amount), hashes(p.proof)),
  escapeClaim: (g: string, p: { index: number; equity: string; proof: string[] }) => invoke(g, settlement, "escape_claim", addr(g), laneAccount(g), u32(p.index), i128(p.equity), hashes(p.proof)),
  refund: (g: string, index: bigint) => invoke(g, settlement, "refund_unprocessed_deposit", u64(index)),
  freeze: (g: string) => invoke(g, settlement, "freeze"),
};
