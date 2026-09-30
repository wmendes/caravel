#!/usr/bin/env bash
# DEC-051: lanes/perps/engine/ is the Caravel Perps engine of record, frozen.
# The deployed lane's settlement contract stores its Wasm hash (4571cd25…), so
# not one byte of its sources, manifests or lock may change. This check pins
# the directory's git tree hash in versions.json (lanes.perps.engine_tree) and
# refuses local edits.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
DIR="lanes/perps/engine"
want="$(node -p 'require("./versions.json").lanes.perps.engine_tree')"
have="$(git rev-parse "HEAD:$DIR")"
status=0
if [[ "$have" != "$want" ]]; then
  echo "error: $DIR tree is $have, versions.json lanes.perps.engine_tree pins $want" >&2
  echo "       the frozen perps engine changed; that needs a new lane (DEC-051)" >&2
  status=1
fi
if [[ -n "$(git status --porcelain -- "$DIR")" ]]; then
  echo "error: $DIR has local changes:" >&2
  git status --porcelain -- "$DIR" >&2
  status=1
fi
(( status == 0 )) && echo "check-frozen: ok ($DIR tree $have)"
exit $status
