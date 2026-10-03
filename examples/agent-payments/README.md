# Agents paying each other on a lane

A starter for HackMeridian teams. Some apps want agents (or machines, or game characters) to pay each other a lot, in small amounts, and to know quickly that each payment went through. On Stellar directly, each payment waits for a ledger, about every 5 seconds. On a lane, each payment is confirmed in the lane's next block, and you pick the block time. The money still comes from Stellar and goes back to it.

This recipe does that with the Payments template:

1. It makes a Payments lane on your machine, on a local Stellar network.
2. It gives a few agents test USDC on Stellar and deposits 10 of it onto the lane for each.
3. Every agent pays the next one 0.05 at a time, all at once. Each `caravel tx` returns when a block has the payment.
4. It prints the balances, then sends 5 of agent 1's balance back to Stellar. That waits for the lane's next checkpoint to be accepted there.
5. It winds the lane down with `caravel destroy`.

```sh
./run.sh                        # 3 agents, 30 payments of 0.05
AGENTS=5 PAYMENTS=100 ./run.sh
KEEP=1 ./run.sh                 # keep the lane running to play with it
```

On a laptop, 30 payments between 3 agents took about 15 seconds. Each payment is a separate `caravel tx` that signs, sends and waits for its block. An agent of yours would talk to the lane's API directly and go faster.

## What you need

`caravel` on your PATH (see the main README), Docker for the local network, and Node.js 22.

## Where to take it

- **Your agents, not a loop.** Replace the loop with your agents. Each one is a Stellar CLI identity, and `caravel tx --json` gives you the result to act on.
- **Your own fee.** The lane charges 0.01 per transfer, which goes to the lane's treasury. `transfer_fee` in `lane.toml` sets it. It's part of the lane's genesis, so set it before the first `caravel apply`.
- **Your own block time.** `block_time_ms` under `[node]` (200 to 5000) isn't part of genesis. Change it and `caravel apply` restarts the nodes.
- **Testnet.** Add an `[env.testnet]` deployment to the lane file ([`docs/LANE_FILE.md`](../../docs/LANE_FILE.md)) and run the same steps with `--env testnet`.

## Be honest about what this is

Lanes are testnet software and not audited. Between checkpoints the lane's validators are trusted, and here they all run on your machine. Payments on the lane are final for the lane at once. They reach Stellar at the next checkpoint, and that's when anyone can check them there.
