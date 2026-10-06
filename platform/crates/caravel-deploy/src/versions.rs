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

/// The Stellar quickstart image a local network runs, as `stellar container
/// start --image-tag-override` takes it: the tag of the pinned
/// `stellar/quickstart:<tag>@sha256:…` (D-03).
pub fn quickstart_tag() -> &'static str {
    let image = get("images.stellar_quickstart");
    image
        .split_once(':')
        .map(|(_, rest)| rest.split('@').next().unwrap_or(rest))
        .unwrap_or("latest")
}

pub fn testnet_rpc() -> &'static str {
    get("testnet.rpc_url")
}

/// Circle's USDC asset contract on testnet.
pub fn testnet_usdc() -> &'static str {
    get("testnet.usdc_sac")
}

/// The issuer of Circle's testnet USDC (`USDC:<issuer>`).
pub fn testnet_usdc_issuer() -> &'static str {
    get("testnet.usdc_issuer")
}

/// The settlement build new lanes deploy (DEC-033, DEC-063).
pub fn settlement_wasm() -> &'static str {
    get("artifacts.settlement_wasm_sha256")
}

/// Every settlement build a lane may run: the build of record, lane #1's,
/// deployed before it (DEC-061), the build that kept a record of every
/// checkpoint, before M0.10 (DEC-124), and M0.10's, before the exact escape
/// payout (DEC-126).
pub fn known_settlement_builds() -> [&'static str; 4] {
    [
        settlement_wasm(),
        get("lanes.perps.settlement_deployed_wasm_sha256"),
        get("artifacts.settlement_v1_wasm_sha256"),
        get("artifacts.settlement_v2_wasm_sha256"),
    ]
}

pub fn stellar_cli() -> &'static str {
    get("stellar_cli")
}
