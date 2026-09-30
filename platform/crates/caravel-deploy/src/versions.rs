//! The pins the tool needs from `versions.json`, compiled in so a release
//! binary carries the values it was tested with.

use std::sync::OnceLock;

use serde_json::Value;

fn versions() -> &'static Value {
    static V: OnceLock<Value> = OnceLock::new();
    V.get_or_init(|| {
        serde_json::from_str(include_str!("../../../../versions.json"))
            .expect("versions.json is JSON")
    })
}

fn get(path: &str) -> &'static str {
    path.split('.')
        .fold(versions(), |v, k| &v[k])
        .as_str()
        .unwrap_or_else(|| panic!("versions.json has no string at {path}"))
}

pub fn testnet_rpc() -> &'static str {
    get("testnet.rpc_url")
}

/// Circle's USDC asset contract on testnet.
pub fn testnet_usdc() -> &'static str {
    get("testnet.usdc_sac")
}

/// The settlement build new lanes deploy (DEC-033, DEC-063).
pub fn settlement_wasm() -> &'static str {
    get("artifacts.settlement_wasm_sha256")
}

/// Every settlement build a lane may run: the build of record, and lane
/// #1's, deployed before it (DEC-061).
pub fn known_settlement_builds() -> [&'static str; 2] {
    [
        settlement_wasm(),
        get("lanes.perps.settlement_deployed_wasm_sha256"),
    ]
}

pub fn stellar_cli() -> &'static str {
    get("stellar_cli")
}
