//! Caravel Payments (spec §20.4.4, DEC-060): USDC transfers between lane
//! accounts, on the app SDK. Template `payments` 0.1.0, state magic `CVSTPAY1`.
//!
//! - `app_params` (32 bytes): `transfer_fee i128` (≥ 0, flat, paid to the
//!   treasury, system account 0) · `min_transfer i128` (≥ 1).
//! - Kind 16 `TRANSFER`: `to 32 · amount i128 · memo u64` (56 bytes), by the
//!   owner or a session key with `PERM_TRANSFER`.
//! - Free balance and escape equity are the balance. No feeds.
//! - INV-PAY1: `Σ balances + Σ pending == deposits_credited − withdrawals_committed`.
//!
//! Consensus code: the determinism rules in spec §8 apply to everything here.
#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]

extern crate alloc;

use alloc::vec::Vec;

use caravel_app_sdk::{template_id, AppCtx, AppEngine, OrFatal, Res};
use caravel_core::codec::{Reader, Writer};
use caravel_core::receipts::EventV1;

/// `version()` of the engine contract: `H(TAG_ENGINE || ENGINE_VERSION)`.
pub const ENGINE_VERSION: &str = "payments/0.1.0";

pub const TRANSFER: u8 = 16;
pub const TRANSFER_BODY_LEN: usize = 56;
pub const PERM_TRANSFER: u8 = 0x01;
/// Treasury: system account 0.
pub const TREASURY_INDEX: usize = 0;

/// Receipt codes (the app's range, 10–39).
pub mod codes {
    pub const INSUFFICIENT_BALANCE: u16 = 10;
    pub const UNKNOWN_RECIPIENT: u16 = 11;
    pub const BELOW_MIN_TRANSFER: u16 = 12;
    pub const SELF_TRANSFER: u16 = 13;
}

/// Event 16: `from_idx u32 · to_idx u32 · amount i128 · fee i128 · memo u64`.
pub const EVENT_TRANSFER: u8 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    pub transfer_fee: i128,
    pub min_transfer: i128,
}

impl Params {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(32);
        w.i128(self.transfer_fee);
        w.i128(self.min_transfer);
        w.into_vec()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transfer {
    pub to: [u8; 32],
    pub amount: i128,
    pub memo: u64,
}

impl Transfer {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(TRANSFER_BODY_LEN);
        w.bytes(&self.to);
        w.i128(self.amount);
        w.u64(self.memo);
        w.into_vec()
    }

    pub fn decode(body: &[u8]) -> Option<Self> {
        let mut r = Reader::new(body);
        let t = Self {
            to: r.array().ok()?,
            amount: r.i128().ok()?,
            memo: r.u64().ok()?,
        };
        r.finish().ok()?;
        Some(t)
    }
}

/// A decoded `TRANSFER` event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferEvent {
    pub from_idx: u32,
    pub to_idx: u32,
    pub amount: i128,
    pub fee: i128,
    pub memo: u64,
}

impl TransferEvent {
    pub fn decode(e: &EventV1) -> Option<Self> {
        if e.type_id != EVENT_TRANSFER {
            return None;
        }
        let mut r = Reader::new(&e.fields);
        let t = Self {
            from_idx: r.u32().ok()?,
            to_idx: r.u32().ok()?,
            amount: r.i128().ok()?,
            fee: r.i128().ok()?,
            memo: r.u64().ok()?,
        };
        r.finish().ok()?;
        Some(t)
    }
}

pub struct Payments;

impl AppEngine for Payments {
    const TEMPLATE: [u8; 16] = template_id("payments");
    const TEMPLATE_VERSIONS: &'static [u16] = &[1];
    const STATE_MAGIC: [u8; 8] = *b"CVSTPAY1";
    const PERMISSIONS: u8 = PERM_TRANSFER;

    type Params = Params;

    fn params(bytes: &[u8]) -> Option<Params> {
        let mut r = Reader::new(bytes);
        let p = Params {
            transfer_fee: r.i128().ok()?,
            min_transfer: r.i128().ok()?,
        };
        r.finish().ok()?;
        (p.transfer_fee >= 0 && p.min_transfer >= 1).then_some(p)
    }

    fn body_len(kind: u8) -> Option<usize> {
        (kind == TRANSFER).then_some(TRANSFER_BODY_LEN)
    }

    fn session_permission(kind: u8) -> Option<u8> {
        (kind == TRANSFER).then_some(PERM_TRANSFER)
    }

    fn apply(ctx: &mut AppCtx<'_, Params>, a: usize, kind: u8, body: &[u8]) -> Res<u16> {
        debug_assert_eq!(kind, TRANSFER);
        let e = ctx.entry;
        // The body length was checked before execution, so this decodes.
        let t = Transfer::decode(body).or_fatal(e)?;
        let fee = ctx.params.transfer_fee;
        if t.amount < ctx.params.min_transfer {
            return Ok(codes::BELOW_MIN_TRANSFER);
        }
        if t.to == ctx.st.accounts[a].key {
            return Ok(codes::SELF_TRANSFER);
        }
        let Some(to) = ctx.account(&t.to) else {
            return Ok(codes::UNKNOWN_RECIPIENT);
        };
        match t.amount.checked_add(fee) {
            Some(total) if total <= ctx.st.accounts[a].balance => {}
            _ => return Ok(codes::INSUFFICIENT_BALANCE),
        }
        let debit = t.amount.checked_add(fee).or_fatal(e)?;
        let from = &mut ctx.st.accounts[a].balance;
        *from = from.checked_sub(debit).or_fatal(e)?;
        let dest = &mut ctx.st.accounts[to].balance;
        *dest = dest.checked_add(t.amount).or_fatal(e)?;
        let treasury = &mut ctx.st.accounts[TREASURY_INDEX].balance;
        *treasury = treasury.checked_add(fee).or_fatal(e)?;
        let mut w = Writer::with_capacity(4 + 4 + 16 + 16 + 8);
        w.u32(u32::try_from(a).or_fatal(e)?);
        w.u32(u32::try_from(to).or_fatal(e)?);
        w.i128(t.amount);
        w.i128(fee);
        w.u64(t.memo);
        ctx.events.push(EventV1 {
            type_id: EVENT_TRANSFER,
            fields: w.into_vec(),
        });
        Ok(0)
    }
}
