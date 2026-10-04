#!/usr/bin/env node
// Checks that pinned versions match versions.json (spec §7) and that no
// placeholder outlives its task. No dependencies: runs on a bare Node >= 22.
import { readFileSync, readdirSync, existsSync } from "node:fs";
import { join } from "node:path";

const root = new URL("..", import.meta.url).pathname;
const read = (p) => readFileSync(join(root, p), "utf8");
const versions = JSON.parse(read("versions.json"));
const errors = [];
const fail = (msg) => errors.push(msg);

// Upstream duplicates we cannot remove. soroban-env-host 28.0.2 and stellar-xdr
// 28.0.0 require stellar-strkey ^0.0.13 (exactly 0.0.13), soroban-sdk 28.0.0
// pins =0.0.16. Caravel code uses the pinned 0.0.16.
const ALLOWED_DUPLICATES = new Set(["stellar-strkey"]);

// --- Rust: workspace pins, member manifests and Cargo.lock -----------------
function tomlSection(text, header) {
  const start = text.indexOf(`[${header}]`);
  if (start < 0) return "";
  const rest = text.slice(start + header.length + 2);
  const next = rest.search(/^\[/m);
  return next < 0 ? rest : rest.slice(0, next);
}
function depVersion(line) {
  const m = line.match(/^\s*([A-Za-z0-9_-]+)\s*=\s*(?:"([^"]+)"|\{[^}]*version\s*=\s*"([^"]+)")/);
  return m ? { name: m[1], version: m[2] ?? m[3] } : null;
}

// Two workspaces: the root and the frozen perps engine (DEC-051). Both pin
// the same crates, and their locks resolve them to the same versions.
function checkWorkspace(ws, memberRoots) {
  const at = (p) => (ws === "." ? p : join(ws, p));
  const cargoToml = read(at("Cargo.toml"));
  const wsDeps = new Map();
  for (const line of tomlSection(cargoToml, "workspace.dependencies").split("\n")) {
    const d = depVersion(line);
    if (d) wsDeps.set(d.name, d.version);
  }
  for (const [name, want] of Object.entries(versions.crates)) {
    if (want.startsWith("RESOLVE_IN_")) continue;
    const have = wsDeps.get(name);
    if (have === undefined) fail(`${at("Cargo.toml")} [workspace.dependencies] is missing ${name} (versions.json: ${want})`);
    else if (have !== want) fail(`${at("Cargo.toml")} pins ${name} ${have}, versions.json says ${want}`);
  }

  // Members must inherit pinned crates from the workspace, never restate a version.
  const memberDirs = memberRoots.flatMap((d) =>
    existsSync(join(root, at(d))) ? readdirSync(join(root, at(d))).map((m) => join(at(d), m)) : [],
  );
  for (const dir of memberDirs) {
    const manifest = join(dir, "Cargo.toml");
    if (!existsSync(join(root, manifest))) continue;
    for (const line of read(manifest).split("\n")) {
      const d = depVersion(line);
      if (d && d.name in versions.crates) fail(`${manifest} sets a version for ${d.name}; use { workspace = true }`);
    }
  }

  const lock = existsSync(join(root, at("Cargo.lock"))) ? read(at("Cargo.lock")) : "";
  if (!lock) fail(`${at("Cargo.lock")} is missing`);
  const locked = new Map();
  for (const m of lock.matchAll(/\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"/g)) {
    if (!locked.has(m[1])) locked.set(m[1], []);
    locked.get(m[1]).push(m[2]);
  }
  for (const [name, want] of Object.entries(versions.crates)) {
    if (want.startsWith("RESOLVE_IN_")) continue;
    const exact = want.replace(/^=/, "");
    const have = locked.get(name) ?? [];
    if (!have.includes(exact)) fail(`${at("Cargo.lock")} resolves ${name} to [${have.join(", ")}], versions.json says ${exact}`);
    if (have.length > 1 && !ALLOWED_DUPLICATES.has(name)) fail(`${at("Cargo.lock")} has several versions of ${name}: ${have.join(", ")}`);
  }
}
checkWorkspace(".", ["platform/crates", "platform/contracts"]);
checkWorkspace("lanes/perps/engine", ["crates", "contracts"]);

// --- Toolchain ---------------------------------------------------------------
const toolchain = read("rust-toolchain.toml").match(/channel\s*=\s*"([^"]+)"/)?.[1];
if (toolchain !== versions.rust_toolchain) fail(`rust-toolchain.toml channel ${toolchain}, versions.json says ${versions.rust_toolchain}`);
if (!read("rust-toolchain.toml").includes(`"${versions.wasm_target}"`)) fail(`rust-toolchain.toml does not list target ${versions.wasm_target}`);
// The Stellar CLI the installer downloads and CI installs (H-03).
for (const [file, re] of [
  ["scripts/install.sh", /^STELLAR_CLI_VERSION="([^"]+)"/m],
  [".github/workflows/ci.yml", /STELLAR_CLI_VERSION: "([^"]+)"/],
  [".github/workflows/release.yml", /STELLAR_CLI_VERSION: "([^"]+)"/],
]) {
  const m = read(file).match(re);
  if (!m) fail(`${file} does not pin STELLAR_CLI_VERSION`);
  else if (m[1] !== versions.stellar_cli) fail(`${file} pins the Stellar CLI ${m[1]}, versions.json says ${versions.stellar_cli}`);
}

