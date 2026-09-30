//! Caravel Payments engine contract (spec §20.4.4): a thin Soroban wrapper
//! around `caravel-payments` on the app SDK. It has no storage. Lane nodes
//! execute this exact Wasm through `soroban-env-host` for consensus (DEC-002).
#![no_std]
#![deny(clippy::float_arithmetic)]

extern crate alloc;

use caravel_app_sdk::Crypto;
use caravel_core::codes::fatal;
use caravel_core::step::StepEnvelope;
use caravel_payments::{Payments, ENGINE_VERSION};
use soroban_sdk::{contract, contracterror, contractimpl, panic_with_error, Bytes, BytesN, Env};

/// The platform's fatal codes (spec §20.4.3), surfaced as contract errors.
/// The entry index is lost here; an invalid signature traps inside
/// `ed25519_verify` instead of returning `BadSignature`.
#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum EngineError {
    BadStateEncoding = 1,
    BadBlockEncoding = 2,
    BadConfig = 3,
    WrongLaneBlock = 4,
    BadHeight = 5,
    BadPrevHash = 6,
    TimeRegression = 7,
    BlockTooLarge = 8,
    TooManyEntries = 9,
    EntryOrder = 10,
    InboxGap = 11,
    BadEntryEncoding = 12,
    UnknownFeedKey = 13,
    DuplicateFeedSlot = 14,
    PendingQueueOverflow = 15,
    ArithmeticOverflow = 16,
    BadSignature = 17,
}

/// The engine's crypto on the Soroban host.
pub struct SorobanCrypto<'a>(pub &'a Env);

impl Crypto for SorobanCrypto<'_> {
    fn sha256(&self, data: &[u8]) -> [u8; 32] {
        self.0
            .crypto()
            .sha256(&Bytes::from_slice(self.0, data))
            .to_array()
    }

    /// Traps on an invalid key or signature (spec §8.4).
    fn ed25519_verify(&self, public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]) {
        self.0.crypto().ed25519_verify(
            &BytesN::from_array(self.0, public_key),
            &Bytes::from_slice(self.0, message),
            &BytesN::from_array(self.0, signature),
        );
    }
}

fn fail(env: &Env, code: u16) -> ! {
    panic_with_error!(
        env,
        soroban_sdk::Error::from_contract_error(u32::from(code))
    )
}

#[contract]
pub struct PaymentsEngine;

#[contractimpl]
impl PaymentsEngine {
    /// Rules identity: `H("CARAVEL/ENGINE/V1" || "payments/0.1.0")`.
    pub fn version(env: Env) -> BytesN<32> {
        let preimage = caravel_core::preimage::engine_version_preimage(ENGINE_VERSION);
        BytesN::from_array(&env, &SorobanCrypto(&env).sha256(&preimage))
    }

    /// `AppGenesisV1` bytes → genesis state bytes.
    pub fn genesis(env: Env, config: Bytes) -> Bytes {
        match caravel_app_sdk::genesis::<Payments>(&config.to_alloc_vec(), &SorobanCrypto(&env)) {
            Ok(state) => Bytes::from_slice(&env, &state),
            Err(f) => fail(&env, f.code),
        }
    }

    /// Returns `"CVSTEP01" · state_len u32 · state · receipts_len u32 · receipts`.
    pub fn step(env: Env, state: Bytes, block: Bytes) -> Bytes {
        let out = match caravel_app_sdk::step::<Payments, _>(
            &state.to_alloc_vec(),
            &block.to_alloc_vec(),
            &SorobanCrypto(&env),
        ) {
            Ok(out) => out,
            Err(f) => fail(&env, f.code),
        };
        match (StepEnvelope {
            state: out.state,
            receipts: out.receipts,
        })
        .encode()
        {
            Ok(bytes) => Bytes::from_slice(&env, &bytes),
            Err(_) => fail(&env, fatal::ARITHMETIC_OVERFLOW),
        }
    }
}
