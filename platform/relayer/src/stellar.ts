/**
 * The settlement contract through Stellar RPC
 * (`@stellar/stellar-sdk` 17.2.0). Views run as simulations from a null
 * source account; `submit_checkpoint` is simulated, assembled, signed by the
 * relayer key, sent and polled.
 */
import { Account, BASE_FEE, Contract, Keypair, TransactionBuilder, nativeToScVal, rpc, scValToNative, xdr } from "@stellar/stellar-sdk";

import type { PendingCheckpoint } from "./sequencer.js";

/** A source account for read-only simulations; it need not exist. */
const NULL_ACCOUNT = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

export interface InboxRecord {
  kind: number;
  laneAccount: Uint8Array;
  amount: bigint;
  enqueuedAt: bigint;
  accAfter: Uint8Array;
}

export interface CheckpointRecord {
  headerHash: Uint8Array;
  stellarLedger: number;
}

export interface SubmitResult {
  hash: string;
  ledger: number;
  /** Stroops, from the transaction result. */
  feeCharged: bigint;
  /** Stroops, from simulation. */
  minResourceFee: bigint;
  /** Signed envelope size. */
  txSizeBytes: number;
}

export interface SettlementApi {
  inboxCount(): Promise<bigint>;
  inbox(index: bigint): Promise<InboxRecord | null>;
  lastCheckpoint(): Promise<{ seq: bigint; headerHash: Uint8Array }>;
  checkpoint(seq: bigint): Promise<CheckpointRecord | null>;
  /** The transaction that accepted `seq`, from its `ckpt` event. */
  findCheckpointTx(seq: bigint, fromLedger: number): Promise<{ hash: string; ledger: number } | null>;
  submitCheckpoint(p: PendingCheckpoint): Promise<SubmitResult>;
}

