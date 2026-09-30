# Payments test vectors

Golden vectors for the Caravel Payments formats (template `payments` 0.1.0,
spec §20.4.4, DEC-060):

| File | Format |
|---|---|
| `payments.json` | `app_params`, the `TRANSFER` body (kind 16) and event (type 16), invalid bodies, and a short run: config, genesis state, then three blocks with their receipts and states |

`lanes/payments/node/tests/vectors.rs` generates and checks them, and replays
the run through the engine Wasm of record. They are frozen: a change is a
format change and needs a version bump and a DEC (spec §0.4).
