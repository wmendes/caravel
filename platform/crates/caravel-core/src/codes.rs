//! Receipt codes and fatal codes the platform defines (spec §11, DEC-052).
//!
//! Ranges: receipt codes 0–9 and 40–59 are the platform's, 10–39 and 60+ an
//! app's. Fatal codes 1–31 are the platform's, 32+ an app's. The M0 perps
//! codes keep their numbers, so the perps lane is unchanged.

/// Receipt codes: the transaction envelope and the standard kinds.
pub mod receipt {
    pub const OK: u16 = 0;
    pub const WRONG_LANE: u16 = 1;
    pub const UNKNOWN_ACCOUNT: u16 = 2;
    pub const UNAUTHORIZED_SIGNER: u16 = 3;
    pub const EXPIRED: u16 = 4;
    pub const BAD_NONCE: u16 = 5;
    pub const RATE_LIMITED: u16 = 6;
    /// `WITHDRAW` (kind 4).
    pub const BELOW_MIN_WITHDRAWAL: u16 = 40;
    pub const INSUFFICIENT_FREE_BALANCE: u16 = 41;
    pub const WITHDRAWAL_QUEUE_FULL: u16 = 42;
    pub const INSUFFICIENT_LANE_LIQUIDITY: u16 = 43;
    /// `ADD_SESSION_KEY` / `REVOKE_SESSION_KEY` (kinds 5 and 6).
    pub const TOO_MANY_SESSION_KEYS: u16 = 50;
    pub const BAD_SESSION_KEY: u16 = 51;

    /// Whether `code` is in a range the platform defines.
    pub fn is_platform(code: u16) -> bool {
        code <= 9 || (40..=59).contains(&code)
    }
}

/// Fatal codes: a block no engine may execute.
pub mod fatal {
    /// Entry index of a block-level fatal.
    pub const BLOCK_LEVEL: u32 = 0xFFFF_FFFF;
    pub const BAD_STATE_ENCODING: u16 = 1;
    pub const BAD_BLOCK_ENCODING: u16 = 2;
    pub const BAD_CONFIG: u16 = 3;
    pub const WRONG_LANE_BLOCK: u16 = 4;
    pub const BAD_HEIGHT: u16 = 5;
    pub const BAD_PREV_HASH: u16 = 6;
    pub const TIME_REGRESSION: u16 = 7;
    pub const BLOCK_TOO_LARGE: u16 = 8;
    pub const TOO_MANY_ENTRIES: u16 = 9;
    pub const ENTRY_ORDER: u16 = 10;
    pub const INBOX_GAP: u16 = 11;
    pub const BAD_ENTRY_ENCODING: u16 = 12;
    /// A feed entry signed by a key the config does not list (M0: oracle key).
    pub const UNKNOWN_FEED_KEY: u16 = 13;
    /// Two feed entries for one slot in a block (M0: one oracle update per market).
    pub const DUPLICATE_FEED_SLOT: u16 = 14;
    pub const PENDING_QUEUE_OVERFLOW: u16 = 15;
    pub const ARITHMETIC_OVERFLOW: u16 = 16;
    pub const BAD_SIGNATURE: u16 = 17;
    /// The last platform fatal code; apps use 32 and up.
    pub const LAST_PLATFORM: u16 = 31;
}
