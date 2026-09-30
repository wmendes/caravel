#!/usr/bin/env bash
# T-012: deploys the engine and settlement contracts to Stellar TESTNET and
# sends the witness `step` transaction (spec §20.1).
#
#   gh run download <main run id> -n contracts-wasm -D /tmp/wasm
#   WASM_DIR=/tmp/wasm ./scripts/deploy-testnet.sh
#
# - The Wasm files must be the ones of record (x86_64 Linux builds from CI,
#   DEC-033): both hashes are checked against versions.json.
# - Keys live in the Stellar CLI keystore (never in git): caravel-admin
#   (deployer, contract admin), caravel-relayer and caravel-validator-1..3 are
#   created and funded by friendbot if missing; caravel-backstop,
#   caravel-treasury and caravel-oracle must match the lane file.
# - It refuses to redeploy when versions.json already has contract IDs,
#   unless FORCE=1.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
WASM_DIR="${WASM_DIR:?set WASM_DIR to the CI contracts-wasm artifact}"
NET=(--network testnet)
RPC="https://soroban-testnet.stellar.org"
PASS="Test SDF Network ; September 2015"
LANE="$ROOT/lanes/perps/config/lane.caravel-perps.testnet.toml"
USDC_SAC="CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA"
LOG="$(mktemp)"
fail() { echo "FAIL: $*" >&2; exit 1; }
sc() { stellar "$@" 2>> "$LOG"; }
pk() { sc keys public-key "$1"; }
raw() { node -e 'const {StrKey}=require(process.argv[1]);console.log(Buffer.from(StrKey.decodeEd25519PublicKey(process.argv[2])).toString("hex"))' "$ROOT/platform/relayer/node_modules/@stellar/stellar-sdk" "$1"; }
recorded() { node -p "require('./versions.json').$1"; }

echo "== Wasm of record"
for pair in "perps_engine:artifacts.engine_wasm_sha256" "settlement:artifacts.settlement_wasm_sha256"; do
  f="$WASM_DIR/${pair%%:*}.wasm"
  [[ -f "$f" ]] || fail "$f is missing"
  have="$(shasum -a 256 "$f" | awk '{print $1}')"
  [[ "$have" == "$(recorded "${pair##*:}")" ]] || fail "$f is $have, versions.json records $(recorded "${pair##*:}")"
  echo "${pair%%:*} $have"
done
if [[ "$(recorded testnet.settlement_contract)" != FILLED_BY_* && "${FORCE:-0}" != "1" ]]; then
  fail "versions.json already has testnet contracts; FORCE=1 deploys new ones"
fi

echo "== keys"
for n in caravel-admin caravel-relayer caravel-validator-1 caravel-validator-2 caravel-validator-3; do
  if ! sc keys public-key "$n" > /dev/null; then sc keys generate "$n" > /dev/null; fi
  sc keys fund "$n" "${NET[@]}" > /dev/null || true
  echo "$n $(pk "$n")"
done
for pair in "caravel-backstop:backstop_key" "caravel-treasury:treasury_key"; do
  grep -q "${pair##*:} = \"$(pk "${pair%%:*}")\"" "$LANE" || fail "${pair%%:*} is not the lane's ${pair##*:}"
done
grep -q "\"$(pk caravel-oracle)\"" "$LANE" || fail "caravel-oracle is not the lane's oracle key"

echo "== engine contract"
cargo build --release --locked -p caravel-perps-node -q
BIN="$ROOT/target/release/caravel-perps-node"
ENGINE="$(sc contract deploy --wasm "$WASM_DIR/perps_engine.wasm" --source-account caravel-admin "${NET[@]}" --alias caravel-perps-engine)"
echo "engine $ENGINE"

echo "== settlement contract"
genesis="$("$BIN" genesis --config "$LANE")"
signers="$(for i in 1 2 3; do raw "$(pk "caravel-validator-$i")"; done | sort | jq -R '{key: ., weight: 1}' | jq -sc '{signers: ., threshold: 2}')"
# M0 testnet defaults (spec §13.1).
params='{"force_inclusion_window_secs":3600,"escape_timeout_secs":21600,"min_rotation_delay_secs":3600,"signer_retention_epochs":2,"min_deposit":"10000000"}'
SETTLEMENT="$(sc contract deploy --wasm "$WASM_DIR/settlement.wasm" --source-account caravel-admin "${NET[@]}" --alias caravel-settlement -- \
  --admin "$(pk caravel-admin)" --usdc "$USDC_SAC" --lane_id "$(jq -r .lane_id <<< "$genesis")" \
  --engine_wasm_hash "$(recorded artifacts.engine_wasm_sha256)" --genesis_state_hash "$(jq -r .genesis_state_hash <<< "$genesis")" \
  --config_hash "$(jq -r .config_hash <<< "$genesis")" --signers "$signers" --params "$params")"
echo "settlement $SETTLEMENT"

echo "== witness step transaction"
w="$("$BIN" witness --lane "$LANE" --engine-wasm "$WASM_DIR/perps_engine.wasm" --depositor "$(pk caravel-admin)")"
: > "$LOG"
out="$(sc contract invoke --id "$ENGINE" --source-account caravel-admin "${NET[@]}" --send=yes -- step --state "$(jq -r .state_hex <<< "$w")" --block "$(jq -r .block_hex <<< "$w")" | tr -d '"')"
tx="$(grep -oE 'Signing transaction: [0-9a-f]{64}' "$LOG" | tail -1 | awk '{print $3}')"
[[ -n "$tx" ]] || fail "no transaction hash in the CLI output"
[[ "$out" == "$(jq -r .expected_hex <<< "$w")" ]] || fail "the witness output differs from the executor's"
txinfo="$(curl -s -X POST "$RPC" -H 'content-type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getTransaction\",\"params\":{\"hash\":\"$tx\"}}")"
ledger="$(jq -r .result.ledger <<< "$txinfo")"
proto="$(curl -s -X POST "$RPC" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"getNetwork"}' | jq -r .result.protocolVersion)"
echo "witness tx $tx at ledger $ledger (protocol $proto): output byte-equal, sha256 $(jq -r .expected_sha256 <<< "$w")"

echo "== record"
node - "$ENGINE" "$SETTLEMENT" <<'EOF'
const fs = require("fs");
const [engine, settlement] = process.argv.slice(2);
const v = JSON.parse(fs.readFileSync("versions.json", "utf8"));
v.testnet.engine_contract = engine;
v.testnet.settlement_contract = settlement;
fs.writeFileSync("versions.json", JSON.stringify(v, null, 2) + "\n");
EOF
echo "engine $ENGINE, settlement $SETTLEMENT, witness $tx: record them in docs/RESULTS.md"
