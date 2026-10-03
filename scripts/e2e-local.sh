#!/usr/bin/env bash
# The whole lane on a local Stellar network (spec §19.5), driven by the deploy
# tool (spec §20.3, P-14): the template's lane file, including the e2e's
# [env.e2e] deployment (scripts/e2e/env.toml) with its vars in a --var-file.
#
#   ./scripts/e2e-local.sh                        # KEEP=1 leaves everything running
#   E2E_TEMPLATE=payments ./scripts/e2e-local.sh  # the payments template instead of perps
#   E2E_NETWORK=testnet ./scripts/e2e-local.sh    # a throwaway lane on Stellar testnet
#   E2E_TOKEN_CODE=EURC ./scripts/e2e-local.sh    # a local lane settling in another token
#
# With E2E_NETWORK=testnet the lane gets a settlement contract of its own (the
# demo lane's is never touched) and Circle's testnet USDC, bought with
# friendbot XLM on the testnet DEX. E2E_WASM_DIR=<CI contracts-wasm artifact>
# deploys the Wasm of record (DEC-033); without it (or E2E_RELEASE_DIR), the
# settlement_wasm var pins this machine's settlement build.
#
#   0. `caravel apply`: accounts, USDC, the settlement contract at its derived
#      address, node configs, 3 validators, the sequencer, the relayer;
#      a second `caravel plan` shows no changes;
#   1. deposit 1,000 USDC for A and B;
#   2. perps: A rests a bid, B sells into it; the fill shows in the API;
#      payments: A sends B 100 USDC, and the fee reaches the treasury;
#   3. a checkpoint is accepted on Stellar;
#   4. A withdraws 100 USDC and claims it on Stellar;
#   4b. validator 3 is replaced by validator 4 (--var 'validators=["1","2","4"]'),
#       and `caravel apply` rotates the signers; a checkpoint signed by the
#       old set but not yet submitted is signed again by the new set;
#   4c. B asks for a forced withdrawal on Stellar, the lane processes it, B claims it;
#   5. `caravel destroy`: drain, stop, export every exit to exit.json, freeze;
#      A and B escape pro rata from exit.json;
#   6. `<app>-node replay` from Stellar data reports OK.
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
  local)
    # The settlement token: a test asset <E2E_TOKEN_CODE>:<admin> (USDC unless set).
    CODE="${E2E_TOKEN_CODE:-USDC}"
    NET=(--network local); RPC="http://localhost:8000/rpc"; PASS="Standalone Network ; February 2017"; TOKEN="{ local = \"$CODE\" }" ;;
  testnet)
    NET=(--network testnet); RPC="$(node -p 'require("./versions.json").testnet.rpc_url')"; PASS="Test SDF Network ; September 2015"; TOKEN='"circle-usdc"'
    USDC_ASSET="USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5" ;;
  *) echo "E2E_NETWORK is local or testnet" >&2; exit 1 ;;
esac
E2E_TEMPLATE="${E2E_TEMPLATE:-perps}"
case "$E2E_TEMPLATE" in
  perps)    LANE_SRC="$ROOT/lanes/perps/config/lane.caravel-perps.local.toml"; BALANCE=collateral ;;
  payments) LANE_SRC="$ROOT/lanes/payments/config/lane.caravel-payments.local.toml"; BALANCE=balance ;;
  *) echo "E2E_TEMPLATE is perps or payments" >&2; exit 1 ;;
esac
NODE="caravel-$E2E_TEMPLATE-node"
BIN="$ROOT/target/release/$NODE"
CARAVEL="$ROOT/target/release/caravel"
LANE="$WORK/lane.toml"
SEQ_PORT=18080
SEQ="http://127.0.0.1:$SEQ_PORT"
V1="http://127.0.0.1:$((SEQ_PORT + 1))"
USDC=10000000
RELEASE=()
[[ -n "${E2E_RELEASE_DIR:-}" ]] && RELEASE=(--release-dir "$E2E_RELEASE_DIR")
[[ -n "${E2E_WASM_DIR:-}" ]] && RELEASE+=(--wasm-dir "$E2E_WASM_DIR")

log() { printf '\n== %s\n' "$*"; }
fail() { echo "FAIL: $*" >&2; exit 1; }

