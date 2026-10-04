#!/usr/bin/env bash
# The whole lane on a local Stellar network (spec §19.5), driven by the
# `caravel` CLI alone (M0.6, C-14): a lane from `caravel init`, the e2e's
# [env.e2e] deployment (scripts/e2e/env.toml) included, its vars in a
# --var-file. Outside the cleanup trap the script calls only `caravel` and
# `jq` (scripts/check-e2e.sh keeps it so: no stellar, curl, node -e, kill or
# sleep).
#
#   ./scripts/e2e-local.sh                        # KEEP=1 leaves everything running
#   E2E_TEMPLATE=payments ./scripts/e2e-local.sh  # the payments template instead of perps
#   E2E_NETWORK=testnet ./scripts/e2e-local.sh    # a throwaway lane on Stellar testnet
#   E2E_TOKEN_CODE=EURC ./scripts/e2e-local.sh    # a local lane settling in another token
#   E2E_RUNTIME=docker ./scripts/e2e-local.sh     # the nodes as containers (D-03): on Linux the
#                                                 # release and its images are built here; elsewhere
#                                                 # E2E_RELEASE_DIR=<a Linux release with IMAGES>
#
# With E2E_NETWORK=testnet the lane gets a settlement contract of its own (the
# demo lane's is never touched) and Circle's testnet USDC, bought with
# friendbot XLM on the testnet DEX. E2E_WASM_DIR=<CI contracts-wasm artifact>
# deploys the Wasm of record (DEC-033); without it (or E2E_RELEASE_DIR), the
# settlement_wasm var pins this machine's settlement build.
#
#   0. `caravel init`, then `caravel apply`: identities, the token, the
#      settlement contract at its derived address, node configs, 3 validators,
#      the sequencer, the relayer; `caravel plan --exit-code` then exits 0;
#   1. `caravel account create` and `caravel deposit` 1,000 for A and B;
#   2. perps: A rests a bid, B sells into it (`caravel tx`); the fill shows in
#      the API; payments: A sends B 100, and the fee reaches the treasury;
#   3. `caravel wait checkpoint`: a checkpoint is accepted on Stellar;
#   4. `caravel withdraw`: A withdraws 100 and claims it on Stellar;
#   4b. validator 3 is replaced by validator 4 (--var 'validators=["1","2","4"]'),
#       and `caravel apply` rotates the signers; a checkpoint signed by the
#       old set but not yet submitted is signed again by the new set;
#   4c. `caravel force-withdraw`: B's forced withdrawal on Stellar, processed
#       by the lane and claimed;
#   5. `caravel destroy`: drain, stop, export every exit to exit.json, freeze;
#      `caravel escape`: A and B escape pro rata;
#   6. `caravel replay` from Stellar data reports OK.
set -euo pipefail
START_TIME=$(date +%s)

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
WORK="${WORK:-$(mktemp -d)}"
mkdir -p "$WORK/logs"
# The Stellar CLI keeps its keys and settings here, not in your own config.
export XDG_CONFIG_HOME="$WORK/xdg"
E2E_NETWORK="${E2E_NETWORK:-local}"
case "$E2E_NETWORK" in
  # The settlement token: a test asset <E2E_TOKEN_CODE>:<admin> (USDC unless set).
  local) TOKEN="{ local = \"${E2E_TOKEN_CODE:-USDC}\" }"; FUND=(--amount 2000) ;;
  # Circle's testnet USDC, bought on the testnet DEX with friendbot XLM.
  testnet) TOKEN='"circle-usdc"'; FUND=(--amount 1100 --max-xlm 1200) ;;
  *) echo "E2E_NETWORK is local or testnet" >&2; exit 1 ;;
esac
E2E_RUNTIME="${E2E_RUNTIME:-process}"
case "$E2E_RUNTIME" in process | docker) ;; *) echo "E2E_RUNTIME is process or docker" >&2; exit 1 ;; esac
E2E_TEMPLATE="${E2E_TEMPLATE:-perps}"
case "$E2E_TEMPLATE" in
  perps) BALANCE=collateral ;;
  payments) BALANCE=balance ;;
  *) echo "E2E_TEMPLATE is perps or payments" >&2; exit 1 ;;
