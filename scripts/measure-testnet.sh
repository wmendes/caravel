#!/usr/bin/env bash
# T-014 (spec §19.6): load and cost numbers from the live testnet lane.
#
#   ./scripts/measure-testnet.sh          # TPS=20 DURATION=600 ACCOUNTS=12
#
# 1. Throwaway load accounts: friendbot XLM, testnet USDC bought with XLM on
#    the testnet DEX, and a real deposit each through the settlement contract.
#    Keys stay in $WORK (under target/, never in git) and are reused on reruns.
# 2. The load generator drives the public lane API (the same HTTPS endpoint
#    the web app uses) and measures latency from the WebSocket stream.
# 3. The relayer's checkpoint log comes from the VM over IAP ssh, and
#    scripts/measure-report.mjs prints the §19.6 table.
#
# Testnet only: the network, contract IDs and USDC asset are the testnet ones
# in versions.json, and the load generator refuses a sequencer running
# another lane.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
TPS="${TPS:-20}"
DURATION="${DURATION:-600}"
ACCOUNTS="${ACCOUNTS:-12}"
DEPOSIT_USDC="${DEPOSIT_USDC:-500}"
SWAP_XLM="${SWAP_XLM:-600}"
API="${API:-https://35-224-76-64.sslip.io}"
PROJECT="${PROJECT:-caravel-testnet}"
ZONE="${ZONE:-us-central1-a}"
VM="${VM:-caravel-1}"
WORK="${WORK:-$ROOT/target/measure-testnet}"
LANE="$ROOT/config/lane.caravel-perps.testnet.toml"
USDC=10000000
mkdir -p "$WORK/keys/load" "$WORK/logs"
chmod 700 "$WORK/keys" "$WORK/keys/load"
# The Stellar CLI keeps these throwaway keys here, not in your own keystore.
export XDG_CONFIG_HOME="$WORK/xdg"
NET=(--network testnet)
RPC="$(node -p 'require("./versions.json").testnet.rpc_url')"
SETTLEMENT="$(node -p 'require("./versions.json").testnet.settlement_contract')"
[[ "$(node -p 'require("./versions.json").testnet.network_passphrase')" == "Test SDF Network ; September 2015" ]] || { echo "not testnet" >&2; exit 1; }
USDC_ASSET="USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
SDK="$ROOT/apps/relayer/node_modules/@stellar/stellar-sdk"

log() { printf '\n== %s\n' "$*"; }
fail() { echo "FAIL: $*" >&2; exit 1; }
sc() { stellar "$@" 2>> "$WORK/logs/stellar-cli.log"; }
pk() { sc keys public-key "$1"; }
raw() { node -e 'const {StrKey}=require(process.argv[1]);console.log(Buffer.from(StrKey.decodeEd25519PublicKey(process.argv[2])).toString("hex"))' "$SDK" "$1"; }
exists() { node -e 'const {rpc}=require(process.argv[1]);new rpc.Server(process.argv[2]).getAccount(process.argv[3]).then(()=>process.exit(0),()=>process.exit(1))' "$SDK" "$RPC" "$1"; }
usdc_balance() { curl -sf "https://horizon-testnet.stellar.org/accounts/$1" | jq -r --arg i "${USDC_ASSET#USDC:}" '[.balances[] | select(.asset_code == "USDC" and .asset_issuer == $i) | .balance][0] // "none"'; }
collateral() { curl -sf "$API/v1/accounts/$1" | jq -r '.collateral // "0"' 2>/dev/null || echo 0; }
until_ok() {
  local what="$1" deadline=$(( $(date +%s) + ${TIMEOUT:-120} )); shift
  until "$@" > /dev/null 2>&1; do
    (( $(date +%s) < deadline )) || fail "timed out waiting for $what"
    sleep 2
  done
}

