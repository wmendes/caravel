#!/usr/bin/env node
// M0.5 dependency guard (spec §20.3): the platform must not depend on any lane.
//
// Reads every Cargo.toml under platform/ and lanes/ (plain text, no cargo, no
// network) and follows path dependencies of every kind: normal, dev, build and
// target-specific, directly or through workspace = true. It fails when a
// package under platform/ reaches a package under lanes/.
//
// EXCEPTIONS are the couplings M0 left behind. Each names the task that removes
// it. A listed exception that is no longer needed also fails, so the list only
// shrinks.
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const EXCEPTIONS = {
  "caravel-node": "P-06 puts the node on NodeApp",
  settlement: "P-09 moves its codecs to caravel-core and its tests to caravel-harness",
};

function manifests(dir) {
  const out = [];
  const walk = (d) => {
    for (const name of readdirSync(d)) {
      if (name === "target" || name === "node_modules" || name.startsWith(".")) continue;
      const p = join(d, name);
      if (statSync(p).isDirectory()) walk(p);
      else if (name === "Cargo.toml") out.push(p);
    }
  };
  if (existsSync(dir)) walk(dir);
  return out;
}

const text = (p) => readFileSync(p, "utf8");
const pkgName = (toml) => toml.match(/^\[package\][^[]*?^name\s*=\s*"([^"]+)"/ms)?.[1];

// Workspace dependency paths, from the root and the frozen workspace.
function wsPaths(manifest) {
  const t = text(manifest);
  const start = t.indexOf("[workspace.dependencies]");
  const paths = new Map();
  if (start < 0) return paths;
  const section = t.slice(start).split(/\n\[/)[0];
  for (const m of section.matchAll(/^([A-Za-z0-9_-]+)\s*=\s*\{[^}]*path\s*=\s*"([^"]+)"/gm)) {
    paths.set(m[1], resolve(dirname(manifest), m[2]));
  }
  return paths;
}
const workspaces = [join(root, "Cargo.toml"), join(root, "lanes/perps/engine/Cargo.toml")].map((m) => ({
  dir: dirname(m),
  paths: wsPaths(m),
}));
const wsFor = (manifest) =>
  workspaces
    .filter((w) => manifest.startsWith(w.dir + "/"))
    .sort((a, b) => b.dir.length - a.dir.length)[0];

// Every local package: name -> { dir, deps: Set<dir> }.
const byDir = new Map();
for (const m of [...manifests(join(root, "platform")), ...manifests(join(root, "lanes"))]) {
  const t = text(m);
  const name = pkgName(t);
  if (!name) continue;
  const ws = wsFor(m);
  const deps = new Set();
  const sections = t.split(/\n(?=\[)/).filter((s) => /^\[(target\.[^\]]+\.)?(dev-|build-)?dependencies\]/.test(s));
  for (const s of sections) {
    for (const line of s.split("\n").slice(1)) {
      const d = line.match(/^([A-Za-z0-9_-]+)\s*=\s*(\{.*\}|".*")/);
      if (!d) continue;
      const path = d[2].match(/path\s*=\s*"([^"]+)"/)?.[1];
      if (path) deps.add(resolve(dirname(m), path));
      else if (/workspace\s*=\s*true/.test(d[2]) && ws?.paths.has(d[1])) deps.add(ws.paths.get(d[1]));
    }
  }
  deps.delete(dirname(m));
  byDir.set(dirname(m), { name, deps });
}

const isLane = (dir) => relative(root, dir).startsWith("lanes/");
let failed = false;
const needed = new Set();
for (const [dir, pkg] of byDir) {
  if (!relative(root, dir).startsWith("platform/")) continue;
  // Breadth-first: the first path into lanes/ explains the coupling.
  const seen = new Map([[dir, [pkg.name]]]);
  const queue = [dir];
  let hit = null;
  while (queue.length && !hit) {
    const cur = queue.shift();
    for (const dep of byDir.get(cur)?.deps ?? []) {
      if (seen.has(dep)) continue;
      const trail = [...seen.get(cur), byDir.get(dep)?.name ?? relative(root, dep)];
      seen.set(dep, trail);
      if (isLane(dep)) {
        hit = trail;
        break;
      }
      queue.push(dep);
    }
  }
  if (!hit) continue;
  if (EXCEPTIONS[pkg.name]) {
    needed.add(pkg.name);
    console.log(`check-deps: allowed for now: ${hit.join(" -> ")} (${EXCEPTIONS[pkg.name]})`);
  } else {
    console.error(`check-deps: error: platform package ${pkg.name} reaches a lane: ${hit.join(" -> ")}`);
    failed = true;
  }
}
for (const name of Object.keys(EXCEPTIONS)) {
  if (!needed.has(name)) {
    console.error(`check-deps: error: the exception for ${name} is no longer needed; remove it from scripts/check-deps.mjs`);
    failed = true;
  }
}
if (failed) process.exit(1);
const platform = [...byDir.keys()].filter((d) => relative(root, d).startsWith("platform/")).length;
console.log(`check-deps: ok (${platform} platform packages, ${Object.keys(EXCEPTIONS).length} exceptions left)`);