// --- Container base images (M0.8, D-01): each Dockerfile's FROM is the pinned image, by digest.
for (const [file, key] of [
  ["docker/node.Dockerfile", "debian"],
  ["docker/relayer.Dockerfile", "node"],
  ["docker/web.Dockerfile", "caddy"],
]) {
  const from = [...read(file).matchAll(/^FROM\s+(\S+)/gm)].map((m) => m[1]);
  const want = versions.images?.[key];
  if (!want || !/@sha256:[0-9a-f]{64}$/.test(want)) fail(`versions.json images.${key} must be an image pinned by digest`);
  else if (from.length !== 1 || from[0] !== want) fail(`${file} builds FROM ${from.join(", ") || "nothing"}, versions.json says ${want}`);
}

// The Linux builder for any machine (D-08): the pinned Rust image of the toolchain of record.
if (!new RegExp(`^rust:${versions.rust_toolchain.replace(/\./g, "\\.")}-[\\w.-]+@sha256:[0-9a-f]{64}$`).test(versions.images?.rust_builder ?? "")) {
  fail(`versions.json images.rust_builder must be rust:${versions.rust_toolchain}-<variant>@sha256:<digest>`);
}
if (!/^stellar\/quickstart:[\w.-]+@sha256:[0-9a-f]{64}$/.test(versions.images?.stellar_quickstart ?? "")) {
  fail("versions.json images.stellar_quickstart must be stellar/quickstart:<tag>@sha256:<digest>");
}

// --- Machines as code (M0.8, D-04): OpenTofu, its providers and the host's Docker, all pinned.
{
  const tofu = versions.opentofu ?? {};
  const files = (dir) => readdirSync(join(root, dir), { withFileTypes: true }).flatMap((e) =>
    e.isDirectory() ? (e.name.startsWith(".") ? [] : files(join(dir, e.name))) : [join(dir, e.name)]);
  const all = existsSync(join(root, "infra/opentofu")) ? files("infra/opentofu") : [];
  if (!all.length) fail("infra/opentofu is missing");
  for (const f of all.filter((f) => f.endsWith(".tf"))) {
    const text = read(f);
    const req = text.match(/required_version\s*=\s*"([^"]+)"/)?.[1];
    if (req !== undefined && req !== `~> ${tofu.version}`) fail(`${f} requires OpenTofu ${req}, versions.json says ~> ${tofu.version}`);
    for (const m of text.matchAll(/source\s*=\s*"([\w-]+\/[\w-]+)"\s*\n\s*version\s*=\s*"([^"]+)"/g)) {
      if (tofu.providers?.[m[1]] !== m[2]) fail(`${f} pins provider ${m[1]} ${m[2]}, versions.json says ${tofu.providers?.[m[1]]}`);
    }
  }
  const locks = all.filter((f) => f.endsWith(".lock.hcl"));
  if (!locks.length) fail("infra/opentofu: no provider lock file committed");
  for (const f of locks) {
    for (const m of read(f).matchAll(/provider "registry\.opentofu\.org\/([^"]+)" \{\s*version\s*=\s*"([^"]+)"/g)) {
      if (tofu.providers?.[m[1]] !== m[2]) fail(`${f} locks provider ${m[1]} ${m[2]}, versions.json says ${tofu.providers?.[m[1]]}`);
    }
  }
  const ci = read(".github/workflows/ci.yml");
  if (ci.match(/TOFU_VERSION: "([^"]+)"/)?.[1] !== tofu.version) fail(`ci.yml TOFU_VERSION must be ${tofu.version}`);
  if (ci.match(/TOFU_SHA256: "([^"]+)"/)?.[1] !== tofu.sha256_linux_amd64) fail("ci.yml TOFU_SHA256 must match versions.json opentofu.sha256_linux_amd64");
  const host = versions.docker_host ?? {};
  const startup = read("infra/opentofu/modules/caravel-host-gcp/startup.sh");
  for (const [v, key] of [["DOCKER_VERSION", "docker_ce"], ["COMPOSE_VERSION", "compose_plugin"], ["CONTAINERD_VERSION", "containerd"], ["DOCKER_KEY_FPR", "apt_key_fingerprint"]]) {
    const got = startup.match(new RegExp(`^${v}="([^"]+)"`, "m"))?.[1];
    if (!host[key] || got !== host[key]) fail(`startup.sh ${v}=${got}, versions.json docker_host.${key} says ${host[key]}`);
  }
}

