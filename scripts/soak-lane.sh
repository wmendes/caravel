#!/usr/bin/env bash
# A full-stack soak (M0.9, F-02): the perps lane on a local Stellar network
# (sequencer, 3 validators, the relayer), deployed with `caravel apply` like
# the e2e, under synthetic load from accounts that deposited through the
# settlement contract. It reports what the performance cycle tracks:
# per-phase timings from every node (F-01), store bytes per block and per
# day, node CPU and memory, soft and hard latency, sustained rate.
#
#   BLOCK_MS=500 TPS=45 DURATION=600 ./scripts/soak-lane.sh
#   BLOCK_MS=200 TPS=100 ACCOUNTS=48 INFLIGHT=2 ./scripts/soak-lane.sh
#   TPS=0 DURATION=600 ./scripts/soak-lane.sh      # idle: storage of empty blocks
#
# CHECKPOINT_EVERY defaults to one checkpoint a minute at BLOCK_MS. With
# URGENT_MS, BUSY_MS and IDLE_MS the lane checkpoints by time and content
# (§14.2 d-f, as lane #1: 5000, 60000 and an hour), and CHECKPOINT_EVERY
# defaults to an hour of blocks; without them, the scaffold's own times
# apply. SKIP_BUILD=1 reuses the builds; KEEP=1 leaves the lane running.
# The summary is the last line of stdout and $WORK/summary.json; the
# per-report rows are $WORK/load.csv and the node samples $WORK/samples.jsonl.
set -euo pipefail
START_TIME=$(date +%s)

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
BLOCK_MS="${BLOCK_MS:-500}"
if [[ -n "${URGENT_MS:-}${BUSY_MS:-}${IDLE_MS:-}" ]]; then
  : "${URGENT_MS:?set URGENT_MS, BUSY_MS and IDLE_MS together}" "${BUSY_MS:?}" "${IDLE_MS:?}"
  CHECKPOINT_EVERY="${CHECKPOINT_EVERY:-$(( 3600000 / BLOCK_MS ))}"
else
  CHECKPOINT_EVERY="${CHECKPOINT_EVERY:-$(( 60000 / BLOCK_MS ))}"
fi
TPS="${TPS:-45}"
DURATION="${DURATION:-600}"
ACCOUNTS="${ACCOUNTS:-24}"
INFLIGHT="${INFLIGHT:-2}"
PORT="${PORT:-19080}"
SAMPLE_SECS="${SAMPLE_SECS:-10}"
WORK="${WORK:-$(mktemp -d)}"
mkdir -p "$WORK/logs" "$WORK/keys"
chmod 700 "$WORK/keys"
export XDG_CONFIG_HOME="$WORK/xdg"
CARAVEL="$ROOT/target/release/caravel"
LOADGEN="$ROOT/target/release/examples/loadgen"
LANE="$WORK/lane.toml"
SEQ="http://127.0.0.1:$PORT"
echo "work dir: $WORK"

log() { printf '\n== %s\n' "$*" >&2; }
fail() { echo "FAIL: $*" >&2; exit 1; }

