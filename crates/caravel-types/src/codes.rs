//! Non-fatal transaction reason codes (spec §11.3). Keep this exact numbering.

pub const OK: u16 = 0;
pub const WRONG_LANE: u16 = 1;
pub const UNKNOWN_ACCOUNT: u16 = 2;
pub const UNAUTHORIZED_SIGNER: u16 = 3;
pub const EXPIRED: u16 = 4;
pub const BAD_NONCE: u16 = 5;
pub const RATE_LIMITED: u16 = 6;
pub const UNKNOWN_MARKET: u16 = 10;
pub const BAD_PRICE: u16 = 11;
pub const BAD_SIZE: u16 = 12;
pub const ORACLE_STALE: u16 = 13;
pub const OUTSIDE_PRICE_BAND: u16 = 14;
pub const TOO_MANY_OPEN_ORDERS: u16 = 15;
pub const REDUCE_ONLY_VIOLATION: u16 = 16;
pub const POST_ONLY_WOULD_CROSS: u16 = 17;
pub const INSUFFICIENT_MARGIN: u16 = 18;
pub const POSITION_LIMIT: u16 = 19;
pub const OPEN_INTEREST_LIMIT: u16 = 20;
pub const BOOK_FULL: u16 = 21;
// 22 is reserved.
pub const ORDER_NOT_FOUND: u16 = 30;
pub const BELOW_MIN_WITHDRAWAL: u16 = 40;
pub const INSUFFICIENT_FREE_COLLATERAL: u16 = 41;
pub const WITHDRAWAL_QUEUE_FULL: u16 = 42;
pub const INSUFFICIENT_LANE_LIQUIDITY: u16 = 43;
pub const TOO_MANY_SESSION_KEYS: u16 = 50;
pub const BAD_SESSION_KEY: u16 = 51;

/// Every code a receipt may carry, in ascending order.
pub const ALL: &[u16] = &[
    OK,
    WRONG_LANE,
    UNKNOWN_ACCOUNT,
    UNAUTHORIZED_SIGNER,
    EXPIRED,
    BAD_NONCE,
    RATE_LIMITED,
    UNKNOWN_MARKET,
    BAD_PRICE,
    BAD_SIZE,
    ORACLE_STALE,
    OUTSIDE_PRICE_BAND,
    TOO_MANY_OPEN_ORDERS,
    REDUCE_ONLY_VIOLATION,
    POST_ONLY_WOULD_CROSS,
    INSUFFICIENT_MARGIN,
    POSITION_LIMIT,
    OPEN_INTEREST_LIMIT,
    BOOK_FULL,
    ORDER_NOT_FOUND,
    BELOW_MIN_WITHDRAWAL,
    INSUFFICIENT_FREE_COLLATERAL,
    WITHDRAWAL_QUEUE_FULL,
    INSUFFICIENT_LANE_LIQUIDITY,
    TOO_MANY_SESSION_KEYS,
    BAD_SESSION_KEY,
];

/// Whether `code` is a listed reason code (22 is reserved and never emitted).
pub fn is_known(code: u16) -> bool {
    ALL.binary_search(&code).is_ok()
}

/// The spec name of a code, for logs and vectors.
pub fn name(code: u16) -> Option<&'static str> {
    Some(match code {
        OK => "OK",
        WRONG_LANE => "WRONG_LANE",
        UNKNOWN_ACCOUNT => "UNKNOWN_ACCOUNT",
        UNAUTHORIZED_SIGNER => "UNAUTHORIZED_SIGNER",
        EXPIRED => "EXPIRED",
        BAD_NONCE => "BAD_NONCE",
        RATE_LIMITED => "RATE_LIMITED",
        UNKNOWN_MARKET => "UNKNOWN_MARKET",
        BAD_PRICE => "BAD_PRICE",
        BAD_SIZE => "BAD_SIZE",
        ORACLE_STALE => "ORACLE_STALE",
        OUTSIDE_PRICE_BAND => "OUTSIDE_PRICE_BAND",
        TOO_MANY_OPEN_ORDERS => "TOO_MANY_OPEN_ORDERS",
        REDUCE_ONLY_VIOLATION => "REDUCE_ONLY_VIOLATION",
        POST_ONLY_WOULD_CROSS => "POST_ONLY_WOULD_CROSS",
        INSUFFICIENT_MARGIN => "INSUFFICIENT_MARGIN",
        POSITION_LIMIT => "POSITION_LIMIT",
        OPEN_INTEREST_LIMIT => "OPEN_INTEREST_LIMIT",
        BOOK_FULL => "BOOK_FULL",
        ORDER_NOT_FOUND => "ORDER_NOT_FOUND",
        BELOW_MIN_WITHDRAWAL => "BELOW_MIN_WITHDRAWAL",
        INSUFFICIENT_FREE_COLLATERAL => "INSUFFICIENT_FREE_COLLATERAL",
        WITHDRAWAL_QUEUE_FULL => "WITHDRAWAL_QUEUE_FULL",
        INSUFFICIENT_LANE_LIQUIDITY => "INSUFFICIENT_LANE_LIQUIDITY",
        TOO_MANY_SESSION_KEYS => "TOO_MANY_SESSION_KEYS",
        BAD_SESSION_KEY => "BAD_SESSION_KEY",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_is_sorted_named_and_skips_reserved() {
        assert!(ALL.windows(2).all(|w| w[0] < w[1]));
        assert!(ALL.iter().all(|c| name(*c).is_some()));
        assert!(!is_known(22));
        assert!(is_known(BAD_SESSION_KEY));
        assert_eq!(ALL.len(), 26);
    }
}
