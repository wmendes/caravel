//! Receipts (spec §11.10), for any app: the container and the platform events.
//!
//! The bytes are the M0 `CVRCPT01` receipts:
//! `CVRCPT01 · count u32 · count × (entry_index u32 · status u8 · code u16 ·
//! event_count u16 · events)`, and each event is `type u8 · len u16 · fields`.
//!
//! Only three event types are the platform's (DEC-052):
//! - 6 `DEPOSIT`;
//! - 7 `FORCED_WITHDRAWAL_PROCESSED`;
//! - 10 `COMMITMENT`.
//!
//! Every other type is the app's and stays opaque here. The container checks
//! that `status` is 0 or 1 and agrees with `code != 0`. Which codes and events
//! exist is the app's check.

use alloc::vec::Vec;

use crate::codec::{DecodeError, EncodeError, Reader, Writer};

pub const RECEIPTS_MAGIC: &[u8; 8] = b"CVRCPT01";
/// `entry_index` of the block-start pseudo-entry (events before the first entry).
pub const PSEUDO_BLOCK_START: u32 = 0xFFFF_FFF0;
/// `entry_index` of the block-end pseudo-entry (events after the last entry).
pub const PSEUDO_BLOCK_END: u32 = 0xFFFF_FFF1;

/// Platform event types.
pub mod event_type {
    pub const DEPOSIT: u8 = 6;
    pub const FORCED_WITHDRAWAL_PROCESSED: u8 = 7;
    pub const COMMITMENT: u8 = 10;
}

/// One event: its type and raw fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventV1 {
    pub type_id: u8,
    pub fields: Vec<u8>,
}

/// `outcome` of a `DEPOSIT` event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepositOutcome {
    /// Added to an existing account.
    Credited = 0,
    /// A new account was created.
    Created = 1,
    /// Refused by the lane (access, capacity or amount) and queued for withdrawal.
    Bounced = 2,
}

/// The events the platform reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlatformEvent {
    /// `key[32] · amount i128 · outcome u8` (49 bytes).
    Deposit {
        key: [u8; 32],
        amount: i128,
        outcome: DepositOutcome,
    },
    /// `key[32] · amount i128` (48 bytes): the amount actually queued.
    ForcedWithdrawalProcessed { key: [u8; 32], amount: i128 },
    /// `seq u64 · withdrawals_total i128 · escape_total i128` (40 bytes).
    Commitment {
        seq: u64,
        withdrawals_total: i128,
        escape_total: i128,
    },
}

impl PlatformEvent {
    pub fn to_event(&self) -> EventV1 {
        let mut w = Writer::new();
        let type_id = match self {
            Self::Deposit {
                key,
                amount,
                outcome,
            } => {
                w.bytes(key);
                w.i128(*amount);
                w.u8(*outcome as u8);
                event_type::DEPOSIT
            }
            Self::ForcedWithdrawalProcessed { key, amount } => {
                w.bytes(key);
                w.i128(*amount);
                event_type::FORCED_WITHDRAWAL_PROCESSED
            }
            Self::Commitment {
                seq,
                withdrawals_total,
                escape_total,
            } => {
                w.u64(*seq);
                w.i128(*withdrawals_total);
                w.i128(*escape_total);
                event_type::COMMITMENT
            }
        };
        EventV1 {
            type_id,
            fields: w.into_vec(),
        }
    }
}

