//! The native Crypto verifies exactly like soroban-env-host 28.0.2 (spec §8.4):
//! every case in test-vectors/signatures.json gets its `verify_strict` result.

use caravel_perps::native::{verify_strict, DiagnosticCrypto, NativeCrypto};
use caravel_perps::Crypto;

fn bytes<const N: usize>(v: &serde_json::Value) -> [u8; N] {
    let s = v.as_str().expect("hex string");
    let raw: Vec<u8> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect();
    raw.try_into().expect("length")
}

#[test]
fn native_verification_matches_verify_strict_on_every_edge_case() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-vectors/signatures.json");
    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let cases = file["vectors"].as_array().unwrap();
    assert_eq!(cases.len(), 3);
    for case in cases {
        let f = &case["fields"];
        let (key, msg, sig) = (
            bytes::<32>(&f["public_key"]),
            bytes::<32>(&f["message"]),
            bytes::<64>(&f["signature"]),
        );
        let want = f["verify_strict"].as_bool().unwrap();
        assert_eq!(verify_strict(&key, &msg, &sig), want, "{}", case["name"]);
        // The diagnostic impl reports instead of panicking; the plain one panics.
        assert_eq!(
            DiagnosticCrypto.check_ed25519(&key, &msg, &sig).is_ok(),
            want,
            "{}",
            case["name"]
        );
        let traps =
            std::panic::catch_unwind(|| NativeCrypto.ed25519_verify(&key, &msg, &sig)).is_err();
        assert_eq!(traps, !want, "{}", case["name"]);
    }
    // The small-order case is exactly where lax verification would disagree.
    let small = cases
        .iter()
        .find(|c| c["name"] == "small_order_key_identity")
        .unwrap();
    assert_eq!(small["fields"]["verify"], true);
    assert_eq!(small["fields"]["verify_strict"], false);
}
