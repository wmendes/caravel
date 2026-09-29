//! Native crypto for tests, sequencer pre-validation and the diagnostic re-run.
//!
//! Verification matches `soroban-env-host 28.0.2` exactly (spec §8.4): the key
//! is parsed with `VerifyingKey::from_bytes` and checked with `verify_strict`.
//! An invalid key or signature panics, as the host traps.

use ed25519_dalek::{Signature, VerifyingKey};
use sha2::Digest;

use crate::{Crypto, InvalidSignature};

/// `verify_strict` over a parsed key, as the host does.
pub fn verify_strict(public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    VerifyingKey::from_bytes(public_key).is_ok_and(|k| {
        k.verify_strict(message, &Signature::from_bytes(signature))
            .is_ok()
    })
}

fn sha256(data: &[u8]) -> [u8; 32] {
    sha2::Sha256::digest(data).into()
}

/// Panics on an invalid signature, like the Wasm path traps.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeCrypto;

impl Crypto for NativeCrypto {
    fn sha256(&self, data: &[u8]) -> [u8; 32] {
        sha256(data)
    }

    fn ed25519_verify(&self, public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]) {
        assert!(
            verify_strict(public_key, message, signature),
            "ed25519 verification failed"
        );
    }
}

/// Returns `Fatal { code: BAD_SIGNATURE, entry_index }` instead of panicking, so
/// the sequencer can find and quarantine the offending entry (spec §14.1).
#[derive(Clone, Copy, Debug, Default)]
pub struct DiagnosticCrypto;

impl Crypto for DiagnosticCrypto {
    fn sha256(&self, data: &[u8]) -> [u8; 32] {
        sha256(data)
    }

    fn ed25519_verify(&self, public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]) {
        assert!(
            verify_strict(public_key, message, signature),
            "ed25519 verification failed"
        );
    }

    fn check_ed25519(
        &self,
        public_key: &[u8; 32],
        message: &[u8],
        signature: &[u8; 64],
    ) -> Result<(), InvalidSignature> {
        if verify_strict(public_key, message, signature) {
            Ok(())
        } else {
            Err(InvalidSignature)
        }
    }
}
