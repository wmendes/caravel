//! The host crypto an app engine needs, as the perps engine has it (spec §8.4).

/// A signature failed verification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidSignature;

pub trait Crypto {
    fn sha256(&self, data: &[u8]) -> [u8; 32];

    /// MUST trap/panic on an invalid signature or key, like Soroban's
    /// `ed25519_verify` (spec §8.4).
    fn ed25519_verify(&self, public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]);

    /// The pipeline calls this. The default traps through [`Crypto::ed25519_verify`].
    /// A diagnostic implementation returns `Err`, so the sequencer can find
    /// the offending entry (spec §14.1).
    fn check_ed25519(
        &self,
        public_key: &[u8; 32],
        message: &[u8],
        signature: &[u8; 64],
    ) -> Result<(), InvalidSignature> {
        self.ed25519_verify(public_key, message, signature);
        Ok(())
    }
}

/// Adapts [`Crypto`] to the Merkle hasher.
pub(crate) struct Hasher<'a, C: Crypto>(pub &'a C);

impl<C: Crypto> caravel_core::merkle::Sha256 for Hasher<'_, C> {
    fn hash(&self, data: &[u8]) -> [u8; 32] {
        self.0.sha256(data)
    }
}

/// Native crypto for tests, the sequencer's pre-validation and diagnosis.
/// Verification matches the host: `VerifyingKey::from_bytes`, then `verify_strict`.
#[cfg(feature = "native")]
pub mod native {
    use ed25519_dalek::{Signature, VerifyingKey};
    use sha2::Digest;

    use super::{Crypto, InvalidSignature};

    pub fn verify_strict(public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
        VerifyingKey::from_bytes(public_key).is_ok_and(|k| {
            k.verify_strict(message, &Signature::from_bytes(signature))
                .is_ok()
        })
    }

    fn sha256(data: &[u8]) -> [u8; 32] {
        sha2::Sha256::digest(data).into()
    }

    /// Panics on an invalid signature, as the Wasm path traps.
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

    /// Returns `BAD_SIGNATURE` for the entry instead of panicking.
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
}
