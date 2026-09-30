//! What an app gives the SDK (spec §20.4.3): its identity, its transaction
//! kinds and permission bits, and its hooks. Everything else (the inbox,
//! withdrawals, session keys, nonces, commitments, receipts) is the SDK's.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use caravel_core::receipts::EventV1;

use crate::state::SdkState;
use crate::Res;

/// What an app sees while it applies a transaction or a block hook.
pub struct AppCtx<'a, P> {
    pub st: &'a mut SdkState,
    pub params: &'a P,
    /// Account key → index.
    pub index: &'a BTreeMap<[u8; 32], u32>,
    /// `block.timestamp_ms`.
    pub now: u64,
    /// The entry being applied (`BLOCK_LEVEL` in block hooks).
    pub entry: u32,
    pub events: &'a mut Vec<EventV1>,
}

impl<P> AppCtx<'_, P> {
    pub fn account(&self, key: &[u8; 32]) -> Option<usize> {
        self.index.get(key).map(|i| *i as usize)
    }
}

/// An app engine on the SDK. Static dispatch: `step::<MyApp>(..)`.
pub trait AppEngine {
    /// `AppGenesisV1.template`.
    const TEMPLATE: [u8; 16];
    /// The template versions this engine runs.
    const TEMPLATE_VERSIONS: &'static [u16];
    /// The state magic, the first 8 bytes of the state.
    const STATE_MAGIC: [u8; 8];
    /// The session-key permission bits the app defines. A session key with
    /// other bits is refused (`BAD_SESSION_KEY`).
    const PERMISSIONS: u8;

    /// The decoded `app_params`.
    type Params;

    /// Decodes `app_params` strictly and checks the app's genesis rules.
    fn params(bytes: &[u8]) -> Option<Self::Params>;

    /// `app_globals` at genesis.
    fn genesis_globals(_params: &Self::Params) -> Vec<u8> {
        Vec::new()
    }

    /// `ext` of a new account.
    fn new_account_ext(_params: &Self::Params) -> Vec<u8> {
        Vec::new()
    }

    /// The exact body length of an app kind (1..=3 or 16..=255), or `None`
    /// if the app has no such kind. A block holding an unknown kind or a body
    /// of the wrong length is fatal, as in M0.
    fn body_len(kind: u8) -> Option<usize>;

    /// The permission bit a session key needs to sign `kind`; `None` means
    /// owner only.
    fn session_permission(kind: u8) -> Option<u8>;

    /// Applies an app kind for account `a`; the nonce is already consumed.
    /// Returns the receipt code: `OK`, or a rejection (events are then dropped).
    fn apply(ctx: &mut AppCtx<'_, Self::Params>, a: usize, kind: u8, body: &[u8]) -> Res<u16>;

    /// Runs after the per-block reset, before the inbox. Its events go in the
    /// block-start pseudo entry.
    fn begin_block(_ctx: &mut AppCtx<'_, Self::Params>) -> Res<()> {
        Ok(())
    }

    /// Runs after the user entries, before the commitment. Its events go in
    /// the block-end pseudo entry.
    fn end_block(_ctx: &mut AppCtx<'_, Self::Params>) -> Res<()> {
        Ok(())
    }

    /// What account `a` may withdraw now; at most its balance.
    fn free_balance(st: &SdkState, _params: &Self::Params, a: usize) -> Res<i128> {
        Ok(st.accounts[a].balance)
    }

    /// Account `a`'s escape equity for the commitment; the SDK floors it at 0.
    fn escape_equity(st: &SdkState, _params: &Self::Params, a: usize) -> Res<i128> {
        Ok(st.accounts[a].balance)
    }

    /// Whether non-system account `a` holds nothing, so a deposit for a new
    /// key may reuse its slot. The SDK also requires no pending withdrawal.
    fn is_empty(st: &SdkState, _params: &Self::Params, a: usize) -> bool {
        st.accounts[a].balance == 0
    }

    /// Runs before a forced withdrawal is sized (perps cancels orders here).
    fn before_forced_withdrawal(_ctx: &mut AppCtx<'_, Self::Params>, _a: usize) -> Res<()> {
        Ok(())
    }
}