esac
CARAVEL="$ROOT/target/release/caravel"
LANE="$WORK/lane.toml"
SEQ_PORT=18080
SEQ="http://127.0.0.1:$SEQ_PORT"
RELEASE=()
[[ -n "${E2E_RELEASE_DIR:-}" ]] && RELEASE=(--release-dir "$E2E_RELEASE_DIR")
[[ -n "${E2E_WASM_DIR:-}" ]] && RELEASE+=(--wasm-dir "$E2E_WASM_DIR")

log() { printf '\n== %s\n' "$*"; }
fail() { echo "FAIL: $*" >&2; exit 1; }

cleanup() {
  local code=$?
  if [[ "${KEEP:-0}" != "1" ]]; then
    for f in "$WORK"/.caravel/*/*/run/*.pid; do [[ -f "$f" ]] && kill "$(cat "$f")" 2>/dev/null || true; done
    for f in "$WORK"/.caravel/*/*/config/compose.yml; do [[ -f "$f" ]] && docker compose -f "$f" down -t 2 > /dev/null 2>&1 || true; done
    [[ "$E2E_NETWORK" == local ]] && stellar container stop local > /dev/null 2>&1 || true
  fi
  if (( code != 0 )); then
    echo "logs and state: $WORK" >&2
    for f in "$WORK"/.caravel/*/*/logs/*.log; do [[ -f "$f" ]] && { echo "--- $(basename "$f")" >&2; tail -15 "$f" >&2; }; done
    for f in "$WORK"/.caravel/*/*/config/compose.yml; do [[ -f "$f" ]] && docker compose -f "$f" logs --tail 15 >&2 2>&1 || true; done
  fi
}
trap cleanup EXIT

