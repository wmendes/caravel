//! Signed `/v1/sign` requests (DEC-095, OQ-009): the sequencer signs each
//! call with its own key, and a validator that knows that key refuses any
//! call it didn't sign, or signed too long ago. Not consensus: it only
//! decides who may ask a validator to sign.
//!
//! The signature is ed25519 over `"caravel/v1/sign" ‖ time ‖ sha256(body)`,
//! `time` as Unix seconds (u64, big-endian), sent as two headers.

use caravel_runtime::checkpoint::sha256;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

/// Unix seconds when the request was signed.
pub const TIME_HEADER: &str = "x-caravel-time";
/// The signature, hex.
pub const SIGNATURE_HEADER: &str = "x-caravel-signature";
/// How far a request's time may be from the validator's clock.
pub const WINDOW_SECS: u64 = 60;

const DOMAIN: &[u8] = b"caravel/v1/sign";

fn message(time: u64, body: &[u8]) -> Vec<u8> {
    let mut m = DOMAIN.to_vec();
    m.extend_from_slice(&time.to_be_bytes());
    m.extend_from_slice(&sha256(body));
    m
}

/// The two headers for `body`, signed at `time`.
pub fn sign(key: &SigningKey, time: u64, body: &[u8]) -> [(&'static str, String); 2] {
    let sig = key.sign(&message(time, body)).to_bytes();
    [
        (TIME_HEADER, time.to_string()),
        (
            SIGNATURE_HEADER,
            sig.iter().map(|b| format!("{b:02x}")).collect(),
        ),
    ]
}

/// Checks a request's headers against the sequencer's key, at `now`.
pub fn verify(
    key: &[u8; 32],
    time: Option<&str>,
    signature: Option<&str>,
    body: &[u8],
    now: u64,
) -> Result<(), &'static str> {
    let (Some(time), Some(signature)) = (time, signature) else {
        return Err("the request is not signed by the sequencer");
    };
    let time: u64 = time.parse().map_err(|_| "a bad request time")?;
    if time.abs_diff(now) > WINDOW_SECS {
        return Err("the request was signed too long ago, or the clocks disagree");
    }
    let sig: [u8; 64] = caravel_runtime::sequencer::unhex(signature)
        .and_then(|s| s.try_into().ok())
        .ok_or("a bad request signature")?;
    let key = VerifyingKey::from_bytes(key).map_err(|_| "a bad sequencer key")?;
    key.verify_strict(&message(time, body), &Signature::from_bytes(&sig))
        .map_err(|_| "the request's signature is not the sequencer's")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_requests() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let public = key.verifying_key().to_bytes();
        let body = br#"{"header":"00","batch":"00"}"#;
        let [(_, t), (_, s)] = sign(&key, 1000, body);
        assert_eq!(verify(&public, Some(&t), Some(&s), body, 1000), Ok(()));
        assert_eq!(verify(&public, Some(&t), Some(&s), body, 1060), Ok(()));
        // Too old, or from the future.
        assert!(verify(&public, Some(&t), Some(&s), body, 1061).is_err());
        assert!(verify(&public, Some(&t), Some(&s), body, 939).is_err());
        // Another body, another time, another key, or nothing.
        assert!(verify(&public, Some(&t), Some(&s), b"{}", 1000).is_err());
        assert!(verify(&public, Some("1001"), Some(&s), body, 1000).is_err());
        let other = SigningKey::from_bytes(&[8; 32]).verifying_key().to_bytes();
        assert!(verify(&other, Some(&t), Some(&s), body, 1000).is_err());
        assert!(verify(&public, None, None, body, 1000).is_err());
        assert!(verify(&public, Some(&t), Some("zz"), body, 1000).is_err());
    }
}