cleanup() {
  local code=$?
  [[ -n "${SAMPLER:-}" ]] && kill "$SAMPLER" 2>/dev/null || true
  if [[ "${KEEP:-0}" != "1" ]]; then
    for f in "$WORK"/.caravel/*/*/run/*.pid; do [[ -f "$f" ]] && kill "$(cat "$f")" 2>/dev/null || true; done
    stellar container stop local > /dev/null 2>&1 || true
  fi
  if (( code != 0 )); then
    echo "logs and state: $WORK" >&2
    for f in "$WORK"/.caravel/*/*/logs/*.log; do [[ -f "$f" ]] && { echo "--- $(basename "$f")" >&2; tail -15 "$f" >&2; }; done
  fi
}
trap cleanup EXIT

caravel() { local cmd="$1"; shift; (cd "$WORK" && "$CARAVEL" "$cmd" -f "$LANE" --env soak --var-file "$WORK/soak.vars.toml" "$@") 2>> "$WORK/logs/caravel.log"; }
q() { local cmd="$1"; shift; caravel "$cmd" --json "$@"; }

log "tools"
for t in docker jq stellar sqlite3; do command -v "$t" > /dev/null || fail "$t is required"; done
docker info > /dev/null 2>&1 || fail "the Docker daemon is not running"

if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
  log "build"
  ./scripts/build-contracts.sh > "$WORK/logs/build.log"
  # Bins and the example apart: `--example` alone would build only the example.
  cargo build --release --locked -p caravel-perps-node -p caravel-cli --bins 2>> "$WORK/logs/build.log"
  cargo build --release --locked -p caravel-perps-node --example loadgen 2>> "$WORK/logs/build.log"
  npm --prefix platform/relayer ci --silent && npm --prefix platform/relayer run build --silent
  npm --prefix lanes/perps/relayer-feeds ci --silent && npm --prefix lanes/perps/relayer-feeds run build --silent
fi

log "lane: ${BLOCK_MS} ms blocks, a checkpoint every $CHECKPOINT_EVERY"
(cd "$WORK" && "$CARAVEL" init perps scaffold --name soak-perps --prefix soak --port "$PORT") > "$WORK/logs/init.log" 2>&1 \
  || { cat "$WORK/logs/init.log"; fail "caravel init"; }
TIMES=()
[[ -n "${URGENT_MS:-}" ]] && TIMES=(-e "s/^checkpoint_urgent_ms = [0-9]*/checkpoint_urgent_ms = $URGENT_MS/" \
  -e "s/^checkpoint_busy_ms = [0-9]*/checkpoint_busy_ms = $BUSY_MS/" -e "s/^checkpoint_idle_ms = [0-9]*/checkpoint_idle_ms = $IDLE_MS/")
{ echo 'include = ["soak-env.toml"]'; sed -e "s/^block_time_ms = [0-9]*/block_time_ms = $BLOCK_MS/" \
    -e "s/^checkpoint_every_blocks = [0-9]*/checkpoint_every_blocks = $CHECKPOINT_EVERY/" ${TIMES[@]+"${TIMES[@]}"} "$WORK/scaffold/lane.toml"; } > "$LANE"
grep -q "^block_time_ms = $BLOCK_MS" "$LANE" || fail "block_time_ms not set"
cp "$ROOT/scripts/soak/env.toml" "$WORK/soak-env.toml"
{
  echo "port = $PORT"
  echo 'feed_keys = { CARAVEL_ORACLE_SECRET = "soak-oracle" }'
  echo 'feeds = [{ module = "relayer-feeds/perps/dist/index.js", intervalMs = 2000, options = { maxSourceAgeSecs = 900, markets = { "1" = [{ fixed = "65000" }], "2" = [{ fixed = "3500" }], "3" = [{ fixed = "0.40" }] } } }]'
} > "$WORK/soak.vars.toml"
caravel validate > /dev/null || fail "caravel validate"
caravel keys ensure > /dev/null
caravel apply --yes >> "$WORK/logs/apply.log" || { tail -30 "$WORK/logs/apply.log"; fail "caravel apply"; }
caravel wait --timeout 90 api /v1/markets /0/oracle_price=65000000 > /dev/null || fail "oracle prices"
STATE="$(ls -d "$WORK"/.caravel/*/soak)"

(( TPS > 0 )) || ACCOUNTS=0
log "$ACCOUNTS load accounts deposit through Stellar"
for (( i = 1; i <= ACCOUNTS; i++ )); do
  caravel account create "load-$i" --amount 2000 > /dev/null || fail "account load-$i"
  q deposit "load-$i" 1000 | jq -e '.credited' > /dev/null || fail "load-$i's deposit"
  stellar keys show "load-$i" > "$WORK/keys/load-$i.key"
done

# Each store: the bytes its live pages hold (what a reader sees, WAL frames
# included; the free list left out), and its files on disk (database + WAL).
size() { [[ -f "$1" ]] && { stat -f %z "$1" 2>/dev/null || stat -c %s "$1"; } || echo 0; }
stores() {
  for f in "$STATE"/data/*.sqlite; do
    local used
    used="$(sqlite3 -readonly "$f" 'SELECT (page_count - freelist_count) * page_size FROM pragma_page_count, pragma_freelist_count, pragma_page_size;')"
    printf '"%s":{"used":%d,"disk":%d},' "$(basename "$f" .sqlite)" "$used" $(( $(size "$f") + $(size "$f-wal") ))
  done | sed 's/,$//; s/^/{/; s/$/}/'
}
height() { q api /v1/status | jq -r .height; }
# Every SAMPLE_SECS: each node's perf phases, CPU and RSS.
sample() {
  while :; do
    local t; t=$(date +%s)
    for pidf in "$STATE"/run/*.pid; do
      local node pid port status
      node="$(basename "$pidf" .pid)"; pid="$(cat "$pidf")"
      case "$node" in sequencer) port=$PORT ;; validator-*) port=$(( PORT + ${node#validator-} )) ;; *) port= ;; esac
      status='null'
      [[ -n "$port" ]] && status="$(curl -sf --max-time 2 "http://127.0.0.1:$port/v1/status" | jq -c '.perf.phases // null' 2>/dev/null || echo null)"
      ps -o pcpu=,rss= -p "$pid" 2>/dev/null | awk -v t="$t" -v n="$node" -v p="${status:-null}" '{printf "{\"t\":%d,\"node\":\"%s\",\"cpu_pct\":%s,\"rss_kb\":%s,\"perf\":%s}\n", t, n, $1, $2, p}'
    done >> "$WORK/samples.jsonl"
    sleep "$SAMPLE_SECS"
  done
}

log "load: $TPS tx/s for $DURATION s, $INFLIGHT in flight per account"
H0="$(height)"; S0="$(stores)"; T0=$(date +%s)
sample & SAMPLER=$!
if (( TPS > 0 )); then
  "$LOADGEN" --url "$SEQ" --lane "$LANE" --key-dir "$WORK/keys" --no-fund --no-oracle --measure \
    --tps "$TPS" --duration-secs "$DURATION" --inflight "$INFLIGHT" --report-secs 60 \
    --csv "$WORK/load.csv" > "$WORK/load.jsonl" 2> "$WORK/logs/loadgen.log" \
    || { tail -20 "$WORK/logs/loadgen.log"; fail "loadgen"; }
else
  sleep "$DURATION"; : > "$WORK/load.jsonl"
fi
kill "$SAMPLER" 2>/dev/null || true; SAMPLER=
H1="$(height)"; S1="$(stores)"; T1=$(date +%s)
POLICY="$(q api /v1/status | jq -c '.checkpoint_policy // null')"
# The relayer's checkpoints during the load, with their fees (K-06).
METRICS="$STATE/data/relayer-checkpoints.jsonl"
CKPTS="$(jq -s -c --argjson t0 "$T0" --argjson t1 "$T1" '
  map(select((.submitted_at | sub("\\.[0-9]+Z$"; "Z") | fromdate) as $t | $t >= $t0 and $t <= $t1)) |
  { n: length,
    fee_stroops: (map(.fee_charged_stroops | tonumber) | add // 0),
    rent_stroops: (map(.rent_fee_stroops // "0" | tonumber) | add // 0),
    fee_p50_stroops: (map(.fee_charged_stroops | tonumber) | sort | .[length / 2 | floor] // null),
    batch_p50: (map(.batch_bytes) | sort | .[length / 2 | floor] // null) }' "$METRICS" 2>/dev/null || echo null)"

jq -n -c \
  --argjson block_ms "$BLOCK_MS" --argjson every "$CHECKPOINT_EVERY" --argjson tps "$TPS" --argjson accounts "$ACCOUNTS" \
  --argjson inflight "$INFLIGHT" --argjson h0 "$H0" --argjson h1 "$H1" --argjson t0 "$T0" --argjson t1 "$T1" \
  --argjson s0 "$S0" --argjson s1 "$S1" --argjson policy "$POLICY" --argjson ckpts "$CKPTS" \
  --slurpfile load <(grep '"final":true' "$WORK/load.jsonl") --slurpfile samples "$WORK/samples.jsonl" '
  ($t1 - $t0) as $secs | ($h1 - $h0) as $blocks |
  {
    lane: { block_ms: $block_ms, checkpoint_every_blocks: $every },
    load: { tps: $tps, accounts: $accounts, inflight: $inflight, secs: $secs, blocks: $blocks },
    stores: ($s1 | to_entries | map((.value.used - ($s0[.key].used // 0)) as $grew | { key, value: {
      used: .value.used,
      disk: .value.disk,
      grew: $grew,
      bytes_per_block: (if $blocks > 0 then ($grew / $blocks | floor) else null end),
      mb_per_day: (($grew / $secs * 86400 / 1e5 | round) / 10)
    } }) | from_entries),
    nodes: ($samples | group_by(.node) | map({ key: .[0].node, value: {
      cpu_pct_avg: ((map(.cpu_pct) | add / length) * 10 | round / 10),
      cpu_pct_max: (map(.cpu_pct) | max),
      rss_mb_max: ((map(.rss_kb) | max) / 1024 | round),
      perf: (map(select(.perf != null)) | last | .perf // null)
    } }) | from_entries),
    checkpoints: ($ckpts + { per_day: (if $ckpts then ($ckpts.n / $secs * 86400 | round) else null end),
                             xlm_per_day: (if $ckpts then ($ckpts.fee_stroops / 1e7 / $secs * 86400 * 10 | round / 10) else null end),
                             end_reasons: $policy.end_reasons }),
    result: ($load[0] // null | if . then del(.hard_samples, .perf) else . end)
  }' | tee "$WORK/summary.json"
echo "soak done in $(( $(date +%s) - START_TIME )) s, work dir $WORK" >&2