# caravel CMD ...: on the e2e deployment, with its vars (VARS: the var file,
# and later the rotation). It runs from $WORK, so its state (.caravel/) stays
# there; progress goes to a log ($ERR, else logs/caravel.log).
VARS=(--var-file "$WORK/e2e.vars.toml")
caravel() { local cmd="$1"; shift; (cd "$WORK" && "$CARAVEL" "$cmd" -f "$LANE" --env e2e ${RELEASE[@]+"${RELEASE[@]}"} "${VARS[@]}" "$@") 2>> "${ERR:-$WORK/logs/caravel.log}"; }
# JSON from a command (stdout). --json goes first: after a `tx` body it
# would be part of the body.
q() { local cmd="$1"; shift; caravel "$cmd" --json "$@"; }
# A token amount ("12.5") in base units (7 decimals), for exact arithmetic.
units() { local w="${1%%.*}" f=""; [[ "$1" == *.* ]] && f="${1#*.}"; f="${f}0000000"; echo $(( 10#$w * 10000000 + 10#${f:0:7} )); }
# A view the API serves updates right after its block: wait for it.
until_api() { caravel wait --timeout 30 api "$@" > /dev/null; }

log "tools"
if [[ "$E2E_NETWORK" == local ]]; then
  command -v docker > /dev/null || fail "docker is required"
  docker info > /dev/null 2>&1 || fail "the Docker daemon is not running"
fi
command -v jq > /dev/null || fail "jq is required"

log "build"
./scripts/build-contracts.sh
cargo build --release --locked -p "caravel-$E2E_TEMPLATE-node" -p caravel-cli
npm --prefix platform/relayer ci --silent
npm --prefix platform/relayer run build --silent
if [[ "$E2E_TEMPLATE" == perps ]]; then
  npm --prefix lanes/perps/relayer-feeds ci --silent
  npm --prefix lanes/perps/relayer-feeds run build --silent
fi

if [[ "$E2E_RUNTIME" == docker && -z "${E2E_RELEASE_DIR:-}" ]]; then
  log "release and images"
  [[ "$(uname -s)" == Linux ]] || fail "E2E_RUNTIME=docker builds Linux images from this checkout on Linux only; elsewhere give E2E_RELEASE_DIR=<a Linux release with IMAGES>"
  npm --prefix lanes/perps/web ci --silent
  npm --prefix lanes/perps/web run build --silent
  ./scripts/assemble-release.sh "$WORK/release" --templates "$E2E_TEMPLATE" > "$WORK/logs/assemble.log"
  ./scripts/build-images.sh "$WORK/release" > "$WORK/logs/images.log" 2>&1 || { tail -20 "$WORK/logs/images.log"; fail "build-images"; }
  RELEASE=(--release-dir "$WORK/release")
  [[ -n "${E2E_WASM_DIR:-}" ]] && RELEASE+=(--wasm-dir "$E2E_WASM_DIR")
fi

log "lane file"
# `caravel init`: the template's scaffold with e2e-<role> identities. The
# e2e's deployment (scripts/e2e/env.toml, [env.e2e]) is included, and its vars
# go in a file.
(cd "$WORK" && "$CARAVEL" init "$E2E_TEMPLATE" scaffold --name "e2e-$E2E_TEMPLATE" --prefix e2e --port "$SEQ_PORT") > "$WORK/logs/init.log" 2>&1 \
  || { cat "$WORK/logs/init.log"; fail "caravel init"; }
{ echo 'include = ["e2e-env.toml"]'; cat "$WORK/scaffold/lane.toml"; } > "$LANE"
cp "$ROOT/scripts/e2e/env.toml" "$WORK/e2e-env.toml"
{
  echo "network = \"$E2E_NETWORK\""
  echo "token = $TOKEN"
  echo "port = $SEQ_PORT"
  echo "runtime = \"$E2E_RUNTIME\""
  if [[ "$E2E_NETWORK" == testnet && -z "${E2E_RELEASE_DIR:-}${E2E_WASM_DIR:-}" ]]; then
    echo "settlement_wasm = \"$(shasum -a 256 target/contracts/settlement.wasm | awk '{print $1}')\""
  fi
  if [[ "$E2E_TEMPLATE" == perps ]]; then
    echo 'feed_keys = { CARAVEL_ORACLE_SECRET = "e2e-oracle" }'
    echo 'feeds = [{ module = "relayer-feeds/perps/dist/index.js", intervalMs = 2000, options = { maxSourceAgeSecs = 900, markets = { "1" = [{ fixed = "65000" }], "2" = [{ fixed = "3500" }], "3" = [{ fixed = "0.40" }] } } }]'
  fi
} > "$WORK/e2e.vars.toml"
caravel validate > /dev/null || fail "caravel validate"
caravel render > "$WORK/logs/render.toml" || fail "caravel render"
caravel doctor > "$WORK/logs/doctor.log" || { cat "$WORK/logs/doctor.log"; fail "caravel doctor"; }

log "0. caravel apply"
caravel keys ensure > /dev/null
ERR="$WORK/logs/apply.log" caravel apply --yes >> "$WORK/logs/apply.log" || { tail -30 "$WORK/logs/apply.log"; fail "caravel apply"; }
grep -E '^\+|^~|^-|→|Applied' "$WORK/logs/apply.log" | sed 's/^/   /'
caravel plan --exit-code > /dev/null || fail "a second plan still has changes"
# The deployment's outputs (built in, and its own [outputs] in scripts/e2e/env.toml).
SETTLEMENT="$(caravel output settlement)"
[[ "$(caravel output sequencer)" == "$SEQ" ]] || fail "the sequencer output"
[[ "$(q status | jq -r .outputs.settlement_contract)" == "$SETTLEMENT" ]] || fail "status --json outputs"
echo "settlement $SETTLEMENT, token $(caravel output token)"
q api /v1/status | jq -e ".template == \"$E2E_TEMPLATE\"" > /dev/null || fail "the sequencer runs another template"
if [[ "$E2E_TEMPLATE" == perps ]]; then
  caravel wait --timeout 90 api /v1/markets /0/oracle_price=65000000 > /dev/null || fail "oracle prices"
fi

log "1. accounts and deposits"
for n in alice bob; do
  caravel account create "$n" "${FUND[@]}" > /dev/null || fail "caravel account create $n"
done
A="$(caravel keys show alice)"; B="$(caravel keys show bob)"
for n in alice bob; do
  q deposit "$n" 1000 | jq -e '.credited' > /dev/null || fail "$n's deposit"
  q balance "$n" | jq -e ".lane.$BALANCE == \"10000000000\"" > /dev/null || fail "$n's lane balance"
done

if [[ "$E2E_TEMPLATE" == perps ]]; then
  log "2. A rests a bid, B sells into it"
  # Prices per lot in token units: 1 lot is 0.0001 BTC, so 6.5 is $65,000 per BTC.
  q tx --from alice place-order --market 1 --side buy --tif gtc --price 6.5 --lots 10 | jq -e '.code == 0' > /dev/null || fail "A's bid"
  until_api '/v1/markets/1/book?depth=1' /bids/0/lots=10 || fail "A's bid on the book"
  q tx --from bob place-order --market 1 --side sell --tif ioc --price 6.4 --lots 4 | jq -e '.code == 0' > /dev/null || fail "B's sell"
  until_api "/v1/accounts/$A" /positions/0/lots=4 || fail "A's position"
  until_api "/v1/accounts/$B" /positions/0/lots=-4 || fail "B's position"
  q api '/v1/markets/1/trades?limit=1' | jq -e ".[0].lots == 4 and .[0].price == \"65000000\" and .[0].taker == \"$B\"" > /dev/null || fail "the trade"
else
  log "2. A sends B 100; the fee goes to the treasury"
  FEE="$(awk -F'[= ]+' '/^transfer_fee/ {print $2}' "$LANE")"
  q tx --from alice transfer --to @bob --amount 100 --memo 7 | jq -e '.code == 0' > /dev/null || fail "the transfer"
  until_api "/v1/accounts/$B" /balance=11000000000 || fail "B's balance after the transfer"
  q api "/v1/accounts/$A" | jq -e ".balance == \"$(( 9000000000 - FEE ))\"" > /dev/null || fail "A's balance after the transfer"
  q api "/v1/accounts/$(caravel keys show e2e-treasury)" | jq -e ".balance == \"$FEE\" and .system" > /dev/null || fail "the treasury's fee"
fi

log "3. a checkpoint accepted on Stellar"
caravel wait --timeout 90 checkpoint > /dev/null || fail "an accepted checkpoint"
[[ "$(q status | jq -r .last_checkpoint.seq)" -ge 1 ]] || fail "the last checkpoint on Stellar"

log "4. A withdraws 100 and claims it"
before="$(q balance alice | jq -r .stellar)"
q withdraw alice 100 --timeout 180 | jq -e '.claimed == true and .leaf.amount == "100"' > /dev/null || fail "A's withdrawal"
after="$(q balance alice | jq -r .stellar)"
(( $(units "$after") - $(units "$before") == $(units 100) )) || fail "A's token went from $before to $after"
echo "A's balance on Stellar +100"

log "4b. validator 3 replaced by validator 4 (a --var); caravel apply rotates"
# Stop the relayer until a checkpoint is signed by the old set but not
# submitted: after the rotation, the sequencer must have it signed again by the
# new set (older epochs are invalid at once).
caravel stop relayer --yes > /dev/null
caravel wait --timeout 90 checkpoint --signed > /dev/null || fail "a checkpoint signed but not submitted"
stale="$(q api /v1/status | jq -r .checkpoints.signed)"
VARS+=(--var 'validators=["1","2","4"]')
caravel keys ensure > /dev/null
caravel plan | grep -E '^\+|^~|^-' | sed 's/^/   /'
ERR="$WORK/logs/apply-rotate.log" caravel apply --yes >> "$WORK/logs/apply-rotate.log" || { tail -30 "$WORK/logs/apply-rotate.log"; fail "caravel apply (rotation)"; }
[[ "$(q status | jq -r .epoch)" == 2 ]] || fail "the contract epoch after the rotation"
# Captured first: grep -q stops reading early, and a streamed log (docker) would then fail the pipe.
caravel logs sequencer -n 100000 > "$WORK/logs/sequencer-after-rotation.log"
grep -q "older epoch go back for signatures" "$WORK/logs/sequencer-after-rotation.log" || fail "the sequencer did not send checkpoint $stale back for signatures"
caravel wait --timeout 120 checkpoint --seq "$stale" --epoch 2 > /dev/null || fail "checkpoint $stale accepted under epoch 2"
caravel wait --timeout 120 checkpoint --seq $(( stale + 1 )) --epoch 2 > /dev/null || fail "checkpoint $(( stale + 1 )) accepted under epoch 2"
caravel plan --exit-code > /dev/null || fail "a plan after the rotation still has changes"
echo "epoch 2: checkpoints $stale and $(( stale + 1 )) accepted, signed by validators 1, 2 and 4"

log "4c. B asks for a forced withdrawal of 50 on Stellar and claims it"
before="$(q balance bob | jq -r .stellar)"
q force-withdraw bob 50 --timeout 180 | jq -e '.claimed == true and .leaf.amount == "50"' > /dev/null || fail "B's forced withdrawal"
after="$(q balance bob | jq -r .stellar)"
(( $(units "$after") - $(units "$before") == $(units 50) )) || fail "B's token went from $before to $after"
echo "B's balance on Stellar +50 by forced withdrawal"

log "5. caravel destroy; A and B escape"
ERR="$WORK/logs/destroy.log" caravel destroy --yes >> "$WORK/logs/destroy.log" || { tail -30 "$WORK/logs/destroy.log"; fail "caravel destroy"; }
grep -E '→|Frozen|validators' "$WORK/logs/destroy.log" | sed 's/^/   /'
q status | jq -e '.frozen' > /dev/null || fail "the lane is not frozen"
EXIT="$(ls "$WORK"/.caravel/*/e2e/exit.json)"
last_seq="$(q status | jq -r .last_checkpoint.seq)"
[[ "$(jq -r .seq "$EXIT")" == "$last_seq" && "$(jq -r .checked_on_chain "$EXIT")" == true ]] || fail "exit.json is not Stellar's last checkpoint $last_seq"
vault="$(q balance "$SETTLEMENT" | jq -r .stellar)"
total=0
for n in alice bob; do
  r="$(q escape "$n")" || fail "$n's escape"
  jq -e '.escape == "claimed" and .escape_paid == .expected' <<< "$r" > /dev/null || fail "$n's escape: $r"
  echo "$n escaped $(jq -r .escape_paid <<< "$r") (equity $(jq -r .equity <<< "$r"))"
  total=$(( total + $(units "$(jq -r .escape_paid <<< "$r")") ))
done
(( total <= $(units "$vault") )) || fail "escape payouts $total exceed the vault $vault"
q escape alice | jq -e '.escape == "already claimed"' > /dev/null || fail "a second escape"

log "6. replay from Stellar data only"
q replay --prove-escape alice > "$WORK/replay.json" || fail "replay: $(cat "$WORK/replay.json")"
jq -e '.report.ok == true' "$WORK/replay.json" > /dev/null || fail "replay: $(cat "$WORK/replay.json")"
[[ "$(jq -r .escape.equity "$WORK/replay.json")" == "$(jq -r --arg g "$A" '.escape[] | select(.account == $g) | .equity' "$EXIT")" ]] || fail "replay's escape proof differs from exit.json"
jq -c .report "$WORK/replay.json"

echo
echo "E2E OK ($E2E_TEMPLATE) in $(( $(date +%s) - START_TIME )) s: settlement $SETTLEMENT, $(jq -r .report.checkpoints "$WORK/replay.json") checkpoints replayed, work dir $WORK"
