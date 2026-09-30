//! Receipts: informational per-entry results and events (spec §11.10).
//!
//! Receipts are part of the parity gate (INV-P5) but are not hashed into
//! checkpoints. Changing this format needs a `CVRCPT0x` bump, not a state bump.

use alloc::vec::Vec;

use crate::codec::{DecodeError, EncodeError, Reader, Writer};
use crate::codes;
use crate::tx::Side;

pub const RECEIPTS_MAGIC: &[u8; 8] = b"CVRCPT01";
/// Pseudo entry for block-start events (funding), emitted before INBOX entries.
pub const PSEUDO_BLOCK_START: u32 = 0xFFFF_FFF0;
/// Pseudo entry for block-end events (liquidations, backstop flag, commitment).
pub const PSEUDO_BLOCK_END: u32 = 0xFFFF_FFF1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelReason {
    User = 0,
    SelfTrade = 1,
    IocRemainder = 2,
    Liquidation = 3,
    ForcedWithdrawal = 4,
}

impl CancelReason {
    pub fn from_u8(v: u8) -> Result<Self, DecodeError> {
        Ok(match v {
            0 => Self::User,
            1 => Self::SelfTrade,
            2 => Self::IocRemainder,
            3 => Self::Liquidation,
            4 => Self::ForcedWithdrawal,
            _ => return Err(DecodeError::BadEnum),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepositOutcome {
    Credited = 0,
    Created = 1,
    Bounced = 2,
}

impl DepositOutcome {
    pub fn from_u8(v: u8) -> Result<Self, DecodeError> {
        Ok(match v {
            0 => Self::Credited,
            1 => Self::Created,
            2 => Self::Bounced,
            _ => return Err(DecodeError::BadEnum),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Fill {
        market: u16,
        maker_order_id: u64,
        maker_idx: u32,
        taker_idx: u32,
        price: i64,
        lots: i64,
        taker_side: Side,
    },
    OrderRested {
        market: u16,
        order_id: u64,
        account_idx: u32,
        side: Side,
        price: i64,
        lots: i64,
    },
    OrderCanceled {
        market: u16,
        order_id: u64,
        reason: CancelReason,
    },
    Funding {
        market: u16,
        rate_ppm: i32,
        fpl: i64,
    },
    Liquidation {
        account_idx: u32,
        fee: i128,
        deficit: i128,
    },
    Deposit {
        key: [u8; 32],
        amount: i128,
        outcome: DepositOutcome,
    },
    ForcedWithdrawalProcessed {
        key: [u8; 32],
        amount: i128,
    },
    Oracle {
        market: u16,
        price: i64,
        accepted: bool,
    },
    BackstopDeficit {
        active: bool,
        backstop_equity: i128,
    },
    Commitment {
        seq: u64,
        withdrawals_total: i128,
        escape_total: i128,
    },
}

impl Event {
    pub fn type_id(&self) -> u8 {
        match self {
            Self::Fill { .. } => 1,
            Self::OrderRested { .. } => 2,
            Self::OrderCanceled { .. } => 3,
            Self::Funding { .. } => 4,
            Self::Liquidation { .. } => 5,
            Self::Deposit { .. } => 6,
            Self::ForcedWithdrawalProcessed { .. } => 7,
            Self::Oracle { .. } => 8,
            Self::BackstopDeficit { .. } => 9,
            Self::Commitment { .. } => 10,
        }
    }

    /// Fixed field length for an event type.
    pub fn fields_len(type_id: u8) -> Option<u16> {
        Some(match type_id {
            1 => 35,
            2 => 31,
            3 => 11,
            4 => 14,
            5 => 36,
            6 => 49,
            7 => 48,
            8 => 11,
            9 => 17,
            10 => 40,
            _ => return None,
        })
    }

    fn encode_to(&self, w: &mut Writer) {
        let t = self.type_id();
        w.u8(t);
        w.u16(Self::fields_len(t).unwrap_or(0));
        match *self {
            Self::Fill {
                market,
                maker_order_id,
                maker_idx,
                taker_idx,
                price,
                lots,
                taker_side,
            } => {
                w.u16(market);
                w.u64(maker_order_id);
                w.u32(maker_idx);
                w.u32(taker_idx);
                w.i64(price);
                w.i64(lots);
                w.u8(taker_side as u8);
            }
            Self::OrderRested {
                market,
                order_id,
                account_idx,
                side,
                price,
                lots,
            } => {
                w.u16(market);
                w.u64(order_id);
                w.u32(account_idx);
                w.u8(side as u8);
                w.i64(price);
                w.i64(lots);
            }
            Self::OrderCanceled {
                market,
                order_id,
                reason,
            } => {
                w.u16(market);
                w.u64(order_id);
                w.u8(reason as u8);
            }
            Self::Funding {
                market,
                rate_ppm,
                fpl,
            } => {
                w.u16(market);
                w.i32(rate_ppm);
                w.i64(fpl);
            }
            Self::Liquidation {
                account_idx,
                fee,
                deficit,
            } => {
                w.u32(account_idx);
                w.i128(fee);
                w.i128(deficit);
            }
            Self::Deposit {
                key,
                amount,
                outcome,
            } => {
                w.bytes(&key);
                w.i128(amount);
                w.u8(outcome as u8);
            }
            Self::ForcedWithdrawalProcessed { key, amount } => {
                w.bytes(&key);
                w.i128(amount);
            }
            Self::Oracle {
                market,
                price,
                accepted,
            } => {
                w.u16(market);
                w.i64(price);
                w.bool(accepted);
            }
            Self::BackstopDeficit {
                active,
                backstop_equity,
            } => {
                w.bool(active);
                w.i128(backstop_equity);
            }
            Self::Commitment {
                seq,
                withdrawals_total,
                escape_total,
            } => {
                w.u64(seq);
                w.i128(withdrawals_total);
                w.i128(escape_total);
            }
        }
    }

    fn decode_from(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let t = r.u8()?;
        let want = Self::fields_len(t).ok_or(DecodeError::BadEnum)?;
        if r.u16()? != want {
            return Err(DecodeError::BadLength);
        }
        let mut f = Reader::new(r.take(usize::from(want))?);
        let e = match t {
            1 => Self::Fill {
                market: f.u16()?,
                maker_order_id: f.u64()?,
                maker_idx: f.u32()?,
                taker_idx: f.u32()?,
                price: f.i64()?,
                lots: f.i64()?,
                taker_side: Side::from_u8(f.u8()?)?,
            },
            2 => Self::OrderRested {
                market: f.u16()?,
                order_id: f.u64()?,
                account_idx: f.u32()?,
                side: Side::from_u8(f.u8()?)?,
                price: f.i64()?,
                lots: f.i64()?,
            },
            3 => Self::OrderCanceled {
                market: f.u16()?,
                order_id: f.u64()?,
                reason: CancelReason::from_u8(f.u8()?)?,
            },
            4 => Self::Funding {
                market: f.u16()?,
                rate_ppm: f.i32()?,
                fpl: f.i64()?,
            },
            5 => Self::Liquidation {
                account_idx: f.u32()?,
                fee: f.i128()?,
                deficit: f.i128()?,
            },
            6 => Self::Deposit {
                key: f.array()?,
                amount: f.i128()?,
                outcome: DepositOutcome::from_u8(f.u8()?)?,
            },
            7 => Self::ForcedWithdrawalProcessed {
                key: f.array()?,
                amount: f.i128()?,
            },
            8 => Self::Oracle {
                market: f.u16()?,
                price: f.i64()?,
                accepted: f.bool()?,
            },
            9 => Self::BackstopDeficit {
                active: f.bool()?,
                backstop_equity: f.i128()?,
            },
            10 => Self::Commitment {
                seq: f.u64()?,
                withdrawals_total: f.i128()?,
                escape_total: f.i128()?,
            },
            _ => return Err(DecodeError::BadEnum),
        };
        f.finish()?;
        Ok(e)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    /// Index of the block entry, or [`PSEUDO_BLOCK_START`] / [`PSEUDO_BLOCK_END`].
    pub entry_index: u32,
    /// A reason code from [`crate::codes`]; `codes::OK` means the entry succeeded.
    pub code: u16,
    pub events: Vec<Event>,
}

impl Receipt {
    pub fn rejected(&self) -> bool {
        self.code != codes::OK
    }
}

/// The receipts for one block: `CVRCPT01 · count u32 · receipts`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Receipts {
    pub receipts: Vec<Receipt>,
}

impl Receipts {
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let mut w = Writer::new();
        w.bytes(RECEIPTS_MAGIC);
        w.count_u32(self.receipts.len())?;
        for rc in &self.receipts {
            w.u32(rc.entry_index);
            w.u8(u8::from(rc.rejected()));
            w.u16(rc.code);
            w.count_u16(rc.events.len())?;
            for e in &rc.events {
                e.encode_to(&mut w);
            }
        }
        Ok(w.into_vec())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        r.magic(RECEIPTS_MAGIC)?;
        let mut receipts = Vec::new();
        for _ in 0..r.u32()? {
            let entry_index = r.u32()?;
            let rejected = match r.u8()? {
                0 => false,
                1 => true,
                _ => return Err(DecodeError::BadEnum),
            };
            let code = r.u16()?;
            if !codes::is_known(code) {
                return Err(DecodeError::BadEnum);
            }
            if rejected != (code != codes::OK) {
                return Err(DecodeError::Inconsistent);
            }
            let mut events = Vec::new();
            for _ in 0..r.u16()? {
                events.push(Event::decode_from(&mut r)?);
            }
            receipts.push(Receipt {
                entry_index,
                code,
                events,
            });
        }
        r.finish()?;
        Ok(Self { receipts })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn every_event() -> Vec<Event> {
        alloc::vec![
            Event::Fill {
                market: 1,
                maker_order_id: 2,
                maker_idx: 3,
                taker_idx: 4,
                price: 5,
                lots: 6,
                taker_side: Side::Sell
            },
            Event::OrderRested {
                market: 1,
                order_id: 7,
                account_idx: 3,
                side: Side::Buy,
                price: 8,
                lots: 9
            },
            Event::OrderCanceled {
                market: 2,
                order_id: 7,
                reason: CancelReason::ForcedWithdrawal
            },
            Event::Funding {
                market: 3,
                rate_ppm: -500,
                fpl: -12
            },
            Event::Liquidation {
                account_idx: 5,
                fee: 100,
                deficit: -3
            },
            Event::Deposit {
                key: [1; 32],
                amount: 10,
                outcome: DepositOutcome::Bounced
            },
            Event::ForcedWithdrawalProcessed {
                key: [2; 32],
                amount: 11
            },
            Event::Oracle {
                market: 1,
                price: 12,
                accepted: false
            },
            Event::BackstopDeficit {
                active: true,
                backstop_equity: -13
            },
            Event::Commitment {
                seq: 1,
                withdrawals_total: 14,
                escape_total: 15
            },
        ]
    }

    pub(crate) fn sample_receipts() -> Receipts {
        Receipts {
            receipts: alloc::vec![
                Receipt {
                    entry_index: PSEUDO_BLOCK_START,
                    code: codes::OK,
                    events: every_event()[3..4].to_vec()
                },
                Receipt {
                    entry_index: 0,
                    code: codes::OK,
                    events: every_event()
                },
                Receipt {
                    entry_index: 1,
                    code: codes::BAD_NONCE,
                    events: Vec::new()
                },
                Receipt {
                    entry_index: PSEUDO_BLOCK_END,
                    code: codes::OK,
                    events: every_event()[8..].to_vec()
                },
            ],
        }
    }

    #[test]
    fn every_event_has_its_fixed_length_and_round_trips() {
        for e in every_event() {
            let mut w = Writer::new();
            e.encode_to(&mut w);
            let bytes = w.into_vec();
            assert_eq!(
                bytes.len(),
                3 + usize::from(Event::fields_len(e.type_id()).unwrap())
            );
            assert_eq!(Event::decode_from(&mut Reader::new(&bytes)), Ok(e));
        }
    }

    #[test]
    fn receipts_round_trip() {
        let rs = sample_receipts();
        let bytes = rs.encode().unwrap();
        assert_eq!(&bytes[..8], b"CVRCPT01");
        assert_eq!(Receipts::decode(&bytes), Ok(rs));
    }

    #[test]
    fn strict_decoding() {
        let bytes = Receipts {
            receipts: alloc::vec![Receipt {
                entry_index: 0,
                code: codes::BAD_NONCE,
                events: Vec::new()
            }],
        }
        .encode()
        .unwrap();
        // Layout: magic 8, count 4, entry_index 4, status at 16, code at 17..19.
        let mut ok_status = bytes.clone();
        ok_status[16] = 0;
        assert_eq!(Receipts::decode(&ok_status), Err(DecodeError::Inconsistent));
        let mut reserved = bytes.clone();
        reserved[17..19].copy_from_slice(&22u16.to_le_bytes());
        assert_eq!(Receipts::decode(&reserved), Err(DecodeError::BadEnum));
        let mut status2 = bytes.clone();
        status2[16] = 2;
        assert_eq!(Receipts::decode(&status2), Err(DecodeError::BadEnum));

        let mut w = Writer::new();
        Event::Oracle {
            market: 1,
            price: 2,
            accepted: true,
        }
        .encode_to(&mut w);
        let mut ev = w.into_vec();
        ev[1] = 12; // wrong fields length
        assert_eq!(
            Event::decode_from(&mut Reader::new(&ev)),
            Err(DecodeError::BadLength)
        );
        assert_eq!(
            Event::decode_from(&mut Reader::new(&[11, 0, 0])),
            Err(DecodeError::BadEnum)
        );
    }
}
