# Spike: a proven Payments checkpoint (M0.12 X-01)

A RISC Zero guest proves one Caravel Payments checkpoint. It starts from the previous checkpoint's header and state, runs the Payments engine over every block of the batch, and rebuilds the header. Stellar checks the proof in a contract. Design and results: [`docs/DESIGN_PROVEN_LANES.md`](../../docs/DESIGN_PROVEN_LANES.md).

This directory is a workspace of its own. It is outside the root workspace, its `Cargo.lock`, `versions.json` and CI. It changes no Caravel code; the guest uses `caravel-core`, `caravel-app-sdk` and `caravel-payments` by path.

| Path | What it is |
|---|---|
| `methods/guest` | The guest (RISC Zero zkVM 3.0.6). It is a `std` guest, because the accelerated curve25519 needs `std`. Its journal is `H(prev_header) ‖ H(header)`. |
| `host` | Builds a checkpoint on the native harness lane, then runs the guest: `exec` (cycles), `prove` (`composite`, `succinct` or `groth16`), and `tamper`. |
| `contract` | `proven-checkpoint`: checks 4 and 5 as the settlement contract does today, then the proof in place of check 7. |

```sh
# RISC Zero toolchain: rzup, then `rzup install rust`, `cpp`, `cargo-risczero 3.0.6`, `r0vm 3.0.6`
export PATH="$HOME/.risc0/bin:$PATH"
cargo build --release -p zk-payments-host
./target/release/zk-payments-host exec   --accounts 64 --blocks 20 --txs 10
./target/release/zk-payments-host tamper --accounts 8 --blocks 2 --txs 2
RISC0_PROVER=local ./target/release/zk-payments-host prove --accounts 16 --blocks 10 --txs 0 --kind groth16 --out proof.json
(cd contract && stellar contract build)
```

On testnet (2026-10-07):
- the verifier is NethermindEth's `groth16-verifier` (RISC Zero 3.0.0 parameters, selector `73c457ba`) at `CAMLOKU4P3WZW4UW3V3YPGAI64GGTK2CMRAGKOOHLSI7WYXXBLHMBI2S`;
- `proven-checkpoint` is at `CAKXSLR7TKZQU74HTPWRGQ4CYUWDM5T7WXLGASSYZNGI75QTOSWE7SXZ`, with image ID `7cd8ebec…d973`.
