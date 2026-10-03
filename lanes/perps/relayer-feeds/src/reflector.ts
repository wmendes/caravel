/**
 * Reflector's SEP-40 feed (`lastprice(Asset::Other(Symbol))`) through Stellar
 * RPC (`@stellar/stellar-sdk` 17.2.0), read by simulation from a null source
 * account.
 */
import { Account, BASE_FEE, Contract, TransactionBuilder, rpc, scValToNative, xdr } from "@stellar/stellar-sdk";

import type { ReflectorReader } from "./prices.js";

/** A source account for read-only simulations; it need not exist. */
const NULL_ACCOUNT = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
/** A price read gives up after this long, so the next source or tick can run. */
export const RPC_TIMEOUT_MS = 10_000;

export class RpcReflector implements ReflectorReader {
  private readonly server: rpc.Server;
  private readonly contract: Contract;

  constructor(
    rpcUrl: string,
    contractId: string,
    private readonly networkPassphrase: string,
    timeoutMs = RPC_TIMEOUT_MS,
  ) {
    this.server = new rpc.Server(rpcUrl, { allowHttp: rpcUrl.startsWith("http://") });
    // stellar-sdk 17.2.0 ignores rpc.Server's `timeout` option and waits
    // forever by default; its HTTP client's defaults apply to every call.
    this.server.httpClient.defaults.timeout = timeoutMs;
    this.contract = new Contract(contractId);
  }

  private async view(method: string, ...args: xdr.ScVal[]): Promise<unknown> {
    const tx = new TransactionBuilder(new Account(NULL_ACCOUNT, "0"), { fee: BASE_FEE, networkPassphrase: this.networkPassphrase })
      .addOperation(this.contract.call(method, ...args))
      .setTimeout(30)
      .build();
    const sim = await this.server.simulateTransaction(tx);
    if (rpc.Api.isSimulationError(sim)) throw new Error(`reflector ${method}: ${sim.error}`);
    if (!sim.result) throw new Error(`reflector ${method}: no result`);
    return scValToNative(sim.result.retval);
  }

  async decimals(): Promise<number> {
    return Number(await this.view("decimals"));
  }

  async lastPrice(asset: string): Promise<{ price: bigint; timestamp: bigint } | null> {
    const arg = xdr.ScVal.scvVec([xdr.ScVal.scvSymbol("Other"), xdr.ScVal.scvSymbol(asset)]);
    const r = (await this.view("lastprice", arg)) as Record<string, unknown> | null | undefined;
    if (r === null || r === undefined) return null;
    return { price: BigInt(String(r.price)), timestamp: BigInt(String(r.timestamp)) };
  }
}
