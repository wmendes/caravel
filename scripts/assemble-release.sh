#!/usr/bin/env bash
# Assembles a Caravel release from this checkout's builds (M0.6, DEC-076):
# what `caravel apply` installs on a host, and what `scripts/install.sh`
# puts next to the `caravel` binary. CI's release artifact is made by this
# script too.
#
#   scripts/assemble-release.sh <out dir> [--commit <sha>] [--templates "perps payments"]
#
# Layout:
#   bin/caravel, bin/caravel-<template>-node
#   contracts/*.wasm
#   relayer/{dist,package.json,package-lock.json,node_modules}   (production deps only)
#   relayer-feeds/<template>/…                                    (templates that have feeds)
#   web/<template>/…                                              (templates whose web app is built)
#   COMMIT, SHA256SUMS
#
# Build first: ./scripts/build-contracts.sh, cargo build --release for
# caravel-cli and each template's node, npm ci + npm run build in
# platform/relayer (and in each template's relayer-feeds and web).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT=""
COMMIT=""
TEMPLATES=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --commit) COMMIT="$2"; shift 2 ;;
    --templates) TEMPLATES="$2"; shift 2 ;;
    -h|--help) sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) echo "assemble-release: unknown option $1" >&2; exit 2 ;;
    *) OUT="$1"; shift ;;
  esac
done
[[ -n "$OUT" ]] || { echo "usage: scripts/assemble-release.sh <out dir> [--commit <sha>] [--templates \"perps payments\"]" >&2; exit 2; }
if [[ -z "$TEMPLATES" ]]; then
  TEMPLATES="$(cd "$ROOT/lanes" && for d in */; do [[ -d "$d/node" ]] && printf '%s ' "${d%/}"; done)"
fi

sha256() { if command -v sha256sum > /dev/null; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }
need() { [[ -e "$1" ]] || { echo "assemble-release: no $1 (build first: $2)" >&2; exit 1; }; }

need "$ROOT/target/release/caravel" "cargo build --release -p caravel-cli"
need "$ROOT/platform/relayer/dist/main.js" "npm --prefix platform/relayer ci && npm --prefix platform/relayer run build"
ls "$ROOT"/target/contracts/*.wasm > /dev/null 2>&1 || need "$ROOT/target/contracts/settlement.wasm" "./scripts/build-contracts.sh"

rm -rf "$OUT"
mkdir -p "$OUT/bin" "$OUT/contracts" "$OUT/relayer"
cp "$ROOT/target/release/caravel" "$OUT/bin/"
for t in $TEMPLATES; do
  need "$ROOT/target/release/caravel-$t-node" "cargo build --release -p caravel-$t-node"
  cp "$ROOT/target/release/caravel-$t-node" "$OUT/bin/"
  if [[ -d "$ROOT/lanes/$t/relayer-feeds" ]]; then
    need "$ROOT/lanes/$t/relayer-feeds/dist" "npm --prefix lanes/$t/relayer-feeds ci && npm --prefix lanes/$t/relayer-feeds run build"
    mkdir -p "$OUT/relayer-feeds/$t"
    cp -r "$ROOT/lanes/$t/relayer-feeds/dist" "$ROOT/lanes/$t/relayer-feeds/package.json" "$ROOT/lanes/$t/relayer-feeds/package-lock.json" "$OUT/relayer-feeds/$t/"
  fi
  if [[ -f "$ROOT/lanes/$t/web/dist/index.html" ]]; then
    mkdir -p "$OUT/web/$t"
    cp -r "$ROOT/lanes/$t/web/dist/." "$OUT/web/$t/"
  fi
done
cp "$ROOT"/target/contracts/*.wasm "$OUT/contracts/"
cp -r "$ROOT/platform/relayer/dist" "$ROOT/platform/relayer/package.json" "$ROOT/platform/relayer/package-lock.json" "$OUT/relayer/"
# Production dependencies ship in the release (pure JS, checked by the
# package-lock integrity here), so hosts never run npm.
npm --prefix "$OUT/relayer" ci --omit=dev --no-audit --no-fund --loglevel=error
for d in "$OUT"/relayer-feeds/*/; do
  [[ -d "$d" ]] && npm --prefix "$d" ci --omit=dev --no-audit --no-fund --loglevel=error
done

(cd "$OUT" && find bin contracts relayer relayer-feeds -type f -not -path '*/node_modules/*' 2> /dev/null | LC_ALL=C sort | xargs sha256 > SHA256SUMS)
if [[ -z "$COMMIT" ]]; then
  # A build from a clean tree is its commit; anything else is named by
  # what it holds.
  if git -C "$ROOT" diff --quiet HEAD 2> /dev/null && [[ -z "$(git -C "$ROOT" status --porcelain 2> /dev/null)" ]]; then
    COMMIT="$(git -C "$ROOT" rev-parse HEAD)"
  else
    COMMIT="local-$(sha256 "$OUT/SHA256SUMS" | cut -c1-8)"
  fi
fi
echo "$COMMIT" > "$OUT/COMMIT"
echo "assembled $OUT ($COMMIT; templates: $TEMPLATES)"
