import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";

import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { RpcReflector } from "./reflector.js";

const CONTRACT = "CCYOZJCOPG34LLQQ7N24YXBM7LL62R7ONMZ3G6WZAAYPB5OYKOMJRN63";

// An RPC server that takes the request and never answers: the 2026-10-03
// testnet oracle stopped for 19 hours behind a call like this.
let server: Server;
let url = "";
beforeAll(async () => {
  server = createServer(() => {});
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  url = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterAll(() => {
  server.closeAllConnections();
  server.close();
});

describe("Reflector over RPC", () => {
  it("gives up on an RPC that never answers instead of waiting forever", async () => {
    const r = new RpcReflector(url, CONTRACT, "Test SDF Network ; September 2015", 200);
    const started = Date.now();
    await expect(r.lastPrice("BTC")).rejects.toThrow();
    expect(Date.now() - started).toBeLessThan(5_000);
  });
});
