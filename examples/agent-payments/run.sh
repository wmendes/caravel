#!/usr/bin/env bash
# Agents paying each other in small amounts on a lane (HackMeridian starter
# kit, H-09). Every payment is a lane transaction, confirmed in a block; the
# money comes from Stellar and goes back to it.
#
#   ./run.sh                       # 3 agents, 30 payments of 0.05
#   AGENTS=5 PAYMENTS=100 ./run.sh
#   KEEP=1 ./run.sh                # leave the lane running afterwards
#
# Needs caravel on PATH (see the README), Docker and Node.js 22.
set -euo pipefail
AGENTS="${AGENTS:-3}"
PAYMENTS="${PAYMENTS:-30}"
AMOUNT="${AMOUNT:-0.05}"
LANE="${LANE:-agents}"
PORT="${PORT:-18980}"
cd "$(dirname "${BASH_SOURCE[0]}")"
export CARAVEL_YES=1

if [[ ! -f "$LANE/lane.toml" ]]; then
  caravel init payments "$LANE" --name "$LANE" --prefix "$LANE" --port "$PORT"
fi
cd "$LANE"
caravel apply

agent() { echo "$LANE-agent-$1"; }
echo "== $AGENTS agents, each funded on Stellar and with 10 on the lane"
for i in $(seq 1 "$AGENTS"); do
  caravel account create "$(agent "$i")" --amount 20 > /dev/null
  caravel deposit "$(agent "$i")" 10 > /dev/null
done

echo "== $PAYMENTS payments of $AMOUNT, every agent paying the next one at once"
start=$(date +%s)
pids=()
for i in $(seq 1 "$AGENTS"); do
  next=$(( i % AGENTS + 1 ))
  (
    for _ in $(seq 1 $(( PAYMENTS / AGENTS ))); do
      caravel tx --json --from "$(agent "$i")" transfer --to "@$(agent "$next")" --amount "$AMOUNT" > /dev/null 2>&1
    done
  ) &
  pids+=($!)
done
for p in "${pids[@]}"; do wait "$p"; done
secs=$(( $(date +%s) - start ))
echo "$(( PAYMENTS / AGENTS * AGENTS )) payments confirmed in ${secs}s"

# A balance in token units (7 decimals), from `caravel balance --json`.
show() {
  caravel balance --json "$1" 2> /dev/null \
    | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const b=JSON.parse(s);console.log(`${process.argv[1]}  lane ${Number(b.lane.balance)/1e7}  Stellar ${b.stellar}`)})' "$1"
}
echo "== balances (each transfer also paid the lane's 0.01 fee)"
for i in $(seq 1 "$AGENTS"); do show "$(agent "$i")"; done

echo "== agent 1 takes 5 back to Stellar (waits for its checkpoint)"
caravel withdraw "$(agent 1)" 5 > /dev/null 2>&1
show "$(agent 1)"

if [[ "${KEEP:-0}" != 1 ]]; then
  echo "== destroy"
  caravel destroy --yes --stop-validators > /dev/null 2>&1
fi