/** A contract error code from a failed simulation, e.g. `Error(Contract, #25)`. */
export function contractErrorCode(message: string): number | null {
  const m = /Error\(Contract, #(\d+)\)/.exec(message);
  return m?.[1] ? Number(m[1]) : null;
}

export class SimulationError extends Error {
  readonly contractCode: number | null;

  constructor(method: string, detail: string) {
    super(`${method}: ${detail}`);
    this.contractCode = contractErrorCode(detail);
  }
}

function bytes(v: unknown, what: string): Uint8Array {
  if (v instanceof Uint8Array) return new Uint8Array(v);
  throw new Error(`${what} is not bytes`);
}

function u64(v: bigint): xdr.ScVal {
  return nativeToScVal(v, { type: "u64" });
}

/** `Sig { signer_index: u32, signature: BytesN<64> }`: a map with sorted symbol keys. */
function sigVal(s: { signer_index: number; signature: Uint8Array }): xdr.ScVal {
  return xdr.ScVal.scvMap([
    new xdr.ScMapEntry({ key: xdr.ScVal.scvSymbol("signature"), val: xdr.ScVal.scvBytes(Buffer.from(s.signature)) }),
    new xdr.ScMapEntry({ key: xdr.ScVal.scvSymbol("signer_index"), val: xdr.ScVal.scvU32(s.signer_index) }),
  ]);
}

export class RpcContract {
  readonly server: rpc.Server;
  readonly contract: Contract;

  constructor(
    rpcUrl: string,
    readonly contractId: string,
    readonly networkPassphrase: string,
  ) {
    this.server = new rpc.Server(rpcUrl, { allowHttp: rpcUrl.startsWith("http://") });
    this.contract = new Contract(contractId);
  }

  /** Simulates `method(args)` and returns the native return value. */
  async view(method: string, ...args: xdr.ScVal[]): Promise<unknown> {
    const tx = new TransactionBuilder(new Account(NULL_ACCOUNT, "0"), { fee: BASE_FEE, networkPassphrase: this.networkPassphrase })
      .addOperation(this.contract.call(method, ...args))
      .setTimeout(30)
      .build();
    const sim = await this.server.simulateTransaction(tx);
    if (rpc.Api.isSimulationError(sim)) throw new SimulationError(method, sim.error);
    if (!sim.result) throw new SimulationError(method, "no result");
    return scValToNative(sim.result.retval);
  }
}

export class RpcSettlement implements SettlementApi {
  private readonly c: RpcContract;

  constructor(
    rpcUrl: string,
    contractId: string,
    networkPassphrase: string,
    private readonly relayer: Keypair,
  ) {
    this.c = new RpcContract(rpcUrl, contractId, networkPassphrase);
  }

  async inboxCount(): Promise<bigint> {
    return BigInt(String(await this.c.view("inbox_count")));
  }

  async inbox(index: bigint): Promise<InboxRecord | null> {
    const m = (await this.c.view("inbox", u64(index))) as Record<string, unknown> | null | undefined;
    if (m === null || m === undefined) return null;
    return {
      kind: Number(m.kind),
      laneAccount: bytes(m.lane_account, "lane_account"),
      amount: BigInt(String(m.amount)),
      enqueuedAt: BigInt(String(m.enqueued_at)),
      accAfter: bytes(m.acc_after, "acc_after"),
    };
  }

  async lastCheckpoint(): Promise<{ seq: bigint; headerHash: Uint8Array }> {
    const l = (await this.c.view("last_checkpoint")) as Record<string, unknown>;
    return { seq: BigInt(String(l.seq)), headerHash: bytes(l.header_hash, "header_hash") };
  }

  async checkpoint(seq: bigint): Promise<CheckpointRecord | null> {
    const r = (await this.c.view("checkpoint", u64(seq))) as Record<string, unknown> | null | undefined;
    if (r === null || r === undefined) return null;
    return { headerHash: bytes(r.header_hash, "header_hash"), stellarLedger: Number(r.stellar_ledger) };
  }

  async findCheckpointTx(seq: bigint, fromLedger: number): Promise<{ hash: string; ledger: number } | null> {
    const topics = [[xdr.ScVal.scvSymbol("ckpt").toXDR("base64"), u64(seq).toXDR("base64")]];
    const res = await this.c.server.getEvents({ startLedger: fromLedger, filters: [{ type: "contract", contractIds: [this.c.contractId], topics }], limit: 5 });
    const e = res.events[0];
    return e ? { hash: e.txHash, ledger: e.ledger } : null;
  }

  async submitCheckpoint(p: PendingCheckpoint): Promise<SubmitResult> {
    const account = await this.c.server.getAccount(this.relayer.publicKey());
    const args = [xdr.ScVal.scvBytes(Buffer.from(p.header)), xdr.ScVal.scvBytes(Buffer.from(p.batch)), u64(p.epoch), xdr.ScVal.scvVec(p.sigs.map(sigVal))];
    const raw = new TransactionBuilder(account, { fee: BASE_FEE, networkPassphrase: this.c.networkPassphrase })
      .addOperation(this.c.contract.call("submit_checkpoint", ...args))
      .setTimeout(120)
      .build();
    const sim = await this.c.server.simulateTransaction(raw);
    if (rpc.Api.isSimulationError(sim)) throw new SimulationError("submit_checkpoint", sim.error);
    const tx = rpc.assembleTransaction(raw, sim).build();
    tx.sign(this.relayer);
    const txSizeBytes = tx.toEnvelope().toXDR().length;
    const sent = await this.c.server.sendTransaction(tx);
    if (sent.status === "ERROR" || sent.status === "TRY_AGAIN_LATER") throw new Error(`submit_checkpoint ${p.seq}: send status ${sent.status}`);
    const done = await this.c.server.pollTransaction(sent.hash, { attempts: 60 });
    if (done.status !== rpc.Api.GetTransactionStatus.SUCCESS) throw new Error(`submit_checkpoint ${p.seq}: ${done.status} (${sent.hash})`);
    return {
      hash: sent.hash,
      ledger: done.ledger,
      feeCharged: BigInt(done.resultXdr.feeCharged),
      minResourceFee: BigInt(sim.minResourceFee),
      txSizeBytes,
    };
  }
}
