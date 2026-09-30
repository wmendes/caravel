//! `LaneTxV1`, the user transaction (spec §9.2), and its bodies (spec §9.3).

use alloc::vec::Vec;

use crate::codec::{DecodeError, Reader, Writer};
use crate::tags::{SEP53_PREFIX, SEP53_TX_PREFIX, TAG_TX};

pub const LANE_TX_VERSION: u8 = 1;
/// Bytes before the body: version through `body_len`.
pub const TX_HEADER_LEN: usize = 117;
pub const SIGNATURE_LEN: usize = 64;
/// Length of the SEP-53 message for a lane transaction: `"Caravel lane tx " || hex(tx_hash)`.
pub const SEP53_TX_MESSAGE_LEN: usize = 80;

/// Session-key permission bits (spec §9.3). Other bits MUST be 0.
pub const PERM_TRADE: u8 = 0x01;
pub const PERM_CANCEL: u8 = 0x02;
pub const PERM_ALL: u8 = PERM_TRADE | PERM_CANCEL;

/// `CANCEL_ALL` market id meaning every market.
pub const ALL_MARKETS: u16 = 0xFFFF;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TxKind {
    PlaceOrder = 1,
    CancelOrder = 2,
    CancelAll = 3,
    Withdraw = 4,
    AddSessionKey = 5,
    RevokeSessionKey = 6,
}

impl TxKind {
    pub fn from_u8(v: u8) -> Result<Self, DecodeError> {
        Ok(match v {
            1 => Self::PlaceOrder,
            2 => Self::CancelOrder,
            3 => Self::CancelAll,
            4 => Self::Withdraw,
            5 => Self::AddSessionKey,
            6 => Self::RevokeSessionKey,
            _ => return Err(DecodeError::BadEnum),
        })
    }

    /// The fixed body size for this kind (spec §9.3).
    pub fn body_len(self) -> u16 {
        match self {
            Self::PlaceOrder => 29,
            Self::CancelOrder => 10,
            Self::CancelAll => 2,
            Self::Withdraw => 16,
            Self::AddSessionKey => 41,
            Self::RevokeSessionKey => 32,
        }
    }

    /// The session-key permission this kind needs, or `None` if only the owner may sign it.
    pub fn session_permission(self) -> Option<u8> {
        match self {
            Self::PlaceOrder => Some(PERM_TRADE),
            Self::CancelOrder | Self::CancelAll => Some(PERM_CANCEL),
            Self::Withdraw | Self::AddSessionKey | Self::RevokeSessionKey => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::PlaceOrder => "PLACE_ORDER",
            Self::CancelOrder => "CANCEL_ORDER",
            Self::CancelAll => "CANCEL_ALL",
            Self::Withdraw => "WITHDRAW",
            Self::AddSessionKey => "ADD_SESSION_KEY",
            Self::RevokeSessionKey => "REVOKE_SESSION_KEY",
        }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Buy = 0,
    Sell = 1,
}

impl Side {
    pub fn from_u8(v: u8) -> Result<Self, DecodeError> {
        match v {
            0 => Ok(Self::Buy),
            1 => Ok(Self::Sell),
            _ => Err(DecodeError::BadEnum),
        }
    }

