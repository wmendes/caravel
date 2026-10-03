#!/usr/bin/env bash
# Runs the README's quickstart as written (M0.6, C-14, Gate G3): installs
# Caravel from this checkout into a scratch prefix, then runs the shell block
# that follows the `<!-- quickstart -->` marker in README.md, line by line, in
# a scratch directory with a scratch Stellar CLI keystore. Needs Docker, the
# Stellar CLI 28.1.0 and Node.js 22, like the quickstart itself.
#
#   ./scripts/check-quickstart.sh            # SKIP_BUILD=1 reuses this checkout's builds
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="${WORK:-$(mktemp -d)}"
# CARAVEL_YES answers apply's prompt, which a person at a terminal answers.
export CARAVEL_HOME="$WORK/caravel" XDG_CONFIG_HOME="$WORK/xdg" CARAVEL_YES=1
export PATH="$CARAVEL_HOME/bin:$PATH"

cleanup() {
  local code=$?
  # destroy leaves the validators serving proofs: stop what is still running.
  [[ -f "$WORK/run/my-lane/lane.toml" ]] && (cd "$WORK/run/my-lane" && caravel stop --yes > /dev/null 2>&1 || true)
  (( code == 0 )) || echo "quickstart failed; work dir $WORK" >&2
}
trap cleanup EXIT

block="$(awk '/<!-- quickstart/ { on = 1; next } on && /^```sh/ { inside = 1; next } inside && /^```/ { exit } inside' "$ROOT/README.md")"
[[ -n "$block" ]] || { echo "README.md has no quickstart block" >&2; exit 1; }

echo "== install"
args=(--templates payments)
[[ "${SKIP_BUILD:-0}" == 1 ]] && args+=(--skip-build)
"$ROOT/scripts/install.sh" "${args[@]}" > "$WORK/install.log" 2>&1 || { tail -30 "$WORK/install.log"; exit 1; }

echo "== the quickstart, as written"
mkdir -p "$WORK/run" && cd "$WORK/run"
printf '%s\n' "$block"
bash -euo pipefail -c "$block" > "$WORK/quickstart.log" 2>&1 || { tail -40 "$WORK/quickstart.log"; exit 1; }
tail -5 "$WORK/quickstart.log"
echo "QUICKSTART OK in $WORK"
