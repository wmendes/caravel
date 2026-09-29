//! Fatal block errors (spec §8.3, §11). A fatal block is invalid: the sequencer
//! discards it and validators never sign it.

/// `entry_index` for a fatal that is not tied to one entry.
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
pub const UNKNOWN_ORACLE_KEY: u16 = 13;
pub const DUPLICATE_ORACLE_MARKET: u16 = 14;
pub const PENDING_QUEUE_OVERFLOW: u16 = 15;
/// An invariant-level overflow, not one reachable from user input.
pub const ARITHMETIC_OVERFLOW: u16 = 16;
/// Native path only; in Wasm the host traps inside `ed25519_verify`.
pub const BAD_SIGNATURE: u16 = 17;

/// A fatal error with the entry that caused it, or [`BLOCK_LEVEL`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fatal {
    pub code: u16,
    pub entry_index: u32,
}

impl Fatal {
    pub const fn block(code: u16) -> Self {
        Self {
            code,
            entry_index: BLOCK_LEVEL,
        }
    }

    pub const fn entry(code: u16, entry_index: u32) -> Self {
        Self { code, entry_index }
    }
}

/// The spec name of a fatal code, for logs and vectors.
pub fn name(code: u16) -> Option<&'static str> {
    Some(match code {
        BAD_STATE_ENCODING => "BAD_STATE_ENCODING",
        BAD_BLOCK_ENCODING => "BAD_BLOCK_ENCODING",
        BAD_CONFIG => "BAD_CONFIG",
        WRONG_LANE_BLOCK => "WRONG_LANE_BLOCK",
        BAD_HEIGHT => "BAD_HEIGHT",
        BAD_PREV_HASH => "BAD_PREV_HASH",
        TIME_REGRESSION => "TIME_REGRESSION",
        BLOCK_TOO_LARGE => "BLOCK_TOO_LARGE",
        TOO_MANY_ENTRIES => "TOO_MANY_ENTRIES",
        ENTRY_ORDER => "ENTRY_ORDER",
        INBOX_GAP => "INBOX_GAP",
        BAD_ENTRY_ENCODING => "BAD_ENTRY_ENCODING",
        UNKNOWN_ORACLE_KEY => "UNKNOWN_ORACLE_KEY",
        DUPLICATE_ORACLE_MARKET => "DUPLICATE_ORACLE_MARKET",
        PENDING_QUEUE_OVERFLOW => "PENDING_QUEUE_OVERFLOW",
        ARITHMETIC_OVERFLOW => "ARITHMETIC_OVERFLOW",
        BAD_SIGNATURE => "BAD_SIGNATURE",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_one_to_seventeen_are_named() {
        for code in 1..=17 {
            assert!(name(code).is_some(), "fatal code {code} has no name");
        }
        assert_eq!(name(0), None);
        assert_eq!(name(18), None);
        assert_eq!(Fatal::block(BAD_CONFIG).entry_index, BLOCK_LEVEL);
    }
}
