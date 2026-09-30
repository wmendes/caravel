#!/usr/bin/env bash
# Builds both contracts to Wasm with the pinned Stellar CLI, checks size limits
# (spec §3.3, §12.2) and prints each sha256. The engine hash printed here is the
# only engine hash of record (DEC-020): nodes, replay and Stellar load this file.
#
# The recorded hashes are x86_64 Linux builds (DEC-033): cargo mixes the host
# triple into every crate's -C metadata through proc-macro and build-script
# dependencies, and that can change function order in the Wasm. On x86_64 Linux
# (CI) a hash mismatch is an error; on other hosts it is a warning.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

OUT_DIR="target/contracts"
ENGINE_BUDGET=120000   # §12.2: budget for the engine
PAYMENTS_BUDGET=65536  # §20.4.4: budget for the payments engine
WASM_LIMIT=131072      # §3.3: max deployable contract Wasm

want_cli="$(node -p 'require("./versions.json").stellar_cli')"
have_cli="$(stellar --version | head -1 | awk '{print $2}')"
if [[ "$have_cli" != "$want_cli" ]]; then
  echo "error: stellar CLI $have_cli, versions.json pins $want_cli" >&2
  exit 1
fi

rm -rf "$OUT_DIR"
# Only the contracts of record (engine-profile is a profiling tool). The perps
# engine builds from its frozen workspace (DEC-051), the settlement contract
# and the payments engine (P-10) from the root workspace.
stellar contract build --locked --manifest-path lanes/perps/engine/Cargo.toml --package perps-engine --out-dir "$OUT_DIR" --quiet
stellar contract build --locked --package settlement --out-dir "$OUT_DIR" --quiet
stellar contract build --locked --package payments-engine --out-dir "$OUT_DIR" --quiet

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
# The recorded hash (a versions.json path), or empty while it still has a placeholder.
recorded() { node -p "const v = '$1'.split('.').reduce((o, k) => o[k], require('./versions.json')); v.startsWith('FILLED_BY_') ? '' : v"; }
canonical_host=0
[[ "$(uname -s)" == "Linux" && "$(uname -m)" == "x86_64" ]] && canonical_host=1
status=0
for name in perps_engine settlement payments_engine; do
  wasm="$OUT_DIR/$name.wasm"
  if [[ ! -f "$wasm" ]]; then
    echo "error: $wasm was not built" >&2
    exit 1
  fi
  size=$(wc -c < "$wasm" | tr -d ' ')
  limit=$WASM_LIMIT
  [[ "$name" == "perps_engine" ]] && limit=$ENGINE_BUDGET
  [[ "$name" == "payments_engine" ]] && limit=$PAYMENTS_BUDGET
  hash="$(sha256 "$wasm")"
  printf '%-14s %7d bytes (limit %d)  sha256 %s\n' "$name" "$size" "$limit" "$hash"
  if (( size > limit )); then
    echo "error: $name is over its size limit" >&2
    status=1
  fi
  # INV-D7: the Wasm of record must reproduce exactly.
  case "$name" in
    perps_engine) key="artifacts.engine_wasm_sha256" ;;
    settlement) key="artifacts.settlement_wasm_sha256" ;;
    payments_engine) key="lanes.payments.engine_wasm_sha256" ;;
  esac
  want="$(recorded "$key")"
  if [[ -n "$want" && "$want" != "$hash" ]]; then
    if (( canonical_host )); then
      echo "error: $name hash $hash differs from versions.json $key $want" >&2
      echo "       (if the contract changed on purpose, update versions.json in the same change)" >&2
      status=1
    else
      echo "warning: $name hash differs from versions.json $key $want;" >&2
      echo "         recorded hashes are x86_64 Linux builds (DEC-033), and CI checks them" >&2
    fi
  fi
done
exit $status
