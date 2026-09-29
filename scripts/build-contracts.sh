#!/usr/bin/env bash
# Builds both contracts to Wasm with the pinned Stellar CLI, checks size limits
# (spec §3.3, §12.2) and prints each sha256. The engine hash printed here is the
# only engine hash of record (DEC-020): nodes, replay and Stellar load this file.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

OUT_DIR="target/contracts"
ENGINE_BUDGET=120000   # §12.2: budget for the engine
WASM_LIMIT=131072      # §3.3: max deployable contract Wasm

want_cli="$(node -p 'require("./versions.json").stellar_cli')"
have_cli="$(stellar --version | head -1 | awk '{print $2}')"
if [[ "$have_cli" != "$want_cli" ]]; then
  echo "error: stellar CLI $have_cli, versions.json pins $want_cli" >&2
  exit 1
fi

rm -rf "$OUT_DIR"
stellar contract build --locked --out-dir "$OUT_DIR" --quiet

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
# The recorded hash, or empty while versions.json still has a placeholder.
recorded() { node -p "const v = require('./versions.json').artifacts['$1']; v.startsWith('FILLED_BY_') ? '' : v"; }
status=0
for name in perps_engine settlement; do
  wasm="$OUT_DIR/$name.wasm"
  if [[ ! -f "$wasm" ]]; then
    echo "error: $wasm was not built" >&2
    exit 1
  fi
  size=$(wc -c < "$wasm" | tr -d ' ')
  limit=$WASM_LIMIT
  [[ "$name" == "perps_engine" ]] && limit=$ENGINE_BUDGET
  hash="$(sha256 "$wasm")"
  printf '%-14s %7d bytes (limit %d)  sha256 %s\n' "$name" "$size" "$limit" "$hash"
  if (( size > limit )); then
    echo "error: $name is over its size limit" >&2
    status=1
  fi
  # INV-D7: the Wasm of record must reproduce exactly.
  key="${name/perps_engine/engine}_wasm_sha256"
  want="$(recorded "$key")"
  if [[ -n "$want" && "$want" != "$hash" ]]; then
    echo "error: $name hash $hash differs from versions.json $key $want" >&2
    echo "       (if the contract changed on purpose, update versions.json in the same change)" >&2
    status=1
  fi
done
exit $status
