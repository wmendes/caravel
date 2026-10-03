#!/usr/bin/env bash
# The docs site's CLI reference matches the binary (M0.7, H-12): regenerate
# it from `caravel help` and fail on any difference.
#
#   CARAVEL_BIN=target/debug/caravel ./scripts/check-cli-docs.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${CARAVEL_BIN:-$ROOT/target/release/caravel}"
[[ -x "$BIN" ]] || { echo "check-cli-docs: no caravel at $BIN (cargo build -p caravel-cli)" >&2; exit 1; }
node "$ROOT/scripts/gen-cli-docs.mjs" "$BIN" > /dev/null
if ! git -C "$ROOT" diff --quiet -- docs-site/docs/reference/cli || [[ -n "$(git -C "$ROOT" ls-files --others --exclude-standard docs-site/docs/reference/cli)" ]]; then
  git -C "$ROOT" diff --stat -- docs-site/docs/reference/cli >&2
  echo "check-cli-docs: the CLI reference is out of date: node scripts/gen-cli-docs.mjs, then commit" >&2
  exit 1
fi
echo "check-cli-docs: ok"
