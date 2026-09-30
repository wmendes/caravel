#!/usr/bin/env bash
# T-011 (spec §19.5): the whole lane on a local Stellar network, one command.
#
#   ./scripts/e2e-local.sh                        # KEEP=1 leaves everything running
#   E2E_NETWORK=testnet SETTLEMENT_WASM=<CI settlement.wasm> ./scripts/e2e-local.sh    # on Stellar testnet
#
# With E2E_NETWORK=testnet the same lane runs against Stellar testnet with a
# throwaway settlement contract of its own (the freeze drill of spec §24; the
# demo lane's contract is never touched) and Circle's testnet USDC, bought
# with friendbot XLM on the testnet DEX.
#
# A local quickstart (Docker, via `stellar container start`) with a local
# USDC asset contract; the settlement contract with a 30 s escape timeout
# (quickstart cannot move ledger time, DEC-043); the sequencer, 3 validators
# and the relayer as local processes. Then §19.5 steps 1-6:
#   1. deposit 1,000 USDC for A and B;
#   2. A rests a bid, B sells into it; the fill shows in the API;
#   3. a checkpoint is accepted on Stellar;
#   4. A withdraws 100 USDC and claims it on Stellar;
#   4b. validator 3 is rotated to a new key (the RUNBOOK's admin rotation);
#   5. the sequencer stops, anyone freezes, A and B escape pro rata;
#   6. `caravel-node replay` from Stellar data reports OK.
set -euo pipefail
START_TIME=$(date +%s)

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
WORK="${WORK:-$(mktemp -d)}"
mkdir -p "$WORK/keys" "$WORK/logs"
chmod 700 "$WORK/keys"
# The Stellar CLI keeps its keys and settings here, not in your own config.
export XDG_CONFIG_HOME="$WORK/xdg"
E2E_NETWORK="${E2E_NETWORK:-local}"
case "$E2E_NETWORK" in
  local)
    NET=(--network local); RPC="http://localhost:8000/rpc"; PASS="Standalone Network ; February 2017" ;;
  testnet)
    NET=(--network testnet); RPC="$(node -p 'require("./versions.json").testnet.rpc_url')"; PASS="Test SDF Network ; September 2015"
    USDC_ASSET="USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5" ;;
  *) echo "E2E_NETWORK is local or testnet" >&2; exit 1 ;;
esac
LANE="$ROOT/config/lane.caravel-perps.local.toml"
BIN="$ROOT/target/release/caravel-node"
SEQ_PORT=18080
SEQ="http://127.0.0.1:$SEQ_PORT"
USDC=10000000
PIDS=()
STARTED_CONTAINER=0

log() { printf '\n== %s\n' "$*"; }
fail() { echo "FAIL: $*" >&2; exit 1; }

