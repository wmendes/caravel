#!/usr/bin/env bash
# A lane with an oracle: an order book priced by a feed the relayer posts
# (HackMeridian starter kit, H-09). It's the Perps template, the app behind
# Caravel Perps on testnet, on your machine with fixed prices.
#
#   ./run.sh            # make the lane, trade once, wind it down
#   KEEP=1 ./run.sh     # leave the lane running afterwards
#
# Needs caravel on PATH (see the README), Docker and Node.js 22.
set -euo pipefail
LANE="${LANE:-trade}"
PORT="${PORT:-19080}"
cd "$(dirname "${BASH_SOURCE[0]}")"
export CARAVEL_YES=1

if [[ ! -f "$LANE/lane.toml" ]]; then
  caravel init perps "$LANE" --name "$LANE" --prefix "$LANE" --port "$PORT"
fi
cd "$LANE"
caravel apply

echo "== the oracle's first price on the lane (BTC at 65,000)"
caravel wait --timeout 90 api /v1/markets /0/oracle_price=65000000 > /dev/null
A="$LANE-alice"; B="$LANE-bob"
for who in "$A" "$B"; do
  caravel account create "$who" --amount 2000 > /dev/null
  caravel deposit "$who" 1000 > /dev/null
done
echo "== $A and $B have 1,000 test USDC of collateral each on the lane"

# Prices per lot in token units: a lot is 0.0001 BTC, so 6.5 is 65,000 per BTC.
echo "== $A rests a bid for 10 lots at 6.5; $B sells 4 into it"
caravel tx --from "$A" place-order --market 1 --side buy --tif gtc --price 6.5 --lots 10 > /dev/null 2>&1
caravel wait --timeout 30 api '/v1/markets/1/book?depth=1' /bids/0/lots=10 > /dev/null
caravel tx --from "$B" place-order --market 1 --side sell --tif ioc --price 6.4 --lots 4 > /dev/null 2>&1
caravel wait --timeout 30 api "/v1/accounts/$(caravel keys show "$A")" /positions/0/lots=4 > /dev/null
echo "== the trade, from the lane's API"
caravel api '/v1/markets/1/trades?limit=1'
echo
echo "== $A's account"
caravel api "/v1/accounts/$(caravel keys show "$A")"
echo

if [[ "${KEEP:-0}" != 1 ]]; then
  echo "== destroy"
  caravel destroy --yes --stop-validators > /dev/null 2>&1
fi
