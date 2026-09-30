import { afterEach, describe, expect, it, vi } from "vitest";

import { ApiError, lane } from "./lane";

function respond(status: number, type: string, body: string) {
  vi.stubGlobal("fetch", vi.fn(async () => new Response(body, { status, headers: { "content-type": type } })));
}

afterEach(() => vi.unstubAllGlobals());

describe("the lane API client", () => {
  it("names a host that serves only the app", async () => {
    respond(200, "text/html; charset=utf-8", "<!doctype html><html></html>");
    const err = await lane.status().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).code).toBe("NOT_THE_API");
    expect((err as ApiError).message).toContain("text/html");
  });

  it("names it on a 404 page too", async () => {
    respond(404, "text/html", "Not Found");
    await expect(lane.status()).rejects.toMatchObject({ code: "NOT_THE_API" });
  });

  it("keeps the API's own JSON errors", async () => {
    respond(404, "application/json", JSON.stringify({ code: "NOT_FOUND", error: "no such account" }));
    await expect(lane.account("GA")).resolves.toBeNull();
    respond(400, "application/json", JSON.stringify({ code: "BAD_ACCOUNT", error: "not a G... key" }));
    await expect(lane.account("GA")).rejects.toMatchObject({ status: 400, code: "BAD_ACCOUNT" });
  });

  it("returns the API's JSON", async () => {
    respond(200, "application/json", JSON.stringify({ height: "7" }));
    await expect(lane.status()).resolves.toMatchObject({ height: "7" });
  });
});