    pub fn opposite(self) -> Self {
        match self {
            Self::Buy => Self::Sell,
            Self::Sell => Self::Buy,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tif {
    Gtc = 0,
    Ioc = 1,
    PostOnly = 2,
}

impl Tif {
    pub fn from_u8(v: u8) -> Result<Self, DecodeError> {
        match v {
            0 => Ok(Self::Gtc),
            1 => Ok(Self::Ioc),
            2 => Ok(Self::PostOnly),
            _ => Err(DecodeError::BadEnum),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaceOrder {
    pub market_id: u16,
    pub side: Side,
    pub tif: Tif,
    pub reduce_only: bool,
    /// USDC stroops per lot (spec §11.1).
    pub price: i64,
    pub lots: i64,
    pub client_order_id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxBody {
    PlaceOrder(PlaceOrder),
    CancelOrder {
        market_id: u16,
        order_id: u64,
    },
    /// `market_id == ALL_MARKETS` cancels on every market.
    CancelAll {
        market_id: u16,
    },
    Withdraw {
        amount: i128,
    },
    /// `permissions` is not checked here: bad bits are a rejection (§11.3.3), not a decode error.
    AddSessionKey {
        session_key: [u8; 32],
        expires_at_ms: u64,
        permissions: u8,
    },
    RevokeSessionKey {
        session_key: [u8; 32],
    },
}

impl TxBody {
    pub fn kind(&self) -> TxKind {
        match self {
            Self::PlaceOrder(_) => TxKind::PlaceOrder,
            Self::CancelOrder { .. } => TxKind::CancelOrder,
            Self::CancelAll { .. } => TxKind::CancelAll,
            Self::Withdraw { .. } => TxKind::Withdraw,
            Self::AddSessionKey { .. } => TxKind::AddSessionKey,
            Self::RevokeSessionKey { .. } => TxKind::RevokeSessionKey,
        }
    }

    fn encode_to(&self, w: &mut Writer) {
        match self {
            Self::PlaceOrder(o) => {
                w.u16(o.market_id);
                w.u8(o.side as u8);
                w.u8(o.tif as u8);
                w.bool(o.reduce_only);
                w.i64(o.price);
                w.i64(o.lots);
                w.u64(o.client_order_id);
            }
            Self::CancelOrder {
                market_id,
                order_id,
            } => {
                w.u16(*market_id);
                w.u64(*order_id);
            }
            Self::CancelAll { market_id } => w.u16(*market_id),
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
    }

    fn decode_from(kind: TxKind, r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(match kind {
            TxKind::PlaceOrder => Self::PlaceOrder(PlaceOrder {
                market_id: r.u16()?,
                side: Side::from_u8(r.u8()?)?,
                tif: Tif::from_u8(r.u8()?)?,
                reduce_only: r.bool()?,
                price: r.i64()?,
                lots: r.i64()?,
                client_order_id: r.u64()?,
            }),
            TxKind::CancelOrder => Self::CancelOrder {
                market_id: r.u16()?,
                order_id: r.u64()?,
            },
            TxKind::CancelAll => Self::CancelAll {
                market_id: r.u16()?,
            },
            TxKind::Withdraw => Self::Withdraw { amount: r.i128()? },
            TxKind::AddSessionKey => Self::AddSessionKey {
                session_key: r.array()?,
                expires_at_ms: r.u64()?,
                permissions: r.u8()?,
            },
            TxKind::RevokeSessionKey => Self::RevokeSessionKey {
                session_key: r.array()?,
            },
        })
    }
}

/// A user transaction (spec §9.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LaneTxV1 {
    pub lane_id: [u8; 32],
    /// The owner's raw ed25519 public key (their `G...` address).
    pub account: [u8; 32],
    /// `account`, or a registered session key.
    pub signer: [u8; 32],
    pub nonce: u64,
    pub expiry_ms: u64,
    pub sig_scheme: SigScheme,
    pub body: TxBody,
    pub signature: [u8; 64],
}

impl LaneTxV1 {
    pub fn kind(&self) -> TxKind {
        self.body.kind()
    }

    /// Length of the signed part: everything except the signature.
    pub fn signed_len(&self) -> usize {
        TX_HEADER_LEN + usize::from(self.kind().body_len())
    }

    pub fn encoded_len(&self) -> usize {
        self.signed_len() + SIGNATURE_LEN
    }

    /// `bytes[0 .. 117+N]`: everything except the signature.
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(self.encoded_len());
        self.encode_unsigned_to(&mut w);
        w.into_vec()
    }

    fn encode_unsigned_to(&self, w: &mut Writer) {
        let kind = self.kind();
        w.u8(LANE_TX_VERSION);
        w.bytes(&self.lane_id);
        w.bytes(&self.account);
        w.bytes(&self.signer);
        w.u64(self.nonce);
        w.u64(self.expiry_ms);
        w.u8(kind as u8);
        w.u8(self.sig_scheme as u8);
        w.u16(kind.body_len());
        self.body.encode_to(w);
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(self.encoded_len());
        self.encode_unsigned_to(&mut w);
        w.bytes(&self.signature);
        w.into_vec()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        if r.u8()? != LANE_TX_VERSION {
            return Err(DecodeError::BadVersion);
        }
        let lane_id = r.array()?;
        let account = r.array()?;
        let signer = r.array()?;
        let nonce = r.u64()?;
        let expiry_ms = r.u64()?;
        let kind = TxKind::from_u8(r.u8()?)?;
        let sig_scheme = SigScheme::from_u8(r.u8()?)?;
        if r.u16()? != kind.body_len() {
            return Err(DecodeError::BadLength);
        }
        let body = TxBody::decode_from(kind, &mut r)?;
        let signature = r.array()?;
        r.finish()?;
        Ok(Self {
            lane_id,
            account,
            signer,
            nonce,
            expiry_ms,
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

    fn tx(body: TxBody) -> LaneTxV1 {
        LaneTxV1 {
            lane_id: [1; 32],
            account: [2; 32],
            signer: [3; 32],
            nonce: 7,
            expiry_ms: 1_000_000,
            sig_scheme: SigScheme::RawEd25519,
            body,
            signature: [9; 64],
        }
    }

    fn all_bodies() -> [TxBody; 6] {
        [
            TxBody::PlaceOrder(PlaceOrder {
                market_id: 1,
                side: Side::Sell,
                tif: Tif::PostOnly,
                reduce_only: false,
                price: 65_000_000,
                lots: 250,
                client_order_id: 42,
            }),
            TxBody::CancelOrder {
                market_id: 2,
                order_id: 99,
            },
            TxBody::CancelAll {
                market_id: ALL_MARKETS,
            },
            TxBody::Withdraw {
                amount: 100_000_000,
            },
            TxBody::AddSessionKey {
                session_key: [4; 32],
                expires_at_ms: 5,
                permissions: PERM_ALL,
            },
            TxBody::RevokeSessionKey {
                session_key: [4; 32],
            },
        ]
    }

    #[test]
    fn round_trips_every_kind_with_exact_lengths() {
        for body in all_bodies() {
            let t = tx(body);
            let bytes = t.encode();
            assert_eq!(bytes.len(), 117 + usize::from(t.kind().body_len()) + 64);
            assert_eq!(bytes.len(), t.encoded_len());
            assert_eq!(LaneTxV1::decode(&bytes), Ok(t));
            assert_eq!(&bytes[..t.signed_len()], t.signing_bytes().as_slice());
            assert_eq!(bytes[113], t.kind() as u8);
            assert_eq!(
                u16::from_le_bytes([bytes[115], bytes[116]]),
                t.kind().body_len()
            );
        }
    }

    #[test]
    fn layout_offsets_match_spec() {
        let bytes = tx(all_bodies()[0]).encode();
        assert_eq!(bytes[0], 1);
        assert_eq!(&bytes[1..33], &[1; 32]);
        assert_eq!(&bytes[33..65], &[2; 32]);
        assert_eq!(&bytes[65..97], &[3; 32]);
        assert_eq!(u64::from_le_bytes(bytes[97..105].try_into().unwrap()), 7);
        assert_eq!(
            u64::from_le_bytes(bytes[105..113].try_into().unwrap()),
            1_000_000
        );
        assert_eq!(bytes[114], 0);
        assert_eq!(&bytes[146..], &[9; 64]);
    }

    #[test]
    fn strict_decoding() {
        let good = tx(all_bodies()[0]).encode();
        let with = |i: usize, v: u8| {
            let mut b = good.clone();
            b[i] = v;
            LaneTxV1::decode(&b)
        };
        assert_eq!(with(0, 2), Err(DecodeError::BadVersion));
        assert_eq!(with(113, 7), Err(DecodeError::BadEnum));
        assert_eq!(with(113, 0), Err(DecodeError::BadEnum));
        assert_eq!(with(114, 2), Err(DecodeError::BadEnum));
        assert_eq!(with(115, 30), Err(DecodeError::BadLength));
        assert_eq!(with(117 + 2, 2), Err(DecodeError::BadEnum)); // side
        assert_eq!(with(117 + 3, 3), Err(DecodeError::BadEnum)); // tif
        assert_eq!(with(117 + 4, 2), Err(DecodeError::BadBool)); // reduce_only
        let mut trailing = good.clone();
        trailing.push(0);
        assert_eq!(LaneTxV1::decode(&trailing), Err(DecodeError::TrailingBytes));
        assert_eq!(
            LaneTxV1::decode(&good[..good.len() - 1]),
            Err(DecodeError::UnexpectedEnd)
        );
        // Kind and body_len must agree: a CANCEL_ALL header with a PLACE_ORDER length.
        let mut wrong_kind = good.clone();
        wrong_kind[113] = TxKind::CancelAll as u8;
        assert_eq!(LaneTxV1::decode(&wrong_kind), Err(DecodeError::BadLength));
    }

    #[test]
    fn session_key_permission_bits_are_not_a_decode_error() {
        let t = tx(TxBody::AddSessionKey {
            session_key: [4; 32],
            expires_at_ms: 5,
            permissions: 0xFF,
        });
        assert_eq!(LaneTxV1::decode(&t.encode()), Ok(t));
    }

    #[test]
    fn sep53_message_is_80_ascii_bytes() {
        let mut h = [0u8; 32];
        h[0] = 0xAB;
        h[31] = 0x01;
        let m = sep53_tx_message(&h);
        assert_eq!(m.len(), 80);
        assert!(m.starts_with(b"Caravel lane tx ab00"));
        assert!(m.ends_with(b"0001"));
        assert!(m.iter().all(u8::is_ascii));
        let p = sep53_preimage(&m);
        assert!(p.starts_with(b"Stellar Signed Message:\n"));
        assert_eq!(p.len(), 24 + 80);
    }

    #[test]
    fn tx_hash_preimage_binds_config_hash() {
        let t = tx(all_bodies()[3]);
        let p = t.tx_hash_preimage(&[0xCC; 32]);
        assert_eq!(&p[..13], b"CARAVEL/TX/V1");
        assert_eq!(&p[13..45], &[0xCC; 32]);
        assert_eq!(&p[45..], t.signing_bytes().as_slice());
    }

    #[test]
    fn permissions_follow_the_signer_table() {
        assert_eq!(TxKind::PlaceOrder.session_permission(), Some(PERM_TRADE));
        assert_eq!(TxKind::CancelAll.session_permission(), Some(PERM_CANCEL));
        assert_eq!(TxKind::Withdraw.session_permission(), None);
        assert_eq!(TxKind::AddSessionKey.session_permission(), None);
    }
}