impl EventV1 {
    /// The platform event, or `None` for an app event type.
    pub fn platform(&self) -> Option<Result<PlatformEvent, DecodeError>> {
        let mut r = Reader::new(&self.fields);
        let parsed = match self.type_id {
            event_type::DEPOSIT => (|| {
                let key = r.array()?;
                let amount = r.i128()?;
                let outcome = match r.u8()? {
                    0 => DepositOutcome::Credited,
                    1 => DepositOutcome::Created,
                    2 => DepositOutcome::Bounced,
                    _ => return Err(DecodeError::BadEnum),
                };
                Ok(PlatformEvent::Deposit {
                    key,
                    amount,
                    outcome,
                })
            })(),
            event_type::FORCED_WITHDRAWAL_PROCESSED => (|| {
                Ok(PlatformEvent::ForcedWithdrawalProcessed {
                    key: r.array()?,
                    amount: r.i128()?,
                })
            })(),
            event_type::COMMITMENT => (|| {
                Ok(PlatformEvent::Commitment {
                    seq: r.u64()?,
                    withdrawals_total: r.i128()?,
                    escape_total: r.i128()?,
                })
            })(),
            _ => return None,
        };
        Some(parsed.and_then(|e| r.finish().map(|()| e)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptV1 {
    /// The entry's index in the block, or a pseudo-entry.
    pub entry_index: u32,
    /// 0 is OK; anything else means the entry was rejected.
    pub code: u16,
    pub events: Vec<EventV1>,
}

impl ReceiptV1 {
    pub fn rejected(&self) -> bool {
        self.code != 0
    }
}

/// The receipts for one block.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReceiptsV1 {
    pub receipts: Vec<ReceiptV1>,
}

impl ReceiptsV1 {
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
                w.u8(e.type_id);
                w.count_u16(e.fields.len())?;
                w.bytes(&e.fields);
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
            if rejected != (code != 0) {
                return Err(DecodeError::Inconsistent);
            }
            let mut events = Vec::new();
            for _ in 0..r.u16()? {
                let type_id = r.u8()?;
                let len = usize::from(r.u16()?);
                events.push(EventV1 {
                    type_id,
                    fields: r.take(len)?.to_vec(),
                });
            }
            receipts.push(ReceiptV1 {
                entry_index,
                code,
                events,
            });
        }
        r.finish()?;
        Ok(Self { receipts })
    }

    /// The receipt of entry `index`, if the block had one for it.
    pub fn for_entry(&self, index: u32) -> Option<&ReceiptV1> {
        self.receipts.iter().find(|r| r.entry_index == index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_events_have_their_m0_lengths_and_round_trip() {
        let events = [
            PlatformEvent::Deposit {
                key: [1; 32],
                amount: 5,
                outcome: DepositOutcome::Bounced,
            },
            PlatformEvent::ForcedWithdrawalProcessed {
                key: [2; 32],
                amount: -1,
            },
            PlatformEvent::Commitment {
                seq: 3,
                withdrawals_total: 4,
                escape_total: 5,
            },
        ];
        for (e, len) in events.iter().zip([49usize, 48, 40]) {
            let raw = e.to_event();
            assert_eq!(raw.fields.len(), len);
            assert_eq!(raw.platform(), Some(Ok(*e)));
        }
        let app = EventV1 {
            type_id: 1,
            fields: alloc::vec![0; 37],
        };
        assert_eq!(app.platform(), None);
    }

    #[test]
    fn container_round_trips_and_checks_status() {
        let rs = ReceiptsV1 {
            receipts: alloc::vec![
                ReceiptV1 {
                    entry_index: 0,
                    code: 0,
                    events: alloc::vec![PlatformEvent::Deposit {
                        key: [1; 32],
                        amount: 5,
                        outcome: DepositOutcome::Created,
                    }
                    .to_event()],
                },
                ReceiptV1 {
                    entry_index: 1,
                    code: 99,
                    events: alloc::vec![EventV1 {
                        type_id: 200,
                        fields: alloc::vec![9; 3],
                    }],
                },
            ],
        };
        let bytes = rs.encode().unwrap();
        assert_eq!(ReceiptsV1::decode(&bytes).unwrap(), rs);
        assert_eq!(rs.for_entry(1).map(|r| r.code), Some(99));
        // status 0 with a non-zero code is inconsistent
        let mut v = bytes.clone();
        let status_at = 8 + 4 + 4 + 1 + 2 + 2 + (1 + 2 + 49) + 4;
        assert_eq!(v[status_at], 1);
        v[status_at] = 0;
        assert_eq!(ReceiptsV1::decode(&v), Err(DecodeError::Inconsistent));
        let mut v = bytes;
        v.push(0);
        assert_eq!(ReceiptsV1::decode(&v), Err(DecodeError::TrailingBytes));
    }
}
