//! Domain tags used inside `H(...)` (spec §9.1). String literals are ASCII bytes
//! with no terminator (spec §0.2).

pub const TAG_TX: &[u8] = b"CARAVEL/TX/V1";
pub const TAG_INBOX: &[u8] = b"CARAVEL/INBOX/V1";
pub const TAG_ORACLE: &[u8] = b"CARAVEL/ORACLE/V1";
pub const TAG_ACCT_LEAF: &[u8] = b"CARAVEL/ACCT/V1";
pub const TAG_WDL_LEAF: &[u8] = b"CARAVEL/WDL/V1";
pub const TAG_SIGNERS: &[u8] = b"CARAVEL/SIGNERS/V1";
pub const TAG_ROTATE: &[u8] = b"CARAVEL/ROTATE/V1";
pub const TAG_LANE_ID: &[u8] = b"CARAVEL/LANE/V1";
/// Rules identity returned by the engine contract's `version` (spec §12.1).
pub const TAG_ENGINE: &[u8] = b"CARAVEL/ENGINE/V1";

/// SEP-53 message prefix (spec §3.5).
pub const SEP53_PREFIX: &[u8] = b"Stellar Signed Message:\n";
/// Prefix of the SEP-53 message a wallet signs for a lane transaction (spec §9.2).
pub const SEP53_TX_PREFIX: &[u8] = b"Caravel lane tx ";

/// Merkle leaf preimages start with 0x00, node preimages with 0x01 (spec §9.9, §11.8).
pub const MERKLE_LEAF_PREFIX: u8 = 0x00;
pub const MERKLE_NODE_PREFIX: u8 = 0x01;