cleanup() {
  local code=$?
  if [[ "${KEEP:-0}" != "1" ]]; then
    for f in "$WORK"/.caravel/*/*/run/*.pid; do [[ -f "$f" ]] && kill "$(cat "$f")" 2>/dev/null || true; done
    [[ "$E2E_NETWORK" == local ]] && stellar container stop local > /dev/null 2>&1 || true
  fi
  if (( code != 0 )); then
    echo "logs and state: $WORK" >&2
    for f in "$WORK"/.caravel/*/*/logs/*.log; do [[ -f "$f" ]] && { echo "--- $(basename "$f")" >&2; tail -15 "$f" >&2; }; done
  fi
}
trap cleanup EXIT

# until CMD... : retry every second for up to $TIMEOUT seconds (default 90).
until_ok() {
  local what="$1"; shift
  local deadline=$(( $(date +%s) + ${TIMEOUT:-90} ))
  until "$@" > /dev/null 2>&1; do
    (( $(date +%s) < deadline )) || fail "timed out waiting for $what"
    sleep 1
  done
}

# CLI progress output goes to a log; failures print its tail.
sc() { stellar "$@" 2>> "$WORK/logs/stellar-cli.log"; }
pk() { sc keys public-key "$1"; }
raw() { node -e 'const {StrKey}=require(process.argv[1]);console.log(Buffer.from(StrKey.decodeEd25519PublicKey(process.argv[2])).toString("hex"))' "$ROOT/platform/relayer/node_modules/@stellar/stellar-sdk" "$1"; }
invoke() { local id="$1" src="$2"; shift 2; sc contract invoke --id "$id" --source-account "$src" "${NET[@]}" -- "$@"; }
view() { local id="$1"; shift; sc contract invoke --id "$id" --source-account admin "${NET[@]}" --send=no -- "$@"; }
num() { tr -d '"'; }
lane_get() { curl -sf "$SEQ$1"; }
# The deploy tool runs from $WORK, so its state (.caravel/) stays there.
# VARS: the deployment's inputs (the var file, and later the rotation).
VARS=(--var-file "$WORK/e2e.vars.toml")
caravel() { local cmd="$1"; shift; (cd "$WORK" && "$CARAVEL" "$cmd" "$LANE" --env e2e ${RELEASE[@]+"${RELEASE[@]}"} "${VARS[@]}" "$@"); }
status() { caravel status --json 2>> "$WORK/logs/caravel.log"; }

log "tools"
if [[ "$E2E_NETWORK" == local ]]; then
  command -v docker > /dev/null || fail "docker is required"
  docker info > /dev/null 2>&1 || fail "the Docker daemon is not running"
fi
want_cli="$(node -p 'require("./versions.json").stellar_cli')"
[[ "$(stellar --version | head -1 | awk '{print $2}')" == "$want_cli" ]] || fail "stellar CLI $want_cli is required"
command -v jq > /dev/null || fail "jq is required"

log "build"
./scripts/build-contracts.sh
cargo build --release --locked -p "$NODE" -p caravel-cli
npm --prefix platform/relayer ci --silent
npm --prefix platform/relayer run build --silent
if [[ "$E2E_TEMPLATE" == perps ]]; then
  npm --prefix lanes/perps/relayer-feeds ci --silent
  npm --prefix lanes/perps/relayer-feeds run build --silent
fi

log "identities"
for n in admin relayer alice bob validator-1 validator-2 validator-3 validator-4; do
  sc keys generate --overwrite "$n" > /dev/null
done
# The local perps lane's oracle key is the public fixture key (seed [0x31; 32], DEC-037).
node -e 'const {Keypair}=require(process.argv[1]);console.log(Keypair.fromRawEd25519Seed(Buffer.alloc(32,0x31)).secret())' "$ROOT/platform/relayer/node_modules/@stellar/stellar-sdk" \
  | sc keys add oracle --secret-key > /dev/null
A="$(pk alice)"; B="$(pk bob)"

log "lane file"
# The template's lane file with the e2e's deployment included
# (scripts/e2e/env.toml, [env.e2e]), and its vars in a file: no heredoc.
{ echo 'include = ["e2e-env.toml"]'; cat "$LANE_SRC"; } > "$LANE"
cp "$ROOT/scripts/e2e/env.toml" "$WORK/e2e-env.toml"
{
  echo "network = \"$E2E_NETWORK\""
  echo "token = $TOKEN"
  echo "port = $SEQ_PORT"
  if [[ "$E2E_NETWORK" == testnet && -z "${E2E_RELEASE_DIR:-}${E2E_WASM_DIR:-}" ]]; then
    echo "settlement_wasm = \"$(shasum -a 256 target/contracts/settlement.wasm | awk '{print $1}')\""
  fi
  if [[ "$E2E_TEMPLATE" == perps ]]; then
    echo 'feed_keys = { CARAVEL_ORACLE_SECRET = "oracle" }'
    echo 'feeds = [{ module = "relayer-feeds/perps/dist/index.js", intervalMs = 2000, options = { maxSourceAgeSecs = 900, markets = { "1" = [{ fixed = "65000" }], "2" = [{ fixed = "3500" }], "3" = [{ fixed = "0.40" }] } } }]'
  fi
} > "$WORK/e2e.vars.toml"
caravel render > "$WORK/logs/render.toml" 2>> "$WORK/logs/caravel.log" || fail "caravel render"

log "0. caravel apply"
caravel apply --yes > "$WORK/logs/apply.log" 2>&1 || { tail -30 "$WORK/logs/apply.log"; fail "caravel apply"; }
grep -E '^\+|^~|^-|→|Applied' "$WORK/logs/apply.log" | sed 's/^/   /'
caravel plan 2>/dev/null | grep -q "No changes." || fail "a second plan still has changes"
st="$(status)"
SETTLEMENT="$(jq -r .settlement <<< "$st")"; USDC_ID="$(jq -r .token <<< "$st")"
echo "settlement $SETTLEMENT, USDC $USDC_ID"
lane_get /v1/status | jq -e ".template == \"$E2E_TEMPLATE\"" > /dev/null || fail "the sequencer runs another template"
if [[ "$E2E_TEMPLATE" == perps ]]; then
  until_ok "oracle prices" sh -c "curl -sf $SEQ/v1/markets | jq -e '.[0].oracle_price == \"65000000\"'"
fi

log "USDC for A and B"
exists() { node -e 'const {rpc}=require(process.argv[1]);new rpc.Server(process.argv[2],{allowHttp:true}).getAccount(process.argv[3]).then(()=>process.exit(0),()=>process.exit(1))' "$ROOT/platform/relayer/node_modules/@stellar/stellar-sdk" "$RPC" "$1"; }
for n in alice bob; do
  TIMEOUT=180 until_ok "funding $n" sh -c "stellar keys fund $n ${NET[*]} && true"
  until_ok "$n on the network" exists "$(pk "$n")"
done
if [[ "$E2E_NETWORK" == local ]]; then
  for n in alice bob; do
    sc tx new change-trust --source-account "$n" --line "$CODE:$(pk admin)" "${NET[@]}" > /dev/null
    invoke "$USDC_ID" admin mint --to "$(pk "$n")" --amount $(( 2000 * USDC )) > /dev/null
  done
else
  for n in alice bob; do
    sc tx new change-trust --source-account "$n" --line "$USDC_ASSET" "${NET[@]}" > /dev/null
    sc tx new path-payment-strict-send --source-account "$n" --send-asset native --send-amount $(( 1200 * USDC )) \
      --destination "$(pk "$n")" --dest-asset "$USDC_ASSET" --dest-min $(( 1100 * USDC )) "${NET[@]}" > /dev/null \
      || fail "the testnet DEX did not give 1,100 USDC for 1,200 XLM"
  done
fi

log "1. deposits"
for n in alice bob; do
  invoke "$SETTLEMENT" "$n" deposit --from "$(pk "$n")" --amount $(( 1000 * USDC )) --lane_account "$(raw "$(pk "$n")")" > /dev/null
done
for g in "$A" "$B"; do
  until_ok "lane credit for $g" sh -c "curl -sf $SEQ/v1/accounts/$g | jq -e '.$BALANCE == \"$(( 1000 * USDC ))\"'"
done

tx() { local who="$1"; shift; sc keys secret "$who" > "$WORK/$who.key"; chmod 600 "$WORK/$who.key"; "$BIN" tx --lane "$LANE" --key-file "$WORK/$who.key" --sequencer "$SEQ" "$@"; }
if [[ "$E2E_TEMPLATE" == perps ]]; then
  log "2. A rests a bid, B sells into it"
  tx alice place-order --market 1 --side buy --tif gtc --price 65000000 --lots 10 > /dev/null
  until_ok "A's bid on the book" sh -c "curl -sf '$SEQ/v1/markets/1/book?depth=1' | jq -e '.bids[0].lots == 10'"
  tx bob place-order --market 1 --side sell --tif ioc --price 64000000 --lots 4 > /dev/null
  until_ok "the fill" sh -c "curl -sf $SEQ/v1/accounts/$A | jq -e '.positions[0].lots == 4'"
  lane_get "/v1/accounts/$B" | jq -e '.positions[0].lots == -4' > /dev/null || fail "B's position"
  lane_get "/v1/markets/1/trades?limit=1" | jq -e ".[0].lots == 4 and .[0].price == \"65000000\" and .[0].taker == \"$B\"" > /dev/null || fail "the trade"
else
  log "2. A sends B 100 USDC; the fee goes to the treasury"
  FEE="$(awk -F'[= ]+' '/^transfer_fee/ {print $2}' "$LANE")"
  TREASURY="$(awk -F'"' '/^treasury_key/ {print $2}' "$LANE")"
  tx alice transfer --to "$B" --amount $(( 100 * USDC )) --memo 7 > /dev/null
  until_ok "the transfer" sh -c "curl -sf $SEQ/v1/accounts/$B | jq -e '.balance == \"$(( 1100 * USDC ))\"'"
  lane_get "/v1/accounts/$A" | jq -e ".balance == \"$(( 900 * USDC - FEE ))\"" > /dev/null || fail "A's balance after the transfer"
  lane_get "/v1/accounts/$TREASURY" | jq -e ".balance == \"$FEE\" and .system" > /dev/null || fail "the treasury's fee"
fi

log "3. a checkpoint accepted on Stellar"
until_ok "an accepted checkpoint" sh -c "curl -sf $SEQ/v1/status | jq -e '.checkpoints.accepted != null'"
[[ "$(view "$SETTLEMENT" last_checkpoint | jq -r .seq)" -ge 1 ]] || fail "last_checkpoint on Stellar"

log "4. A withdraws 100 USDC and claims it"
before="$(view "$USDC_ID" balance --id "$A" | num)"
tx alice withdraw --amount $(( 100 * USDC )) > /dev/null
TIMEOUT=120 until_ok "A's withdrawal proof" sh -c "curl -sf '$SEQ/v1/proofs/withdrawals?account=$A' | jq -e '.withdrawals | length == 1'"
leaf="$(lane_get "/v1/proofs/withdrawals?account=$A" | jq -c '.withdrawals[0]')"
invoke "$SETTLEMENT" alice claim_withdrawal --recipient "$A" --lane_account "$(raw "$A")" \
  --seq "$(jq -r .seq <<< "$leaf")" --index "$(jq -r .index <<< "$leaf")" --amount "$(jq -r .amount <<< "$leaf")" --proof "$(jq -c .proof <<< "$leaf")" > /dev/null
after="$(view "$USDC_ID" balance --id "$A" | num)"
(( after - before == 100 * USDC )) || fail "A's USDC went from $before to $after"
echo "A's USDC balance +$(( (after - before) / USDC )) USDC"

log "4b. validator 3 replaced by validator 4 (a --var); caravel apply rotates"
# Hold the relayer until a checkpoint is signed by the old set but not
# submitted: after the rotation, the sequencer must have it signed again by the
# new set (older epochs are invalid at once).
RUN="$WORK/.caravel/$(awk -F'"' '/^name/ {print $2; exit}' "$LANE")/e2e"
RELAYER_PID="$(cat "$RUN/run/relayer.pid")"
kill -INT "$RELAYER_PID"
# It finishes the step it is in first, which can be a submission: wait until it exits.
until_ok "the relayer to stop" sh -c "! kill -0 $RELAYER_PID"
until_ok "a checkpoint signed but not submitted" sh -c "curl -sf $SEQ/v1/status | jq -e '(.checkpoints.signed // \"0\" | tonumber) > (.checkpoints.accepted // \"0\" | tonumber)'"
stale="$(lane_get /v1/status | jq -r .checkpoints.signed)"
VARS+=(--var 'validators=["1","2","4"]')
caravel plan 2>/dev/null | grep -E '^\+|^~|^-' | sed 's/^/   /'
caravel apply --yes > "$WORK/logs/apply-rotate.log" 2>&1 || { tail -30 "$WORK/logs/apply-rotate.log"; fail "caravel apply (rotation)"; }
[[ "$(status | jq -r .epoch)" == 2 ]] || fail "the contract epoch after the rotation"
grep -q "older epoch go back for signatures" "$RUN/logs/sequencer.log" || fail "the sequencer did not send checkpoint $stale back for signatures"
TIMEOUT=120 until_ok "checkpoint $stale accepted under epoch 2" sh -c "curl -sf $SEQ/v1/checkpoints/$stale | jq -e '.status == \"accepted\" and .epoch == \"2\"'"
next=$(( stale + 1 ))
TIMEOUT=120 until_ok "checkpoint $next accepted under epoch 2" sh -c "curl -sf $SEQ/v1/checkpoints/$next | jq -e '.status == \"accepted\" and .epoch == \"2\"'"
caravel plan 2>/dev/null | grep -q "No changes." || fail "a plan after the rotation still has changes"
echo "epoch 2: checkpoints $stale and $next accepted, signed by validators 1, 2 and 4"

log "4c. B asks for a forced withdrawal of 50 USDC on Stellar and claims it"
before="$(view "$USDC_ID" balance --id "$B" | num)"
invoke "$SETTLEMENT" bob request_forced_withdrawal --owner "$B" --lane_account "$(raw "$B")" --amount $(( 50 * USDC )) > /dev/null
TIMEOUT=120 until_ok "B's forced withdrawal proof" sh -c "curl -sf '$SEQ/v1/proofs/withdrawals?account=$B' | jq -e '.withdrawals | length == 1'"
leaf="$(lane_get "/v1/proofs/withdrawals?account=$B" | jq -c '.withdrawals[0]')"
[[ "$(jq -r .amount <<< "$leaf")" == "$(( 50 * USDC ))" ]] || fail "B's forced withdrawal leaf: $leaf"
invoke "$SETTLEMENT" bob claim_withdrawal --recipient "$B" --lane_account "$(raw "$B")" \
  --seq "$(jq -r .seq <<< "$leaf")" --index "$(jq -r .index <<< "$leaf")" --amount "$(jq -r .amount <<< "$leaf")" --proof "$(jq -c .proof <<< "$leaf")" > /dev/null
after="$(view "$USDC_ID" balance --id "$B" | num)"
(( after - before == 50 * USDC )) || fail "B's USDC went from $before to $after"
echo "B's USDC balance +$(( (after - before) / USDC )) USDC by forced withdrawal"

log "5. caravel destroy; A and B escape from exit.json"
caravel destroy --yes > "$WORK/logs/destroy.log" 2>&1 || { tail -30 "$WORK/logs/destroy.log"; fail "caravel destroy"; }
grep -E '→|Frozen|validators' "$WORK/logs/destroy.log" | sed 's/^/   /'
[[ "$(status | jq -r .frozen)" == true ]] || fail "the lane is not frozen"
EXIT="$RUN/exit.json"
last_seq="$(view "$SETTLEMENT" last_checkpoint | jq -r .seq)"
[[ "$(jq -r .seq "$EXIT")" == "$last_seq" && "$(jq -r .checked_on_chain "$EXIT")" == true ]] || fail "exit.json is not Stellar's last checkpoint $last_seq"
info="$(view "$SETTLEMENT" frozen_info)"
vault="$(view "$USDC_ID" balance --id "$SETTLEMENT" | num)"
num_="$(jq -r .payout_num <<< "$info")"; den="$(jq -r .payout_den <<< "$info")"
echo "frozen: payout $num_ / $den, vault $vault"
total=0
for n in alice bob; do
  g="$(pk "$n")"
  p="$(jq -c --arg g "$g" '.escape[] | select(.account == $g)' "$EXIT")"
  [[ -n "$p" ]] || fail "exit.json has no escape leaf for $n"
  b0="$(view "$USDC_ID" balance --id "$g" | num)"
  invoke "$SETTLEMENT" "$n" escape_claim --recipient "$g" --lane_account "$(raw "$g")" --index "$(jq -r .index <<< "$p")" \
    --equity "$(jq -r .equity <<< "$p")" --proof "$(jq -c .proof <<< "$p")" > /dev/null
  b1="$(view "$USDC_ID" balance --id "$g" | num)"
  paid=$(( b1 - b0 ))
  want="$(node -p "(BigInt('$(jq -r .equity <<< "$p")') * BigInt('$num_') / BigInt('$den')).toString()")"
  [[ "$paid" == "$want" ]] || fail "$n was paid $paid, expected equity × ratio = $want"
  echo "$n escaped $paid stroops"
  total=$(( total + paid ))
done
(( total <= vault )) || fail "escape payouts $total exceed the vault $vault"

log "6. replay from Stellar data only"
"$BIN" replay --rpc "$RPC" --network-passphrase "$PASS" --settlement "$SETTLEMENT" --genesis-config "$LANE" \
  --engine-wasm "target/contracts/${E2E_TEMPLATE}_engine.wasm" --prove-escape "$A" > "$WORK/replay.json"
jq -s -e '.[0].ok == true' "$WORK/replay.json" > /dev/null || fail "replay: $(cat "$WORK/replay.json")"
[[ "$(jq -s -r '.[1].equity' "$WORK/replay.json")" == "$(jq -r --arg g "$A" '.escape[] | select(.account == $g) | .equity' "$EXIT")" ]] || fail "replay's escape proof differs from exit.json"
jq -s -c '.[0]' "$WORK/replay.json"

echo
echo "E2E OK ($E2E_TEMPLATE) in $(( $(date +%s) - START_TIME )) s: settlement $SETTLEMENT, $(jq -s -r '.[0].checkpoints' "$WORK/replay.json") checkpoints replayed, work dir $WORK"
