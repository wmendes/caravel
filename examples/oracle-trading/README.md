# A lane with an oracle

A starter for HackMeridian teams. Trading on an order book needs quick blocks and fresh prices. This is Caravel Perps, the app behind our testnet lane, on your machine. It runs perpetual futures on an order book, marked by an oracle price that the lane's relayer posts every couple of seconds.

`./run.sh`:

1. makes a Perps lane on a local Stellar network, with fixed prices (BTC 65,000, ETH 3,500, XLM 0.40);
2. waits for the first oracle price on the lane;
3. gives Alice and Bob test USDC on Stellar and deposits 1,000 each as collateral;
4. Alice rests a bid for 10 lots, and Bob sells 4 into it;
5. prints the trade and Alice's position from the lane's API, then winds the lane down.

```sh
./run.sh
KEEP=1 ./run.sh       # keep it running, then try caravel api /v1/markets
```

## What you need

`caravel` on your PATH (see the main README), Docker for the local network, and Node.js 22.

## Where to take it

- **Live prices.** In `lane.toml`, replace a market's `[{ fixed = "65000" }]` with `[{ coinbaseStream = "BTC-USD" }, { coinbase = "BTC-USD" }]`, then `caravel apply`. It restarts the relayer, and nothing else.
- **Your own source.** The relayer loads feed modules (`lanes/perps/relayer-feeds`), which decide where prices come from: see `coinbase.ts` and `reflector.ts`. Add your own source next to them, and the lane signs what it reports with the oracle key.
- **A front end.** Caravel Perps' web app is in `lanes/perps/web`, and the lane's API (`caravel api /v1/...`) is what it reads.

## Be honest about what this is

Lanes are testnet software and not audited. Between checkpoints the validators are trusted, and here they all run on your machine. The oracle key here is a local test identity. Prices are only as good as the feed that signs them.
