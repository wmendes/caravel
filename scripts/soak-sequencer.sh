#!/usr/bin/env bash
# T-007 soak (spec §20.1 T-007 acceptance), on the LOCAL lane:
# - 1 s blocks for DURATION seconds under TPS tx/s of synthetic load;
# - a stop and restart halfway: the restarted sequencer must resume from
#   SQLite at the height and state hash an independent replay of the store gives;
# - every checkpoint rebuilt byte for byte, and their spacing reported.
#
#   DURATION=3600 TPS=50 ./scripts/soak-sequencer.sh
#
# No Stellar and no validators: checkpoints are sealed, not signed.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
DURATION="${DURATION:-3600}"
TPS="${TPS:-50}"
PORT="${PORT:-18080}"
WORK="${WORK:-$(mktemp -d)}"
mkdir -p "$WORK"
echo "work dir: $WORK"

cargo build --release --locked -p caravel-perps-node --bin caravel-perps-node --example loadgen
BIN="$ROOT/target/release/caravel-perps-node"
LOADGEN="$ROOT/target/release/examples/loadgen"
LANE="$ROOT/lanes/perps/config/lane.caravel-perps.local.toml"
HASH="$(node -p 'require("./versions.json").artifacts.engine_wasm_sha256')"

cat > "$WORK/sequencer.toml" <<EOF
[sequencer]
listen = "127.0.0.1:$PORT"
lane = "$LANE"
engine_wasm = "$ROOT/target/contracts/perps_engine.wasm"
engine_wasm_sha256 = "$HASH"
db = "$WORK/sequencer.sqlite"
network_passphrase = "Test SDF Network ; September 2015"
# A placeholder contract id (32 bytes of 0x07): no Stellar in this test.
settlement_contract = "CADQOBYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQP5KR"
EOF

CARAVEL_INTERNAL_TOKEN="$(openssl rand -hex 16)"
export CARAVEL_INTERNAL_TOKEN
URL="http://127.0.0.1:$PORT"

start() {
  RUST_LOG=info "$BIN" sequencer --config "$WORK/sequencer.toml" >> "$WORK/sequencer.log" 2>&1 &
  echo $! > "$WORK/pid"
  for _ in $(seq 1 100); do
    curl -sf "$URL/v1/status" > /dev/null && return 0
    sleep 0.2
  done
  echo "sequencer did not start" >&2
  tail -20 "$WORK/sequencer.log" >&2
  exit 1
}

stop() {
  kill -INT "$(cat "$WORK/pid")"
  wait "$(cat "$WORK/pid")" 2>/dev/null || true
}

half=$((DURATION / 2))

start
"$LOADGEN" --url "$URL" --lane "$LANE" --tps "$TPS" --duration-secs "$half" > "$WORK/load-1.jsonl"
stop
"$BIN" check-store --config "$WORK/sequencer.toml" > "$WORK/check-1.json"

# The restarted sequencer logs the height and state hash it resumed from.
start
resumed="$(grep 'sequencer starting' "$WORK/sequencer.log" | tail -1)"
"$LOADGEN" --url "$URL" --lane "$LANE" --tps "$TPS" --duration-secs "$((DURATION - half))" > "$WORK/load-2.jsonl"
stop
"$BIN" check-store --config "$WORK/sequencer.toml" > "$WORK/check-2.json"

node - "$WORK" "$resumed" <<'EOF'
const fs = require('fs');
const [work, resumed] = process.argv.slice(2);
const c1 = JSON.parse(fs.readFileSync(`${work}/check-1.json`));
const c2 = JSON.parse(fs.readFileSync(`${work}/check-2.json`));
const last = (f) => JSON.parse(fs.readFileSync(`${work}/${f}`, 'utf8').trim().split('\n').pop());
const l1 = last('load-1.jsonl'), l2 = last('load-2.jsonl');
const plain = resumed.replace(/\x1b\[[0-9;]*m/g, '');
const m = plain.match(/height=(\d+) state_hash=([0-9a-f]{64})/);
const resumeOk = m && Number(m[1]) === c1.height && m[2] === c1.final_state_hash;
const out = {
  ok: c1.ok && c2.ok && resumeOk,
  restart: { stopped_at_height: c1.height, replayed_state_hash: c1.final_state_hash, resumed_log: m ? { height: Number(m[1]), state_hash: m[2] } : null, identical: !!resumeOk },
  final: c2,
  load: { first_half: l1, second_half: l2 },
};
console.log(JSON.stringify(out, null, 2));
if (!out.ok) process.exit(1);
EOF
echo "checkpoint spacing (blocks per checkpoint → count):"
sqlite3 "$WORK/sequencer.sqlite" "SELECT last_height - first_height + 1 AS blocks, COUNT(*) FROM checkpoints GROUP BY blocks ORDER BY blocks;"