log "tools and lane"
command -v jq > /dev/null || fail "jq is required"
[[ -d "$SDK" ]] || npm --prefix apps/relayer ci --silent
cargo build --release --locked -p caravel-node --example loadgen
status="$(curl -sf "$API/v1/status")" || fail "no lane at $API"
jq -c '{lane_name, height, executor, checkpoints}' <<< "$status"
# Request time after the TLS handshake (the load generator reuses connections).
rtt="$(for _ in 1 2 3 4 5; do curl -s -o /dev/null -w '%{time_appconnect} %{time_starttransfer}\n' "$API/v1/status"; done | awk '{print $2 - $1}' | sort -n | sed -n 3p)"
echo "client → API request time after TLS (median of 5 GET /v1/status): $(node -p "Math.round($rtt * 1000)") ms"

log "load accounts ($ACCOUNTS)"
for i in $(seq 1 "$ACCOUNTS"); do
  n="load-$i"
  if ! sc keys public-key "$n" > /dev/null 2>&1; then
    sc keys generate "$n" > /dev/null
  fi
  g="$(pk "$n")"
  exists "$g" || { sc keys fund "$n" "${NET[@]}" > /dev/null; TIMEOUT=60 until_ok "friendbot for $n" exists "$g"; }
  if [[ "$(collateral "$g")" -lt $(( DEPOSIT_USDC * USDC / 2 )) ]]; then
    [[ "$(usdc_balance "$g")" == "none" ]] && sc tx new change-trust --source-account "$n" --line "$USDC_ASSET" "${NET[@]}" > /dev/null
    have="$(usdc_balance "$g")"
    if node -e "process.exit(Number('$have') >= $DEPOSIT_USDC ? 1 : 0)"; then
      sc tx new path-payment-strict-send --source-account "$n" --send-asset native --send-amount $(( SWAP_XLM * USDC )) \
        --destination "$g" --dest-asset "$USDC_ASSET" --dest-min $(( DEPOSIT_USDC * USDC )) "${NET[@]}" > /dev/null \
        || fail "the testnet DEX did not give $DEPOSIT_USDC USDC for $SWAP_XLM XLM (see $WORK/logs/stellar-cli.log)"
    fi
    sc contract invoke --id "$SETTLEMENT" --source-account "$n" "${NET[@]}" -- \
      deposit --from "$g" --amount $(( DEPOSIT_USDC * USDC )) --lane_account "$(raw "$g")" > /dev/null
  fi
  sc keys secret "$n" > "$WORK/keys/load/$n.key"
  echo "$n $g USDC on Stellar $(usdc_balance "$g")"
done
chmod 600 "$WORK"/keys/load/*.key
for i in $(seq 1 "$ACCOUNTS"); do
  g="$(pk "load-$i")"
  TIMEOUT=180 until_ok "lane credit for load-$i" sh -c "[ \"\$(curl -sf $API/v1/accounts/$g | jq -r .collateral)\" -ge $(( DEPOSIT_USDC * USDC / 2 )) ]"
done
echo "all $ACCOUNTS accounts credited in the lane"

log "load: $TPS tx/s for $DURATION s"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
RUN="$WORK/run-$STAMP"
mkdir -p "$RUN"
echo "$rtt" > "$RUN/rtt_s"
"$ROOT/target/release/examples/loadgen" --url "$API" --lane "$LANE" --key-dir "$WORK/keys/load" \
  --no-fund --no-oracle --measure --tps "$TPS" --duration-secs "$DURATION" --report-secs 60 | tee "$RUN/loadgen.jsonl"

log "relayer checkpoint log from the VM"
gcloud compute ssh "$VM" --zone "$ZONE" --project "$PROJECT" --tunnel-through-iap --quiet \
  --command "sudo cat /opt/caravel/data/relayer-checkpoints.jsonl" > "$RUN/relayer-checkpoints.jsonl"
wc -l < "$RUN/relayer-checkpoints.jsonl"

log "report"
node scripts/measure-report.mjs --api "$API" --loadgen "$RUN/loadgen.jsonl" --metrics "$RUN/relayer-checkpoints.jsonl" \
  --tps "$TPS" --label "testnet, $TPS tx/s for $DURATION s, $ACCOUNTS accounts, $STAMP" | tee "$RUN/report.txt"
echo "run files in $RUN"