// --- npm: exact pins in package.json and package-lock.json ------------------
// npm apps: the platform relayer, the perps feed module and web app (M0.5 layout), and the docs site (H-12).
const apps = ["platform/relayer", "lanes/perps/relayer-feeds", "lanes/perps/web", "docs-site"].filter((d) => existsSync(join(root, d, "package.json")));
for (const app of apps) {
  const pkgPath = join(app, "package.json");
  if (!existsSync(join(root, pkgPath))) continue;
  const pkg = JSON.parse(read(pkgPath));
  const lockPath = join(app, "package-lock.json");
  const pkgLock = existsSync(join(root, lockPath)) ? JSON.parse(read(lockPath)) : null;
  if (!pkgLock) fail(`${lockPath} is missing`);
  const deps = { ...pkg.dependencies, ...pkg.devDependencies };
  for (const [name, want] of Object.entries(versions.npm)) {
    if (!(name in deps)) continue;
    if (deps[name] !== want) fail(`${pkgPath} has ${name} "${deps[name]}", versions.json pins "${want}" (exact, no range)`);
    const resolved = pkgLock?.packages?.[`node_modules/${name}`]?.version;
    if (pkgLock && resolved !== want) fail(`${lockPath} resolves ${name} to ${resolved}, versions.json pins ${want}`);
  }
}

// --- Spec version and placeholders (spec §7) --------------------------------
const spec = read("docs/CARAVEL_SPEC.md");
const specVersion = spec.match(/^Spec version: ([0-9.]+)/m)?.[1];
if (specVersion !== versions.spec_version) fail(`spec says version ${specVersion}, versions.json says ${versions.spec_version}`);

const status = new Map();
for (const m of spec.matchAll(/^\| (T-\d{3}) \|.*\| (todo|doing|review|done) \|\s*$/gm)) status.set(m[1], m[2]);
if (!status.has("T-000")) fail("could not read task statuses from spec §20.1");

function walk(value, path, visit) {
  if (typeof value === "string") visit(value, path);
  else if (value && typeof value === "object") for (const [k, v] of Object.entries(value)) walk(v, `${path}.${k}`, visit);
}
walk(versions, "versions", (value, path) => {
  if (value.startsWith("RESOLVE_IN_T-000") && status.get("T-000") === "done") fail(`${path} is still ${value} but T-000 is done`);
  const filled = value.match(/^FILLED_BY_(T-\d{3})$/);
  if (filled && status.get(filled[1]) === "done") fail(`${path} is still ${value} but ${filled[1]} is done`);
  if (value.startsWith("RESOLVE_IN_") && !value.startsWith("RESOLVE_IN_T-000")) fail(`${path}: unknown placeholder ${value}`);
});

// --- Testnet only (spec §0.3 rule 9) -----------------------------------------
if (versions.network !== "testnet") fail(`versions.json network is "${versions.network}"; M0 is testnet only`);
if (versions.testnet?.network_passphrase !== "Test SDF Network ; September 2015") fail("versions.json testnet passphrase is not the testnet passphrase");

if (errors.length) {
  for (const e of errors) console.error(`check-versions: ${e}`);
  process.exit(1);
}
console.log(`check-versions: ok (${Object.keys(versions.crates).length} crates, ${Object.keys(versions.npm).length} npm pins, ${status.size} tasks read)`);
