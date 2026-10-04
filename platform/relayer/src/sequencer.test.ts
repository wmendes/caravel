import { describe, expect, it } from "vitest";

import { HttpSequencer } from "./sequencer.js";

/** A fetch that fails with "fetch failed" `failures` times, then answers `status`. */
function flaky(failures: number, status: number) {
  const calls: string[] = [];
  const impl = (async (url: string | URL | Request, init?: RequestInit) => {
    calls.push(`${init?.method ?? "GET"} ${String(url)}`);
    if (calls.length <= failures) throw new TypeError("fetch failed");
    return new Response(status === 204 ? null : "{}", { status });
  }) as typeof fetch;
  return { impl, calls };
}

describe("HttpSequencer (F-14)", () => {
  it("sends a GET again once when the connection drops", async () => {
    const f = flaky(1, 204);
    const seq = new HttpSequencer("http://seq", "t", f.impl);
    expect(await seq.pendingCheckpoint(5000)).toBeNull();
    expect(f.calls).toEqual(["GET http://seq/internal/checkpoints/pending?wait_ms=5000", "GET http://seq/internal/checkpoints/pending?wait_ms=5000"]);
  });

  it("gives up after the second try", async () => {
    const f = flaky(2, 204);
    await expect(new HttpSequencer("http://seq", "t", f.impl).pendingCheckpoint()).rejects.toThrow(/fetch failed/);
    expect(f.calls).toHaveLength(2);
  });

  it("never sends a POST twice", async () => {
    const f = flaky(1, 200);
    await expect(new HttpSequencer("http://seq", "t", f.impl).reportAccepted(1n, "ab", 7)).rejects.toThrow(/fetch failed/);
    expect(f.calls).toHaveLength(1);
  });
});
