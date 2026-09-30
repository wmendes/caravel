//! The lane transaction envelope (spec §9.2), with the body left to the app.
//!
//! The bytes are the M0 `LaneTxV1`:
//! `version u8 · lane_id · account · signer · nonce u64 · expiry_ms u64 ·
//! kind u8 · sig_scheme u8 · body_len u16 · body · signature[64]`.
//! This decoder checks the framing only. Whether a kind exists and whether
//! `body_len` is right for it is the app's check (its mempool check and its
//! engine), not the envelope's.
//!
//! Kinds (DEC-052):
//! - 4, 5 and 6 are platform-standard: `WITHDRAW`, `ADD_SESSION_KEY` and
//!   `REVOKE_SESSION_KEY`, with the M0 bodies;
//! - 7 to 15 are reserved for the platform;
//! - 1 to 3 and 16 and up belong to the app.

use alloc::vec::Vec;

use crate::codec::{DecodeError, Reader, Writer};
use crate::tags::{SEP53_PREFIX, SEP53_TX_PREFIX, TAG_TX};

pub const TX_VERSION: u8 = 1;
/// Bytes before the body: version through `body_len`.
pub const TX_HEADER_LEN: usize = 117;
pub const SIGNATURE_LEN: usize = 64;
/// Length of the SEP-53 message for a lane transaction: `"Caravel lane tx " || hex(tx_hash)`.
pub const SEP53_TX_MESSAGE_LEN: usize = 80;

/// Platform-standard transaction kinds (DEC-052).
pub mod kind {
    pub const WITHDRAW: u8 = 4;
    pub const ADD_SESSION_KEY: u8 = 5;
    pub const REVOKE_SESSION_KEY: u8 = 6;

    /// Kinds 4 to 15 are the platform's (4 to 6 defined, the rest reserved).
    pub fn is_platform(kind: u8) -> bool {
        (4..=15).contains(&kind)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigScheme {
    /// `ed25519_verify(signer, tx_hash, signature)`.
    RawEd25519 = 0,
    /// SEP-53 over `"Caravel lane tx " || hex(tx_hash)`; only valid when `signer == account`.
    Sep53 = 1,
}

impl SigScheme {
    pub fn from_u8(v: u8) -> Result<Self, DecodeError> {
        match v {
            0 => Ok(Self::RawEd25519),
            1 => Ok(Self::Sep53),
            _ => Err(DecodeError::BadEnum),
        }
    }
}

/// A lane transaction with an opaque body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxEnvelopeV1 {
    pub lane_id: [u8; 32],
    /// The owner's raw ed25519 public key (their `G...` address).
    pub account: [u8; 32],
    /// `account`, or a registered session key.
    pub signer: [u8; 32],
    pub nonce: u64,
    pub expiry_ms: u64,
    pub kind: u8,
    pub sig_scheme: SigScheme,
    /// At most `u16::MAX` bytes.
    pub body: Vec<u8>,
    pub signature: [u8; 64],
}

impl TxEnvelopeV1 {
    /// Length of the signed part: everything except the signature.
    pub fn signed_len(&self) -> usize {
        TX_HEADER_LEN + self.body.len()
    }

    pub fn encoded_len(&self) -> usize {
        self.signed_len() + SIGNATURE_LEN
    }

    fn encode_unsigned_to(&self, w: &mut Writer) {
        w.u8(TX_VERSION);
        w.bytes(&self.lane_id);
        w.bytes(&self.account);
        w.bytes(&self.signer);
        w.u64(self.nonce);
        w.u64(self.expiry_ms);
        w.u8(self.kind);
        w.u8(self.sig_scheme as u8);
        w.u16(u16::try_from(self.body.len()).unwrap_or(u16::MAX));
        w.bytes(&self.body);
    }

    /// `bytes[0 .. 117+N]`: everything except the signature.
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(self.signed_len());
        self.encode_unsigned_to(&mut w);
        w.into_vec()
    }

