use super::{EngineError, PerpsEngine, PerpsEngineClient};
use caravel_perps::native::NativeCrypto;
use caravel_testkit::lane::seeds::{A, B};
use caravel_testkit::lane::{config, BTC, BTC_PRICE, ETH, ETH_PRICE, TICK, USDC, XLM, XLM_PRICE};
use caravel_testkit::Lane;
use caravel_types::step::StepEnvelope;
use caravel_types::tx::{Side, Tif};
use soroban_sdk::{Bytes, BytesN, Env};

fn client(env: &Env) -> PerpsEngineClient<'_> {
    env.cost_estimate().budget().reset_unlimited();
    let id = env.register(PerpsEngine, ());
    PerpsEngineClient::new(env, &id)
}

/// `version()` is the rules identity: sha256("CARAVEL/ENGINE/V1" || "0.1.0"),
/// computed independently with `shasum -a 256`.
#[test]
fn version_is_rules_identity() {
    let env = Env::default();
    let expected: [u8; 32] = [
        0x4b, 0x8b, 0x18, 0xf3, 0xdf, 0xb0, 0x04, 0x52, 0x21, 0x82, 0x88, 0xbf, 0xd0, 0x6f, 0x24,
        0x3d, 0xd5, 0xc7, 0x17, 0x66, 0x7a, 0x14, 0x4f, 0xee, 0xad, 0x79, 0x52, 0x2e, 0x48, 0x17,
        0xb0, 0x99,
    ];
    assert_eq!(client(&env).version(), BytesN::from_array(&env, &expected));
}

#[test]
fn genesis_matches_the_native_engine() {
    let env = Env::default();
    let cfg = config().encode().unwrap();
    let native = caravel_perps::genesis(&cfg, &NativeCrypto).unwrap();
    assert_eq!(
        client(&env)
            .genesis(&Bytes::from_slice(&env, &cfg))
            .to_alloc_vec(),
        native
    );
}

/// Replays a lane with deposits, oracle prices, fills, a withdrawal and a
/// checkpoint through the contract, where the Soroban host does every hash and
/// signature check. State and receipts must equal the native engine's.
#[test]
fn step_matches_the_native_engine_block_for_block() {
    let mut lane = Lane::native(config());
    lane.deposit(A, 1000 * USDC);
    lane.deposit(B, 1000 * USDC);
    lane.oracle(BTC, BTC_PRICE);
    lane.oracle(ETH, ETH_PRICE);
    lane.oracle(XLM, XLM_PRICE);
    lane.block();
    lane.order(B, BTC, Side::Sell, Tif::Gtc, BTC_PRICE, 10);
    lane.order(A, BTC, Side::Buy, Tif::Gtc, BTC_PRICE - TICK, 4);
    lane.block();
    lane.order(A, BTC, Side::Buy, Tif::Ioc, BTC_PRICE, 6);
    lane.withdraw(B, 5 * USDC);
    lane.checkpoint();

    let env = Env::default();
    let c = client(&env);
    let mut state = c
        .genesis(&Bytes::from_slice(&env, &config().encode().unwrap()))
        .to_alloc_vec();
    for rec in &lane.records {
        let native = caravel_perps::step(&state, &rec.input, &NativeCrypto).unwrap();
        let out = c
            .step(
                &Bytes::from_slice(&env, &state),
                &Bytes::from_slice(&env, &rec.input),
            )
            .to_alloc_vec();
        let envelope = StepEnvelope::decode(&out).expect("CVSTEP01 envelope");
        assert_eq!(envelope.state, native.state);
        assert_eq!(envelope.receipts, native.receipts);
        assert_eq!(
            caravel_types::vectors::sha256(&envelope.state),
            rec.state_hash_after
        );
        state = envelope.state;
    }
    assert_eq!(state, lane.state_bytes);
}

#[test]
fn fatal_codes_become_contract_errors() {
    let env = Env::default();
    let c = client(&env);
    let state = c.genesis(&Bytes::from_slice(&env, &config().encode().unwrap()));
    let err = c
        .try_step(&state, &Bytes::from_slice(&env, b"not a block"))
        .unwrap_err();
    assert_eq!(
        err,
        Ok(soroban_sdk::Error::from_contract_error(
            EngineError::BadBlockEncoding as u32
        ))
    );
    let err = c
        .try_genesis(&Bytes::from_slice(&env, b"bad config"))
        .unwrap_err();
    assert_eq!(
        err,
        Ok(soroban_sdk::Error::from_contract_error(
            EngineError::BadConfig as u32
        ))
    );
}

#[test]
fn an_invalid_signature_traps_instead_of_returning_an_error_code() {
    let mut lane = Lane::native(config());
    lane.deposit(A, 10 * USDC);
    lane.block();
    let mut tx = lane.signed(
        A,
        A,
        caravel_types::tx::SigScheme::RawEd25519,
        lane.account(A).unwrap().next_nonce,
        u64::MAX,
        caravel_types::tx::TxBody::CancelAll { market_id: 0xFFFF },
    );
    tx.signature[0] ^= 1;
    lane.push_tx(tx);
    let block = lane.build(false).encode().unwrap();

    let env = Env::default();
    let c = client(&env);
    let err = c
        .try_step(
            &Bytes::from_slice(&env, &lane.state_bytes),
            &Bytes::from_slice(&env, &block),
        )
        .unwrap_err();
    // A host error (the trap), never a contract error code.
    let is_contract_error =
        matches!(err, Ok(e) if e.is_type(soroban_sdk::xdr::ScErrorType::Contract));
    assert!(
        !is_contract_error,
        "an invalid signature must trap, got {err:?}"
    );
}
