# Sources

Every external source used to build Caravel. Spec §0.3 rule 2 and §5 set the rules:
- copy code only from MIT or Apache-2.0 files;
- keep the upstream header;
- list the source here.

Never copy GPL files (for example SoroDOOM's PureDOOM files). Stellar facts are checked through the stellar-raven MCP where it covers them. Crate and npm APIs are checked on docs.rs and npm for the pinned version.

## Copied code

None yet.

## Facts and versions checked

| Date | What | Source | Used in |
|---|---|---|---|
| 2026-09-29 | Testnet on Protocol 28 "Adapter" since 2026-08-27; mainnet vote 2026-09-16 | stellar-raven: SDF blog "Introducing Adapter, Protocol 28 on Stellar" and "Adapter, Protocol 28 Upgrade Guide" (stellar.org/blog/developers, 2026-08-13) | `versions.json` `stellar_protocol` |
| 2026-09-29 | Testnet passphrase `Test SDF Network ; September 2015`, RPC `https://soroban-testnet.stellar.org` | stellar-raven: developers.stellar.org "Fully-Typed Contracts" | `versions.json` `testnet` |
| 2026-09-29 | Testnet USDC SAC `CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA`, issuer `GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5` | stellar-raven: developers.stellar.org "Verify Trustlines" and the x402 quickstart guide | `versions.json` `testnet.usdc_sac` |
| 2026-09-29 | Protocol 28 software table lists Rust XDR 28.0.0 and Rust SDK 28.0.0 | stellar-raven: developers.stellar.org "Software Versions" (updated 2026-09-24) | DEC-018 |
| 2026-09-29 | `soroban-env-common 28.0.2` requires `stellar-xdr =28.0.0`; `soroban-sdk 28.0.0` requires `stellar-strkey =0.0.16`; `soroban-env-host 28.0.2` requires `ed25519-dalek ^2.0.0`, `sha2 ^0.10.8`, `stellar-strkey ^0.0.13` | crates.io dependency API | DEC-018, `Cargo.toml` |
| 2026-09-29 | MSRV: soroban-sdk 28.0.0 1.91.0, soroban-env-host 28.0.2 1.84.0, stellar-cli 28.1.0 1.93.0 | crates.io | `rust-toolchain.toml` 1.93.0 |
| 2026-09-29 | `stellar contract build` (CLI 28.1.0) runs `cargo rustc --crate-type=cdylib --target=wasm32v1-none --release`, remaps the registry path, optimizes by default and records the CLI version | `stellar contract build --print-commands-only` and `--help` | DEC-020, `scripts/build-contracts.sh` |
| 2026-09-29 | stellar-cli 28.1.0 Linux x86_64 asset sha256 `c1680dee…6462` | GitHub release API, asset digest | `.github/workflows/ci.yml` |
| 2026-09-29 | Apache License 2.0 text | https://www.apache.org/licenses/LICENSE-2.0.txt | `LICENSE-APACHE` |
| 2026-09-29 | `soroban-env-host 28.0.2` verifies ed25519 with `VerifyingKey::from_bytes` + `verify_strict` (`src/crypto/mod.rs`, `verify_sig_ed25519_internal`); depends on `ed25519-dalek 2.0.0` (feature `rand_core`) and `sha2 0.10.8` | crate source in the cargo registry | §8.4, `test-vectors/signatures.json` |
| 2026-09-29 | ed25519 group order `L = 2^252 + 27742317777372353535851937790883648493` | RFC 8032 §5.1 | `S + L` vector in `test-vectors/signatures.json` |
| 2026-09-29 | Cross-checks of the T-001 vectors: every signature verifies under Node 24's OpenSSL ed25519; `@stellar/stellar-sdk 17.2.0` `Keypair.signMessage` produces the vector's SEP-53 signature byte for byte and `verifyMessage` accepts it; Python `hashlib` reproduces every `H(hex)` | local tools | `test-vectors/*.json` |
| 2026-09-29 | `soroban-sdk 28.0.0` `build.rs` rejects Wasm builds unless `SOROBAN_SDK_BUILD_SYSTEM_SUPPORTS_SPEC_SHAKING_V2` is set (by stellar-cli ≥ 25.2.0); `Crypto::sha256(&Bytes) -> Hash<32>`, `Hash::to_array`, `Vec::from_iter(&Env, iter)` | crate source in the cargo registry | §12.3, `caravel-merkle` Soroban hasher |