    /// The encoded envelope. A body longer than `u16::MAX` cannot be encoded
    /// and gives `None`.
    pub fn encode(&self) -> Option<Vec<u8>> {
        if self.body.len() > usize::from(u16::MAX) {
            return None;
        }
        let mut w = Writer::with_capacity(self.encoded_len());
        self.encode_unsigned_to(&mut w);
        w.bytes(&self.signature);
        Some(w.into_vec())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        if r.u8()? != TX_VERSION {
            return Err(DecodeError::BadVersion);
        }
        let lane_id = r.array()?;
        let account = r.array()?;
        let signer = r.array()?;
        let nonce = r.u64()?;
        let expiry_ms = r.u64()?;
        let kind = r.u8()?;
        let sig_scheme = SigScheme::from_u8(r.u8()?)?;
        let body_len = usize::from(r.u16()?);
        let body = r.take(body_len)?.to_vec();
        let signature = r.array()?;
        r.finish()?;
        Ok(Self {
            lane_id,
            account,
            signer,
            nonce,
            expiry_ms,
            kind,
            sig_scheme,
            body,
            signature,
        })
    }

    /// `TAG_TX || config_hash || bytes[0 .. 117+N]`; `tx_hash` is its SHA-256.
    pub fn tx_hash_preimage(&self, config_hash: &[u8; 32]) -> Vec<u8> {
        let mut w = Writer::with_capacity(TAG_TX.len() + 32 + self.signed_len());
        w.bytes(TAG_TX);
        w.bytes(config_hash);
        self.encode_unsigned_to(&mut w);
        w.into_vec()
    }

    /// The body of a platform-standard kind, or `None` for an app kind.
    pub fn standard_body(&self) -> Option<Result<StandardBody, DecodeError>> {
        StandardBody::decode(self.kind, &self.body)
    }
}

/// The bodies of the platform-standard kinds (the M0 bytes, spec §9.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandardBody {
    /// `amount i128` (16 bytes): moves free balance to the pending withdrawal queue.
    Withdraw { amount: i128 },
    /// `session_key[32] · expires_at_ms u64 · permissions u8` (41 bytes).
    AddSessionKey {
        session_key: [u8; 32],
        expires_at_ms: u64,
        permissions: u8,
    },
    /// `session_key[32]` (32 bytes).
    RevokeSessionKey { session_key: [u8; 32] },
}

impl StandardBody {
    pub fn kind(&self) -> u8 {
        match self {
            Self::Withdraw { .. } => kind::WITHDRAW,
            Self::AddSessionKey { .. } => kind::ADD_SESSION_KEY,
            Self::RevokeSessionKey { .. } => kind::REVOKE_SESSION_KEY,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        match self {
            Self::Withdraw { amount } => w.i128(*amount),
            Self::AddSessionKey {
                session_key,
                expires_at_ms,
                permissions,
            } => {
                w.bytes(session_key);
                w.u64(*expires_at_ms);
                w.u8(*permissions);
            }
            Self::RevokeSessionKey { session_key } => w.bytes(session_key),
        }
        w.into_vec()
    }

    /// `None` when `kind` is not a standard kind; otherwise the strict decode.
    pub fn decode(kind: u8, body: &[u8]) -> Option<Result<Self, DecodeError>> {
        let mut r = Reader::new(body);
        let parsed = match kind {
            kind::WITHDRAW => r.i128().map(|amount| Self::Withdraw { amount }),
            kind::ADD_SESSION_KEY => (|| {
                Ok(Self::AddSessionKey {
                    session_key: r.array()?,
                    expires_at_ms: r.u64()?,
                    permissions: r.u8()?,
                })
            })(),
            kind::REVOKE_SESSION_KEY => r
                .array()
                .map(|session_key| Self::RevokeSessionKey { session_key }),
            _ => return None,
        };
        Some(parsed.and_then(|b| r.finish().map(|()| b)))
    }
}

/// The SEP-53 message for a lane transaction: UTF-8 `"Caravel lane tx " || lowercase_hex(tx_hash)`.
pub fn sep53_tx_message(tx_hash: &[u8; 32]) -> [u8; SEP53_TX_MESSAGE_LEN] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = [0u8; SEP53_TX_MESSAGE_LEN];
    out[..SEP53_TX_PREFIX.len()].copy_from_slice(SEP53_TX_PREFIX);
    for (i, b) in tx_hash.iter().enumerate() {
        out[SEP53_TX_PREFIX.len() + 2 * i] = HEX[usize::from(b >> 4)];
        out[SEP53_TX_PREFIX.len() + 2 * i + 1] = HEX[usize::from(b & 0x0F)];
    }
    out
}

