//! The SDK's test app (P-08): the smallest app that uses every hook. It
//! counts. Kind 16 `COUNT` (`by u64`, 8 bytes) adds `by` to the account's
//! counter (`ext`, u64) and to the lane total (`app_globals`, u64), and needs
//! `PERM_COUNT` from a session key. Each block end records the block's count.
//! It holds no value, so SDK-INV1 is plain conservation of balances.

use alloc::vec::Vec;

use caravel_core::codec::{Reader, Writer};
use caravel_core::receipts::EventV1;

use crate::app::{AppCtx, AppEngine};
use crate::genesis::template_id;
use crate::{OrFatal, Res};

pub const COUNT: u8 = 16;
pub const PERM_COUNT: u8 = 0x01;
/// `by == 0`.
pub const ZERO_COUNT: u16 = 10;
/// Event 16: `account_idx u32 · by u64 · total u64`.
pub const EVENT_COUNTED: u8 = 16;
/// Event 17 (block end): `blocks u64`, the number of blocks that counted.
pub const EVENT_BLOCK_COUNTED: u8 = 17;

pub struct TestApp;

fn u64_of(bytes: &[u8]) -> u64 {
    let mut r = Reader::new(bytes);
    r.u64().unwrap_or(0)
}

fn bytes_of(v: u64) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

impl AppEngine for TestApp {
    const TEMPLATE: [u8; 16] = template_id("testapp");
    const TEMPLATE_VERSIONS: &'static [u16] = &[1];
    const STATE_MAGIC: [u8; 8] = *b"CVSTTST1";
    const PERMISSIONS: u8 = PERM_COUNT;

    type Params = ();

    fn params(bytes: &[u8]) -> Option<()> {
        bytes.is_empty().then_some(())
    }

    fn genesis_globals(_: &()) -> Vec<u8> {
        bytes_of(0)
    }

    fn new_account_ext(_: &()) -> Vec<u8> {
        bytes_of(0)
    }

    fn body_len(kind: u8) -> Option<usize> {
        (kind == COUNT).then_some(8)
    }

    fn session_permission(kind: u8) -> Option<u8> {
        (kind == COUNT).then_some(PERM_COUNT)
    }

    fn apply(ctx: &mut AppCtx<'_, ()>, a: usize, kind: u8, body: &[u8]) -> Res<u16> {
        debug_assert_eq!(kind, COUNT);
        let by = u64_of(body);
        if by == 0 {
            return Ok(ZERO_COUNT);
        }
        let e = ctx.entry;
        let mine = u64_of(&ctx.st.accounts[a].ext)
            .checked_add(by)
            .or_fatal(e)?;
        let total = u64_of(&ctx.st.app_globals).checked_add(by).or_fatal(e)?;
        ctx.st.accounts[a].ext = bytes_of(mine);
        ctx.st.app_globals = bytes_of(total);
        // app_word counts the blocks with a COUNT, set once per block in end_block.
        ctx.st.app_flags = 1;
        let mut w = Writer::new();
        w.u32(u32::try_from(a).or_fatal(e)?);
        w.u64(by);
        w.u64(total);
        ctx.events.push(EventV1 {
            type_id: EVENT_COUNTED,
            fields: w.into_vec(),
        });
        Ok(0)
    }

    fn end_block(ctx: &mut AppCtx<'_, ()>) -> Res<()> {
        if ctx.st.app_flags == 1 {
            ctx.st.app_flags = 0;
            ctx.st.app_word = ctx.st.app_word.checked_add(1).or_fatal(ctx.entry)?;
            ctx.events.push(EventV1 {
                type_id: EVENT_BLOCK_COUNTED,
                fields: bytes_of(ctx.st.app_word),
            });
        }
        Ok(())
    }

    fn is_empty(st: &crate::SdkState, _: &(), a: usize) -> bool {
        st.accounts[a].balance == 0 && u64_of(&st.accounts[a].ext) == 0
    }
}
