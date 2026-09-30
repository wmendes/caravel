import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

// Spec §2.2 (what M0 must not claim) and §18.1 (copy rules). A line may opt out
// with a `claims-ok:` comment that says why, e.g. the /about page listing what
// is not claimed.
const BANNED: [RegExp, string][] = [
  [/\btrustless\b/i, "§2.2: the lane is not trustless"],
  [/\baudited\b/i, "§2.2: nothing is audited"],
  [/\bmainnet\b/i, "§2.2: M0 is testnet only"],
  [/\bproduction\b/i, "§2.2: not production"],
  [/\bfirst ever\b/i, "§2.2: no 'first ever'"],
  [/\bL1\b/, "§18.1: say Stellar, never L1"],
];

function sourceFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return sourceFiles(path);
    return /\.(tsx?|css|html)$/.test(name) && !name.endsWith(".test.ts") ? [path] : [];
  });
}

describe("UI copy follows the honest-claims rules", () => {
  const files = [...sourceFiles(join(import.meta.dirname, ".")), join(import.meta.dirname, "..", "index.html")];

  it("finds source files to check", () => {
    expect(files.length).toBeGreaterThan(0);
  });

  it("contains no banned claims", () => {
    const hits: string[] = [];
    for (const file of files) {
      readFileSync(file, "utf8").split("\n").forEach((line, i) => {
        if (line.includes("claims-ok:")) return;
        for (const [re, why] of BANNED) if (re.test(line)) hits.push(`${file}:${i + 1} ${why}`);
      });
    }
    expect(hits).toEqual([]);
  });
});
