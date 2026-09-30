//! The contract's own surface, natively through soroban-sdk testutils. The
//! consensus path is the Wasm (`lanes/payments/node/tests/parity.rs`).

use caravel_core::preimage::engine_version_preimage;
use payments_engine::{EngineError, PaymentsEngine, PaymentsEngineClient};
use sha2::Digest;
use soroban_sdk::{Bytes, Env};

#[test]
fn version_is_the_rules_identity() {
    let env = Env::default();
    let client = PaymentsEngineClient::new(&env, &env.register(PaymentsEngine, ()));
    let want: [u8; 32] = sha2::Sha256::digest(engine_version_preimage("payments/0.1.0")).into();
    assert_eq!(client.version().to_array(), want);
}

#[test]
fn a_bad_config_is_a_contract_error() {
    let env = Env::default();
    let client = PaymentsEngineClient::new(&env, &env.register(PaymentsEngine, ()));
    let err = client
        .try_genesis(&Bytes::from_slice(&env, b"not a config"))
        .unwrap_err();
    assert_eq!(
        err,
        Ok(soroban_sdk::Error::from_contract_error(
            EngineError::BadConfig as u32
        ))
    );
}
