//! `StateV1`, the canonical engine state (spec §9.10). `state_hash = H(StateV1 bytes)`.
//!
//! The decoder checks encoding only. Ordering invariants (books, session keys)
//! are the engine's to keep and are property-tested (spec §11.9). The account
//! lookup by key is rebuilt by the engine on decode; it is not stored.

use alloc::vec::Vec;

use crate::codec::{DecodeError, EncodeError, Reader, Writer};
use crate::config::GenesisConfigV1;

pub const STATE_MAGIC: &[u8; 8] = b"CVSTATE1";
/// `StateV1.flags` bit 0 (informational, spec §11.7). Other bits MUST be 0.
pub const STATE_FLAG_BACKSTOP_DEFICIT: u8 = 0x01;
/// `AccountV1.flags` bit 0. Other bits MUST be 0.
pub const ACCOUNT_FLAG_SYSTEM: u8 = 0x01;
/// Account index of the backstop (insurance fund) and of the treasury.
pub const BACKSTOP_INDEX: u32 = 0;
pub const TREASURY_INDEX: u32 = 1;
pub const COMMITMENT_LEN: usize = 160;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SessionKeyV1 {
    pub key: [u8; 32],
    pub expires_at_ms: u64,
    pub permissions: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PositionV1 {
    /// Signed: long > 0, short < 0.
    pub lots: i64,
    /// Signed, `Σ Δlots × price`.
    pub cost_basis: i128,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountV1 {
    pub key: [u8; 32],
    pub system: bool,
    pub next_nonce: u64,
    pub collateral: i128,
    pub open_order_count: u16,
    /// Sorted by key.
    pub session_keys: Vec<SessionKeyV1>,
    /// One per market, in `config.markets` order.
    pub positions: Vec<PositionV1>,
    /// Reset each block.
    pub txs_this_block: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrderV1 {
    pub order_id: u64,
    pub account_index: u32,
    pub price: i64,
    pub lots_remaining: i64,
    pub client_order_id: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MarketStateV1 {
    pub oracle_price: i64,
    pub oracle_time_ms: u64,
    pub last_funding_time_ms: u64,
    pub cumulative_funding_per_lot: i128,
    pub open_interest_lots: i64,
    /// Best first: price descending, then order_id ascending.
    pub bids: Vec<OrderV1>,
    /// Best first: price ascending, then order_id ascending.
    pub asks: Vec<OrderV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingWithdrawalV1 {
    pub key: [u8; 32],
    pub amount: i128,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommitmentV1 {
    pub seq: u64,
    pub last_block_height: u64,
    pub accounts_root: [u8; 32],
    pub account_count: u32,
    pub escape_total: i128,
    pub withdrawals_root: [u8; 32],
    pub withdrawal_count: u32,
    pub withdrawals_total: i128,
    pub inbox_through: u64,
    pub inbox_acc: [u8; 32],
}

impl CommitmentV1 {
    fn encode_to(&self, w: &mut Writer) {
        w.u64(self.seq);
        w.u64(self.last_block_height);
        w.bytes(&self.accounts_root);
        w.u32(self.account_count);
        w.i128(self.escape_total);
        w.bytes(&self.withdrawals_root);
        w.u32(self.withdrawal_count);
        w.i128(self.withdrawals_total);
        w.u64(self.inbox_through);
        w.bytes(&self.inbox_acc);
    }

    fn decode_from(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            seq: r.u64()?,
            last_block_height: r.u64()?,
            accounts_root: r.array()?,
            account_count: r.u32()?,
            escape_total: r.i128()?,
            withdrawals_root: r.array()?,
            withdrawal_count: r.u32()?,
            withdrawals_total: r.i128()?,
            inbox_through: r.u64()?,
            inbox_acc: r.array()?,
        })
    }
}

impl OrderV1 {
    fn encode_to(&self, w: &mut Writer) {
        w.u64(self.order_id);
        w.u32(self.account_index);
        w.i64(self.price);
        w.i64(self.lots_remaining);
        w.u64(self.client_order_id);
    }

    fn decode_from(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            order_id: r.u64()?,
            account_index: r.u32()?,
            price: r.i64()?,
            lots_remaining: r.i64()?,
            client_order_id: r.u64()?,
        })
    }
}

impl AccountV1 {
    fn encode_to(&self, w: &mut Writer, market_count: usize) -> Result<(), EncodeError> {
        if self.positions.len() != market_count {
            return Err(EncodeError);
        }
        w.bytes(&self.key);
        w.u8(if self.system { ACCOUNT_FLAG_SYSTEM } else { 0 });
        w.u64(self.next_nonce);
        w.i128(self.collateral);
        w.u16(self.open_order_count);
        w.count_u8(self.session_keys.len())?;
        for s in &self.session_keys {
            w.bytes(&s.key);
            w.u64(s.expires_at_ms);
            w.u8(s.permissions);
        }
        for p in &self.positions {
            w.i64(p.lots);
            w.i128(p.cost_basis);
        }
        w.u16(self.txs_this_block);
        Ok(())
    }

    fn decode_from(r: &mut Reader<'_>, market_count: usize) -> Result<Self, DecodeError> {
        let key = r.array()?;
        let flags = r.u8()?;
        if flags & !ACCOUNT_FLAG_SYSTEM != 0 {
            return Err(DecodeError::BadFlags);
        }
        let next_nonce = r.u64()?;
        let collateral = r.i128()?;
        let open_order_count = r.u16()?;
        let mut session_keys = Vec::new();
        for _ in 0..r.u8()? {
            session_keys.push(SessionKeyV1 {
                key: r.array()?,
                expires_at_ms: r.u64()?,
                permissions: r.u8()?,
            });
        }
        let mut positions = Vec::with_capacity(market_count);
        for _ in 0..market_count {
            positions.push(PositionV1 {
                lots: r.i64()?,
                cost_basis: r.i128()?,
            });
        }
        Ok(Self {
            key,
            system: flags & ACCOUNT_FLAG_SYSTEM != 0,
            next_nonce,
            collateral,
            open_order_count,
            session_keys,
            positions,
            txs_this_block: r.u16()?,
        })
    }
}

impl MarketStateV1 {
    fn encode_to(&self, w: &mut Writer) -> Result<(), EncodeError> {
        w.i64(self.oracle_price);
        w.u64(self.oracle_time_ms);
        w.u64(self.last_funding_time_ms);
        w.i128(self.cumulative_funding_per_lot);
        w.i64(self.open_interest_lots);
        w.count_u32(self.bids.len())?;
        for o in &self.bids {
            o.encode_to(w);
        }
        w.count_u32(self.asks.len())?;
        for o in &self.asks {
            o.encode_to(w);
        }
        Ok(())
    }

    fn decode_from(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let mut m = Self {
            oracle_price: r.i64()?,
            oracle_time_ms: r.u64()?,
            last_funding_time_ms: r.u64()?,
            cumulative_funding_per_lot: r.i128()?,
            open_interest_lots: r.i64()?,
            bids: Vec::new(),
            asks: Vec::new(),
        };
        for _ in 0..r.u32()? {
            m.bids.push(OrderV1::decode_from(r)?);
        }
        for _ in 0..r.u32()? {
            m.asks.push(OrderV1::decode_from(r)?);
        }
        Ok(m)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateV1 {
    pub lane_id: [u8; 32],
    /// `H(GenesisConfigV1 bytes)`.
    pub config_hash: [u8; 32],
    /// Last executed block height (0 at genesis).
    pub height: u64,
    /// `H(BlockInputV1)` of the last executed block; zero at genesis.
    pub last_block_input_hash: [u8; 32],
    pub last_timestamp_ms: u64,
    /// Completed checkpoints.
    pub checkpoint_seq: u64,
    pub inbox_through: u64,
    pub inbox_acc: [u8; 32],
    /// Starts at 1.
    pub next_order_id: u64,
    pub deposits_credited_total: i128,
    pub withdrawals_committed_total: i128,
    pub backstop_deficit: bool,
    pub config: GenesisConfigV1,
    /// Index = position; 0 is the backstop, 1 the treasury.
    pub accounts: Vec<AccountV1>,
    /// One per market, in `config.markets` order.
    pub markets: Vec<MarketStateV1>,
    pub pending: Vec<PendingWithdrawalV1>,
    pub last_commitment: CommitmentV1,
}

/// `OrderV1` bytes.
const ORDER_LEN: usize = 36;

impl StateV1 {
    /// Exact length of [`StateV1::encode`], so encoding allocates once.
    pub fn encoded_len(&self) -> usize {
        let markets = self.config.markets.len();
        let accounts: usize = self
            .accounts
            .iter()
            .map(|a| 62 + 41 * a.session_keys.len() + 24 * markets)
            .sum();
        let books: usize = self
            .markets
            .iter()
            .map(|m| 56 + ORDER_LEN * (m.bids.len() + m.asks.len()))
            .sum();
        209 + self.config.encoded_len()
            + 4
            + accounts
            + books
            + 4
            + 48 * self.pending.len()
            + COMMITMENT_LEN
    }

    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let market_count = self.config.markets.len();
        if self.markets.len() != market_count {
            return Err(EncodeError);
        }
        let mut w = Writer::with_capacity(self.encoded_len());
        w.bytes(STATE_MAGIC);
        w.bytes(&self.lane_id);
        w.bytes(&self.config_hash);
        w.u64(self.height);
        w.bytes(&self.last_block_input_hash);
        w.u64(self.last_timestamp_ms);
        w.u64(self.checkpoint_seq);
        w.u64(self.inbox_through);
        w.bytes(&self.inbox_acc);
        w.u64(self.next_order_id);
        w.i128(self.deposits_credited_total);
        w.i128(self.withdrawals_committed_total);
        w.u8(if self.backstop_deficit {
            STATE_FLAG_BACKSTOP_DEFICIT
        } else {
            0
        });
        w.bytes(&self.config.encode()?);
        w.count_u32(self.accounts.len())?;
        for a in &self.accounts {
            a.encode_to(&mut w, market_count)?;
        }
        for m in &self.markets {
            m.encode_to(&mut w)?;
        }
        w.count_u32(self.pending.len())?;
        for p in &self.pending {
            w.bytes(&p.key);
            w.i128(p.amount);
        }
        self.last_commitment.encode_to(&mut w);
        Ok(w.into_vec())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        r.magic(STATE_MAGIC)?;
        let lane_id = r.array()?;
        let config_hash = r.array()?;
        let height = r.u64()?;
        let last_block_input_hash = r.array()?;
        let last_timestamp_ms = r.u64()?;
        let checkpoint_seq = r.u64()?;
        let inbox_through = r.u64()?;
        let inbox_acc = r.array()?;
        let next_order_id = r.u64()?;
        let deposits_credited_total = r.i128()?;
        let withdrawals_committed_total = r.i128()?;
        let flags = r.u8()?;
        if flags & !STATE_FLAG_BACKSTOP_DEFICIT != 0 {
            return Err(DecodeError::BadFlags);
        }
        let config = GenesisConfigV1::decode_from(&mut r)?;
        let market_count = config.markets.len();
        let mut accounts = Vec::new();
        for _ in 0..r.u32()? {
            accounts.push(AccountV1::decode_from(&mut r, market_count)?);
        }
        let mut markets = Vec::with_capacity(market_count);
        for _ in 0..market_count {
            markets.push(MarketStateV1::decode_from(&mut r)?);
        }
        let mut pending = Vec::new();
        for _ in 0..r.u32()? {
            pending.push(PendingWithdrawalV1 {
                key: r.array()?,
                amount: r.i128()?,
            });
        }
        let last_commitment = CommitmentV1::decode_from(&mut r)?;
        r.finish()?;
        Ok(Self {
            lane_id,
            config_hash,
            height,
            last_block_input_hash,
            last_timestamp_ms,
            checkpoint_seq,
            inbox_through,
            inbox_acc,
            next_order_id,
            deposits_credited_total,
            withdrawals_committed_total,
            backstop_deficit: flags & STATE_FLAG_BACKSTOP_DEFICIT != 0,
            config,
            accounts,
            markets,
            pending,
            last_commitment,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::tests::testnet_like;

    pub(crate) fn sample_state() -> StateV1 {
        let config = testnet_like();
        let n = config.markets.len();
        let account = |key: u8, system: bool| AccountV1 {
            key: [key; 32],
            system,
            next_nonce: 3,
            collateral: -12,
            open_order_count: 1,
            session_keys: Vec::new(),
            positions: alloc::vec![PositionV1::default(); n],
            txs_this_block: 0,
        };
        let mut user = account(0x40, false);
        user.session_keys = alloc::vec![SessionKeyV1 {
            key: [0x50; 32],
            expires_at_ms: 9,
            permissions: 3
        }];
        user.positions[1] = PositionV1 {
            lots: -7,
            cost_basis: -21_000,
        };
        let mut markets = alloc::vec![MarketStateV1::default(); n];
        markets[0].oracle_price = 6_500_000;
        markets[0].bids = alloc::vec![OrderV1 {
            order_id: 1,
            account_index: 2,
            price: 6_499_000,
            lots_remaining: 5,
            client_order_id: 77
        }];
        StateV1 {
            lane_id: config.lane_id,
            config_hash: [0x99; 32],
            height: 10,
            last_block_input_hash: [0x98; 32],
            last_timestamp_ms: 1_790_000_010_000,
            checkpoint_seq: 1,
            inbox_through: 2,
            inbox_acc: [0x97; 32],
            next_order_id: 2,
            deposits_credited_total: 20_000_000,
            withdrawals_committed_total: 0,
            backstop_deficit: true,
            accounts: alloc::vec![account(0x21, true), account(0x22, true), user],
            markets,
            pending: alloc::vec![PendingWithdrawalV1 {
                key: [0x40; 32],
                amount: 5
            }],
            last_commitment: CommitmentV1 {
                seq: 1,
                last_block_height: 10,
                account_count: 3,
                ..CommitmentV1::default()
            },
            config,
        }
    }

    #[test]
    fn round_trip() {
        let s = sample_state();
        let bytes = s.encode().unwrap();
        assert_eq!(bytes.len(), s.encoded_len());
        assert_eq!(s.config.encode().unwrap().len(), s.config.encoded_len());
        assert_eq!(&bytes[..8], b"CVSTATE1");
        assert_eq!(StateV1::decode(&bytes), Ok(s));
    }

    #[test]
    fn commitment_is_160_bytes() {
        let mut w = Writer::new();
        CommitmentV1::default().encode_to(&mut w);
        assert_eq!(w.len(), COMMITMENT_LEN);
    }

    #[test]
    fn strict_flags_and_lengths() {
        let bytes = sample_state().encode().unwrap();
        // StateV1.flags sits right after the fixed 8+32+32+8+32+8+8+8+32+8+16+16 prefix.
        let flags_at = 8 + 32 + 32 + 8 + 32 + 8 + 8 + 8 + 32 + 8 + 16 + 16;
        assert_eq!(bytes[flags_at], 1);
        let mut bad = bytes.clone();
        bad[flags_at] = 0x02;
        assert_eq!(StateV1::decode(&bad), Err(DecodeError::BadFlags));
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(StateV1::decode(&trailing), Err(DecodeError::TrailingBytes));
        assert_eq!(
            StateV1::decode(&bytes[..bytes.len() - 1]),
            Err(DecodeError::UnexpectedEnd)
        );
    }

    #[test]
    fn positions_must_match_market_count() {
        let mut s = sample_state();
        s.accounts[0].positions.pop();
        assert_eq!(s.encode(), Err(EncodeError));
        let mut s = sample_state();
        s.markets.pop();
        assert_eq!(s.encode(), Err(EncodeError));
    }
}