/// `"Stellar Signed Message:\n" || message`; its SHA-256 is what a SEP-53 wallet signs (spec §3.5).
pub fn sep53_preimage(message: &[u8]) -> Vec<u8> {
    let mut w = Writer::with_capacity(SEP53_PREFIX.len() + message.len());
    w.bytes(SEP53_PREFIX);
    w.bytes(message);
    w.into_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(kind: u8, body: Vec<u8>) -> TxEnvelopeV1 {
        TxEnvelopeV1 {
            lane_id: [1; 32],
            account: [2; 32],
            signer: [3; 32],
            nonce: 7,
            expiry_ms: 1_000_000,
            kind,
            sig_scheme: SigScheme::Sep53,
            body,
            signature: [9; 64],
        }
    }

    #[test]
    fn round_trips_any_kind_and_body() {
        for (kind, len) in [(1u8, 29usize), (4, 16), (16, 0), (200, 300)] {
            let tx = envelope(kind, (0..len).map(|i| i as u8).collect());
            let bytes = tx.encode().unwrap();
            assert_eq!(bytes.len(), TX_HEADER_LEN + len + SIGNATURE_LEN);
            assert_eq!(TxEnvelopeV1::decode(&bytes).unwrap(), tx);
            assert_eq!(
                &tx.tx_hash_preimage(&[5; 32])[TAG_TX.len() + 32..],
                &bytes[..bytes.len() - 64]
            );
        }
    }

    #[test]
    fn framing_errors_are_rejected() {
        let bytes = envelope(4, alloc::vec![0; 16]).encode().unwrap();
        let mut v = bytes.clone();
        v[0] = 2;
        assert_eq!(TxEnvelopeV1::decode(&v), Err(DecodeError::BadVersion));
        let mut v = bytes.clone();
        v[114] = 2; // sig_scheme
        assert_eq!(TxEnvelopeV1::decode(&v), Err(DecodeError::BadEnum));
        let mut v = bytes.clone();
        v.push(0);
        assert_eq!(TxEnvelopeV1::decode(&v), Err(DecodeError::TrailingBytes));
        assert_eq!(
            TxEnvelopeV1::decode(&bytes[..bytes.len() - 1]),
            Err(DecodeError::UnexpectedEnd)
        );
        let mut v = bytes;
        v[115] = 17; // body_len one too long: the signature runs short
        assert_eq!(TxEnvelopeV1::decode(&v), Err(DecodeError::UnexpectedEnd));
    }

    #[test]
    fn standard_bodies_are_strict() {
        let w = StandardBody::Withdraw { amount: 42 };
        assert_eq!(StandardBody::decode(4, &w.encode()), Some(Ok(w)));
        assert_eq!(
            StandardBody::decode(4, &[0; 15]),
            Some(Err(DecodeError::UnexpectedEnd))
        );
        assert_eq!(
            StandardBody::decode(4, &[0; 17]),
            Some(Err(DecodeError::TrailingBytes))
        );
        let a = StandardBody::AddSessionKey {
            session_key: [8; 32],
            expires_at_ms: 9,
            permissions: 3,
        };
        assert_eq!(a.encode().len(), 41);
        assert_eq!(StandardBody::decode(5, &a.encode()), Some(Ok(a)));
        let r = StandardBody::RevokeSessionKey {
            session_key: [8; 32],
        };
        assert_eq!(StandardBody::decode(6, &r.encode()), Some(Ok(r)));
        assert_eq!(StandardBody::decode(1, &[]), None);
        assert_eq!(StandardBody::decode(16, &[]), None);
        assert!(kind::is_platform(4) && kind::is_platform(15));
        assert!(!kind::is_platform(3) && !kind::is_platform(16));
    }
}
