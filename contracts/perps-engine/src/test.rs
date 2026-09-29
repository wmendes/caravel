use super::{PerpsEngine, PerpsEngineClient};
use soroban_sdk::{BytesN, Env};

/// `version()` is the rules identity: sha256("CARAVEL/ENGINE/V1" || "0.1.0"),
/// computed independently with `shasum -a 256`.
#[test]
fn version_is_rules_identity() {
    let env = Env::default();
    let id = env.register(PerpsEngine, ());
    let client = PerpsEngineClient::new(&env, &id);
    let expected: [u8; 32] = [
        0x4b, 0x8b, 0x18, 0xf3, 0xdf, 0xb0, 0x04, 0x52, 0x21, 0x82, 0x88, 0xbf, 0xd0, 0x6f, 0x24,
        0x3d, 0xd5, 0xc7, 0x17, 0x66, 0x7a, 0x14, 0x4f, 0xee, 0xad, 0x79, 0x52, 0x2e, 0x48, 0x17,
        0xb0, 0x99,
    ];
    assert_eq!(client.version(), BytesN::from_array(&env, &expected));
}
