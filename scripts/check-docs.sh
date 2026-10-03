#!/usr/bin/env bash
# The docs site's copy rules (M0.7, H-12): honest claims only (spec §2), the
# well-known IaC tool never named, and no em dashes.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT/docs-site"
fail=0
# check WHAT PATTERN [ALLOWED]: lines matching PATTERN, unless they also match ALLOWED.
check() {
  local what="$1" pattern="$2" allowed="${3:-^$}"
  if grep -rniE "$pattern" docs src docusaurus.config.ts 2>/dev/null | grep -viE "$allowed" > /tmp/check-docs.$$; then
    echo "check-docs: $what" >&2; sed 's/^/  /' /tmp/check-docs.$$ >&2; fail=1
  fi
}
check "never claim trustless" '\btrustless\b'
check "nothing is audited" '\baudited\b' "not audited|not been audited|hasn't been audited"
check "testnet only: no mainnet claims" '\bmainnet\b' "mainnet is refused|refuses mainnet"
check "not production" 'production[- ]ready|production[- ]grade'
check "never name the well-known IaC tool" 'terra[f]orm'
check "no em dashes" '—'
rm -f /tmp/check-docs.$$
[[ "$fail" == 0 ]] && echo "check-docs: ok"
exit "$fail"