cleanup() {
  local code=$?
  if [[ "${KEEP:-0}" != "1" ]]; then
    for p in "${PIDS[@]:-}"; do [[ -n "$p" ]] && kill "$p" 2>/dev/null || true; done
    if (( STARTED_CONTAINER )); then stellar container stop caravel-e2e > /dev/null 2>&1 || true; fi
  fi
  if (( code != 0 )); then
    echo "logs and state: $WORK" >&2
    for f in "$WORK"/logs/*.log; do [[ -f "$f" ]] && { echo "--- $(basename "$f")" >&2; tail -15 "$f" >&2; }; done
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
raw() { node -e 'const {StrKey}=require(process.argv[1]);console.log(Buffer.from(StrKey.decodeEd25519PublicKey(process.argv[2])).toString("hex"))' "$ROOT/apps/relayer/node_modules/@stellar/stellar-sdk" "$1"; }
invoke() { local id="$1" src="$2"; shift 2; sc contract invoke --id "$id" --source-account "$src" "${NET[@]}" -- "$@"; }
view() { local id="$1"; shift; sc contract invoke --id "$id" --source-account admin "${NET[@]}" --send=no -- "$@"; }
num() { tr -d '"'; }
lane_get() { curl -sf "$SEQ$1"; }

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
cargo build --release --locked -p caravel-node
npm --prefix apps/relayer ci --silent
npm --prefix apps/relayer run build --silent

log "Stellar network ($E2E_NETWORK)"
if [[ "$E2E_NETWORK" == local ]] && ! curl -sf -X POST "$RPC" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' | grep -q healthy; then
  stellar container start local --name caravel-e2e --limits testnet > "$WORK/logs/container.log" 2>&1
  STARTED_CONTAINER=1
  TIMEOUT=180 until_ok "local RPC" sh -c "curl -sf -X POST $RPC -H 'content-type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getHealth\"}' | grep -q healthy"
fi
curl -s -X POST "$RPC" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"getNetwork"}' | jq -c .result

log "accounts"
# Friendbot comes up after RPC on a fresh quickstart, and `keys generate
# --fund` reports a failed funding without failing, so check the account.
exists() { node -e 'const {rpc}=require(process.argv[1]);new rpc.Server(process.argv[2],{allowHttp:true}).getAccount(process.argv[3]).then(()=>process.exit(0),()=>process.exit(1))' "$ROOT/apps/relayer/node_modules/@stellar/stellar-sdk" "$RPC" "$1"; }
fund() { sc keys fund "$1" "${NET[@]}" && exists "$(pk "$1")"; }
for n in admin relayer issuer alice bob; do
  sc keys generate --overwrite "$n" > /dev/null
  TIMEOUT=180 until_ok "funding $n" fund "$n"
  sc keys secret "$n" > "$WORK/keys/$n.key"
done
for i in 1 2 3; do
  sc keys generate --overwrite "validator-$i" > /dev/null
  sc keys secret "validator-$i" > "$WORK/keys/validator-$i.key"
done
# The local lane's oracle key is the public fixture key (seed [0x31; 32], DEC-037).
ORACLE_SECRET="$(node -e 'const {Keypair}=require(process.argv[1]);console.log(Keypair.fromRawEd25519Seed(Buffer.alloc(32,0x31)).secret())' "$ROOT/apps/relayer/node_modules/@stellar/stellar-sdk")"
chmod 600 "$WORK"/keys/*.key
A="$(pk alice)"; B="$(pk bob)"; ISSUER="$(pk issuer)"

log "USDC"
if [[ "$E2E_NETWORK" == local ]]; then
  USDC_ID="$(sc contract asset deploy --asset "USDC:$ISSUER" --source-account issuer "${NET[@]}")"
  for n in alice bob; do
    sc tx new change-trust --source-account "$n" --line "USDC:$ISSUER" "${NET[@]}" > /dev/null
    invoke "$USDC_ID" issuer mint --to "$(pk "$n")" --amount $(( 2000 * USDC )) > /dev/null
  done
else
  USDC_ID="$(node -p 'require("./versions.json").testnet.usdc_sac')"
  for n in alice bob; do
    sc tx new change-trust --source-account "$n" --line "$USDC_ASSET" "${NET[@]}" > /dev/null
    sc tx new path-payment-strict-send --source-account "$n" --send-asset native --send-amount $(( 1200 * USDC )) \
      --destination "$(pk "$n")" --dest-asset "$USDC_ASSET" --dest-min $(( 1100 * USDC )) "${NET[@]}" > /dev/null \
      || fail "the testnet DEX did not give 1,100 USDC for 1,200 XLM"
  done
fi
echo "USDC $USDC_ID"

log "settlement contract"
genesis="$("$BIN" genesis --config "$LANE")"
LANE_ID="$(jq -r .lane_id <<< "$genesis")"
ENGINE_HASH="$(shasum -a 256 target/contracts/perps_engine.wasm | awk '{print $1}')"
[[ "$ENGINE_HASH" == "$(node -p 'require("./versions.json").artifacts.engine_wasm_sha256')" ]] || fail "the engine Wasm is not the one of record"
signers="$(for i in 1 2 3; do raw "$(pk "validator-$i")"; done | sort | jq -R '{key: ., weight: 1}' | jq -sc '{signers: ., threshold: 2}')"
params='{"force_inclusion_window_secs":20,"escape_timeout_secs":30,"min_rotation_delay_secs":3600,"signer_retention_epochs":2,"min_deposit":"10000000"}'
# SETTLEMENT_WASM: e.g. the x86_64 Linux build of record from CI (DEC-033), checked against versions.json.
SETTLEMENT_WASM="${SETTLEMENT_WASM:-target/contracts/settlement.wasm}"
if [[ "$SETTLEMENT_WASM" != target/contracts/settlement.wasm ]]; then
  [[ "$(shasum -a 256 "$SETTLEMENT_WASM" | awk '{print $1}')" == "$(node -p 'require("./versions.json").artifacts.settlement_wasm_sha256')" ]] || fail "$SETTLEMENT_WASM is not the settlement Wasm of record"
fi
SETTLEMENT="$(sc contract deploy --wasm "$SETTLEMENT_WASM" --source-account admin "${NET[@]}" -- \
  --admin "$(pk admin)" --usdc "$USDC_ID" --lane_id "$LANE_ID" --engine_wasm_hash "$ENGINE_HASH" \
  --genesis_state_hash "$(jq -r .genesis_state_hash <<< "$genesis")" --config_hash "$(jq -r .config_hash <<< "$genesis")" \
  --signers "$signers" --params "$params")"
echo "settlement $SETTLEMENT"

log "configs"
common() {
  cat <<EOF
lane = "$LANE"
engine_wasm = "$ROOT/target/contracts/perps_engine.wasm"
engine_wasm_sha256 = "$ENGINE_HASH"
network_passphrase = "$PASS"
settlement_contract = "$SETTLEMENT"
EOF
}
# sequencer_config EPOCH NAME... : validator N of the list listens on SEQ_PORT + N.
sequencer_config() {
  local epoch="$1" n=0; shift
  echo "[sequencer]"; echo "listen = \"127.0.0.1:$SEQ_PORT\""; common; echo "db = \"$WORK/sequencer.sqlite\""
  echo; echo "[signers]"; echo "epoch = $epoch"; echo "threshold = 2"; echo "validators = ["
  for v in "$@"; do n=$((n + 1)); echo "  { url = \"http://127.0.0.1:$((SEQ_PORT + n))\", key = \"$(pk "$v")\" },"; done
  echo "]"
}
# validator_config NAME PORT
validator_config() {
  echo "[validator]"; echo "listen = \"127.0.0.1:$2\""; echo "sequencer_url = \"$SEQ\""
  echo "key_file = \"keys/$1.key\""; common; echo "db = \"$WORK/$1.sqlite\""
  echo "rpc_url = \"$RPC\""; echo "stellar_poll_secs = 2"; echo "poll_ms = 100"
}
sequencer_config 1 validator-1 validator-2 validator-3 > "$WORK/sequencer.toml"
for i in 1 2 3; do validator_config "validator-$i" $((SEQ_PORT + i)) > "$WORK/validator-$i.toml"; done
jq -n --arg rpc "$RPC" --arg pass "$PASS" --arg s "$SETTLEMENT" --arg seq "$SEQ" '{
  rpcUrl: $rpc, networkPassphrase: $pass, settlementContract: $s, sequencerUrl: $seq,
  metricsFile: "relayer-checkpoints.jsonl",
  loops: {inbox: true, checkpoints: true, oracle: true},
  intervalsMs: {inbox: 1000, checkpoints: 1000, oracle: 2000},
  oracle: {maxSourceAgeSecs: 900, markets: {"1": [{fixed: "65000"}], "2": [{fixed: "3500"}], "3": [{fixed: "0.40"}]}}
}' > "$WORK/relayer.json"

log "lane processes"
CARAVEL_INTERNAL_TOKEN="$(openssl rand -hex 16)"
export CARAVEL_INTERNAL_TOKEN
RUST_LOG=info "$BIN" sequencer --config "$WORK/sequencer.toml" > "$WORK/logs/sequencer.log" 2>&1 & PIDS+=($!); SEQ_PID=$!
VAL_PIDS=()
for i in 1 2 3; do
  RUST_LOG=info "$BIN" validator --config "$WORK/validator-$i.toml" > "$WORK/logs/validator-$i.log" 2>&1 & PIDS+=($!); VAL_PIDS+=($!)
done
relayer() {
  CARAVEL_RELAYER_SECRET="$(cat "$WORK/keys/relayer.key")" CARAVEL_ORACLE_SECRET="$ORACLE_SECRET" \
    node apps/relayer/dist/main.js --config "$WORK/relayer.json" >> "$WORK/logs/relayer.log" 2>&1 & PIDS+=($!); RELAYER_PID=$!
}
relayer
until_ok "the sequencer" lane_get /v1/status
until_ok "oracle prices" sh -c "curl -sf $SEQ/v1/markets | jq -e '.[0].oracle_price == \"65000000\"'"

log "1. deposits"
for n in alice bob; do
  invoke "$SETTLEMENT" "$n" deposit --from "$(pk "$n")" --amount $(( 1000 * USDC )) --lane_account "$(raw "$(pk "$n")")" > /dev/null
done
for g in "$A" "$B"; do
  until_ok "lane credit for $g" sh -c "curl -sf $SEQ/v1/accounts/$g | jq -e '.collateral == \"$(( 1000 * USDC ))\"'"
done

log "2. A rests a bid, B sells into it"
tx() { local who="$1"; shift; "$BIN" tx --lane "$LANE" --key-file "$WORK/keys/$who.key" --sequencer "$SEQ" "$@"; }
tx alice place-order --market 1 --side buy --tif gtc --price 65000000 --lots 10 > /dev/null
until_ok "A's bid on the book" sh -c "curl -sf '$SEQ/v1/markets/1/book?depth=1' | jq -e '.bids[0].lots == 10'"
tx bob place-order --market 1 --side sell --tif ioc --price 64000000 --lots 4 > /dev/null
until_ok "the fill" sh -c "curl -sf $SEQ/v1/accounts/$A | jq -e '.positions[0].lots == 4'"
lane_get "/v1/accounts/$B" | jq -e '.positions[0].lots == -4' > /dev/null || fail "B's position"
lane_get "/v1/markets/1/trades?limit=1" | jq -e ".[0].lots == 4 and .[0].price == \"65000000\" and .[0].taker == \"$B\"" > /dev/null || fail "the trade"

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

log "4b. rotate validator 3 to a new key (testnet-only admin rotation, RUNBOOK)"
# Hold the relayer until a checkpoint is signed by the old set but not
# submitted: after the rotation, the restarted sequencer must have it signed
# again by the new set (older epochs are invalid at once).
kill -INT "$RELAYER_PID"; wait "$RELAYER_PID" 2>/dev/null || true
until_ok "a checkpoint signed but not submitted" sh -c "curl -sf $SEQ/v1/status | jq -e '(.checkpoints.signed // \"0\" | tonumber) > (.checkpoints.accepted // \"0\" | tonumber)'"
stale="$(lane_get /v1/status | jq -r .checkpoints.signed)"
sc keys generate --overwrite validator-4 > /dev/null
sc keys secret validator-4 > "$WORK/keys/validator-4.key"; chmod 600 "$WORK/keys/validator-4.key"
new_signers="$(for v in validator-1 validator-2 validator-4; do raw "$(pk "$v")"; done | sort | jq -R '{key: ., weight: 1}' | jq -sc '{signers: ., threshold: 2}')"
invoke "$SETTLEMENT" admin admin_rotate_signers --new "$new_signers" > /dev/null
[[ "$(view "$SETTLEMENT" epoch | num)" == 2 ]] || fail "the contract epoch after the rotation"
kill -INT "$SEQ_PID" "${VAL_PIDS[2]}"; wait "$SEQ_PID" "${VAL_PIDS[2]}" 2>/dev/null || true
# The new validator starts from an empty store on validator 3's port and catches up.
validator_config validator-4 $((SEQ_PORT + 3)) > "$WORK/validator-4.toml"
RUST_LOG=info "$BIN" validator --config "$WORK/validator-4.toml" > "$WORK/logs/validator-4.log" 2>&1 & PIDS+=($!)
sequencer_config 2 validator-1 validator-2 validator-4 > "$WORK/sequencer.toml"
RUST_LOG=info "$BIN" sequencer --config "$WORK/sequencer.toml" >> "$WORK/logs/sequencer.log" 2>&1 & PIDS+=($!); SEQ_PID=$!
until_ok "the sequencer after the rotation" lane_get /v1/status
relayer
grep -q "older epoch go back for signatures" "$WORK/logs/sequencer.log" || fail "the sequencer did not send checkpoint $stale back for signatures"
TIMEOUT=120 until_ok "checkpoint $stale accepted under epoch 2" sh -c "curl -sf $SEQ/v1/checkpoints/$stale | jq -e '.status == \"accepted\" and .epoch == \"2\"'"
next=$(( stale + 1 ))
TIMEOUT=120 until_ok "checkpoint $next accepted under epoch 2" sh -c "curl -sf $SEQ/v1/checkpoints/$next | jq -e '.status == \"accepted\" and .epoch == \"2\"'"
echo "epoch 2: checkpoints $stale and $next accepted, signed by validators 1, 2 and the new 3"

log "5. the sequencer stops; freeze and escape"
# Let the last checkpoint land first, so every validator and replay agree on it.
until_ok "the submission queue to drain" sh -c "curl -sf $SEQ/v1/status | jq -e '.checkpoints.accepted == .checkpoints.signed'"
kill -INT "$SEQ_PID"; kill -INT "$RELAYER_PID"
sleep "$([[ "$E2E_NETWORK" == local ]] && echo 5 || echo 15)" # a transaction already sent can still land
last_seq="$(view "$SETTLEMENT" last_checkpoint | jq -r .seq)"
V1="http://127.0.0.1:$((SEQ_PORT + 1))"
until_ok "validator 1 to see checkpoint $last_seq accepted" sh -c "curl -sf $V1/v1/status | jq -e '.checkpoints.accepted == \"$last_seq\"'"
TIMEOUT=120 until_ok "freeze (after the 30 s escape timeout)" invoke "$SETTLEMENT" admin freeze
info="$(view "$SETTLEMENT" frozen_info)"
vault="$(view "$USDC_ID" balance --id "$SETTLEMENT" | num)"
num_="$(jq -r .payout_num <<< "$info")"; den="$(jq -r .payout_den <<< "$info")"
echo "frozen: payout $num_ / $den, vault $vault"
total=0
for n in alice bob; do
  g="$(pk "$n")"
  p="$(curl -sf "$V1/v1/proofs/escape?account=$g")"
  [[ "$(jq -r .seq <<< "$p")" == "$last_seq" ]] || fail "escape proof is not from checkpoint $last_seq"
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
  --engine-wasm target/contracts/perps_engine.wasm --prove-escape "$A" > "$WORK/replay.json"
jq -s -e '.[0].ok == true' "$WORK/replay.json" > /dev/null || fail "replay: $(cat "$WORK/replay.json")"
[[ "$(jq -s -r '.[1].equity' "$WORK/replay.json")" == "$(curl -sf "$V1/v1/proofs/escape?account=$A" | jq -r .equity)" ]] || fail "replay's escape proof differs from the validator's"
jq -s -c '.[0]' "$WORK/replay.json"

echo
echo "E2E OK in $(( $(date +%s) - START_TIME )) s: settlement $SETTLEMENT, $(jq -s -r '.[0].checkpoints' "$WORK/replay.json") checkpoints replayed, work dir $WORK"
