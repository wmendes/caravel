//! Engine scenarios (spec §19.3) on the native path. Each scenario asserts its
//! own behaviour; here we also check its final state hash against
//! `test-vectors/scenarios.json`.

use caravel_testkit::scenarios::{run, vector_file};
use caravel_testkit::NativeExecutor;

fn committed() -> String {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-vectors/scenarios.json");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing {}: run `cargo gen-vectors`", path.display()))
}

/// The committed final hash for `name`.
fn committed_hash(name: &str) -> String {
    let file = committed();
    let at = file
        .find(&format!("\"name\": \"{name}\""))
        .unwrap_or_else(|| panic!("{name} missing from scenarios.json"));
    let rest = &file[at..];
    let h = rest.find("\"hash\": \"").expect("hash field") + 9;
    rest[h..h + 64].to_string()
}

macro_rules! scenarios {
    ($($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                let out = run(stringify!($name), &NativeExecutor);
                let last = out.state_hashes.last().expect("a lane ran");
                assert_eq!(caravel_types::vectors::hex(last), committed_hash(stringify!($name)), "final state hash changed: run `cargo gen-vectors` if intended");
            }
        )*
    };
}

scenarios!(
    deposit_create_account,
    limit_rest_and_cancel,
    cross_and_fill_partial,
    ioc_remainder_canceled,
    post_only_rejected_when_crossing,
    self_trade_prevention,
    margin_reject_worst_case,
    flip_position,
    funding_zero_sum,
    liquidation_to_backstop,
    backstop_deficit_flag,
    withdraw_then_checkpoint,
    forced_withdrawal_cancels_orders,
    session_key_permissions,
    nonce_rules,
    oracle_rules,
    fatal_cases,
    withdraw_liquidity_and_cash,
    account_slot_reuse,
    oracle_breaker_widens,
);

#[test]
fn committed_scenarios_are_current() {
    assert!(
        committed() == vector_file(),
        "scenarios.json is stale: run `cargo gen-vectors`"
    );
}

#[test]
fn every_listed_scenario_has_a_test() {
    assert_eq!(caravel_testkit::scenarios::NAMES.len(), 20);
}
