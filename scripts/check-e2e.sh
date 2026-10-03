#!/usr/bin/env bash
# The e2e drives a lane with the `caravel` CLI alone (M0.6, C-14): outside its
# cleanup trap, scripts/e2e-local.sh may not call the Stellar CLI, curl, node
# -e/-p, kill or sleep. Comments don't count.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
FILE=scripts/e2e-local.sh
hits="$(awk '
  /^cleanup\(\) \{/ { skip = 1 }
  skip && /^\}/ { skip = 0; next }
  skip { next }
  { line = $0; sub(/^[[:space:]]*#.*/, "", line); sub(/[[:space:]]+#[^"'"'"']*$/, "", line) }
  line ~ /(^|[^-[:alnum:]_\/.])(stellar|curl|kill|sleep)([^-[:alnum:]_.]|$)/ || line ~ /node -[ep]/ { printf "%s:%d: %s\n", FILENAME, NR, $0 }
' "$FILE")"
if [[ -n "$hits" ]]; then
  echo "check-e2e: $FILE calls a tool outside caravel (use a caravel command):" >&2
  echo "$hits" >&2
  exit 1
fi
echo "check-e2e: ok ($FILE uses caravel and jq only)"
