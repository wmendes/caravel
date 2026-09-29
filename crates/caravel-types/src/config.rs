//! `GenesisConfigV1`, the lane's consensus settings (spec §10.2).

use alloc::vec::Vec;

use crate::batch::MAX_BATCH_BYTES;
use crate::codec::{DecodeError, EncodeError, Reader, Writer};

pub const GENESIS_MAGIC: &[u8; 8] = b"CVGENES1";
pub const MARKET_PARAMS_LEN: usize = 71;
pub const MAX_MARKETS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessMode {
    Open = 0,
    Allowlist = 1,
}

impl AccessMode {
    pub fn from_u8(v: u8) -> Result<Self, DecodeError> {
        match v {
            0 => Ok(Self::Open),
            1 => Ok(Self::Allowlist),
            _ => Err(DecodeError::BadEnum),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarketParamsV1 {
    pub market_id: u16,
    /// ASCII, zero-padded.
    pub symbol: [u8; 16],
    pub tick: i64,
    pub imf_bps: u16,
    pub mmf_bps: u16,
    pub taker_fee_bps: u16,
    pub maker_fee_bps: u16,
    pub liq_fee_bps: u16,
    pub band_bps: u16,
    pub max_position_lots: i64,
    pub max_oi_lots: i64,
    /// Depth used for the funding premium (spec §11.6).
    pub impact_lots: i64,
    /// UI only, not used in math.
    pub display_lot_base_units: i64,
    /// UI only, not used in math.
    pub display_base_decimals: u8,
}

impl MarketParamsV1 {
    fn encode_to(&self, w: &mut Writer) {
        w.u16(self.market_id);
        w.bytes(&self.symbol);
        w.i64(self.tick);
        w.u16(self.imf_bps);
        w.u16(self.mmf_bps);
        w.u16(self.taker_fee_bps);
        w.u16(self.maker_fee_bps);
        w.u16(self.liq_fee_bps);
        w.u16(self.band_bps);
        w.i64(self.max_position_lots);
        w.i64(self.max_oi_lots);
        w.i64(self.impact_lots);
        w.i64(self.display_lot_base_units);
        w.u8(self.display_base_decimals);
    }

    fn decode_from(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            market_id: r.u16()?,
            symbol: r.array()?,
            tick: r.i64()?,
            imf_bps: r.u16()?,
            mmf_bps: r.u16()?,
            taker_fee_bps: r.u16()?,
            maker_fee_bps: r.u16()?,
            liq_fee_bps: r.u16()?,
            band_bps: r.u16()?,
            max_position_lots: r.i64()?,
            max_oi_lots: r.i64()?,
            impact_lots: r.i64()?,
            display_lot_base_units: r.i64()?,
            display_base_decimals: r.u8()?,
        })
    }

    /// The symbol without its zero padding.
    pub fn symbol_str(&self) -> &str {
        let end = self
            .symbol
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(self.symbol.len());
        core::str::from_utf8(&self.symbol[..end]).unwrap_or("")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenesisConfigV1 {
    pub lane_id: [u8; 32],
    /// System account 0: insurance fund and liquidation backstop.
    pub backstop_key: [u8; 32],
    /// System account 1: fees.
    pub treasury_key: [u8; 32],
    pub access_mode: AccessMode,
    /// Sorted strictly ascending (validated, not decoded).
    pub allowlist: Vec<[u8; 32]>,
    /// Sorted strictly ascending, at least one (validated, not decoded).
    pub oracle_keys: Vec<[u8; 32]>,
    pub oracle_max_staleness_ms: u64,
    pub oracle_max_future_ms: u64,
    pub oracle_circuit_breaker_bps: u16,
    pub oracle_breaker_bps_per_sec: u16,
    pub funding_interval_ms: u64,
    pub funding_damping: u16,
    pub funding_max_rate_ppm: u32,
    pub insurance_fee_share_bps: u16,
    pub min_deposit: i128,
    pub min_withdrawal: i128,
    pub max_accounts: u32,
    pub max_orders_per_side: u32,
    pub max_open_orders_per_account: u16,
    pub max_session_keys: u8,
    pub max_txs_per_account_per_block: u16,
    pub max_entries_per_block: u32,
    pub max_block_bytes: u32,
    pub max_pending_withdrawals: u32,
    /// Host CPU budget per `step` call; consensus (spec §14.5, DEC-015).
    pub exec_cpu_limit: u64,
    /// Host memory budget per `step` call; consensus.
    pub exec_mem_limit: u64,
    pub markets: Vec<MarketParamsV1>,
}

/// The first genesis rule (spec §10.2) a config breaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    SystemKeysEqual,
    AllowlistNotAscending,
    NoOracleKeys,
    OracleKeysNotAscending,
    MarketIdsNotAscending,
    FundingInterval,
    FundingDamping,
    FundingMaxRate,
    InsuranceShare,
    CircuitBreaker,
    OracleStaleness,
    MinDeposit,
    MinWithdrawal,
    /// One of the `max_*` caps is 0.
    ZeroCap,
    TooFewAccounts,
    BlockBytes,
    ExecCpuLimit,
    ExecMemLimit,
    MarketCount,
    Tick {
        market_id: u16,
    },
    MarginBps {
        market_id: u16,
    },
    FeeBps {
        market_id: u16,
    },
    LiqFeeAboveMaintenance {
        market_id: u16,
    },
    BandBps {
        market_id: u16,
    },
    PositionLimits {
        market_id: u16,
    },
    ImpactLots {
        market_id: u16,
    },
    SymbolNotAscii {
        market_id: u16,
    },
}

fn strictly_ascending<T: Ord>(items: &[T]) -> bool {
    items.windows(2).all(|w| w[0] < w[1])
}

impl GenesisConfigV1 {
    /// Exact length of [`GenesisConfigV1::encode`].
    pub fn encoded_len(&self) -> usize {
        8 + 32 * 3
            + 1
            + 2
            + 32 * self.allowlist.len()
            + 1
            + 32 * self.oracle_keys.len()
            + 109
            + 2
            + MARKET_PARAMS_LEN * self.markets.len()
    }

    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let mut w = Writer::with_capacity(self.encoded_len());
        w.bytes(GENESIS_MAGIC);
        w.bytes(&self.lane_id);
        w.bytes(&self.backstop_key);
        w.bytes(&self.treasury_key);
        w.u8(self.access_mode as u8);
        w.count_u16(self.allowlist.len())?;
        for k in &self.allowlist {
            w.bytes(k);
        }
        w.count_u8(self.oracle_keys.len())?;
        for k in &self.oracle_keys {
            w.bytes(k);
        }
        w.u64(self.oracle_max_staleness_ms);
        w.u64(self.oracle_max_future_ms);
        w.u16(self.oracle_circuit_breaker_bps);
        w.u16(self.oracle_breaker_bps_per_sec);
        w.u64(self.funding_interval_ms);
        w.u16(self.funding_damping);
        w.u32(self.funding_max_rate_ppm);
        w.u16(self.insurance_fee_share_bps);
        w.i128(self.min_deposit);
        w.i128(self.min_withdrawal);
        w.u32(self.max_accounts);
        w.u32(self.max_orders_per_side);
        w.u16(self.max_open_orders_per_account);
        w.u8(self.max_session_keys);
        w.u16(self.max_txs_per_account_per_block);
        w.u32(self.max_entries_per_block);
        w.u32(self.max_block_bytes);
        w.u32(self.max_pending_withdrawals);
        w.u64(self.exec_cpu_limit);
        w.u64(self.exec_mem_limit);
        w.count_u16(self.markets.len())?;
        for m in &self.markets {
            m.encode_to(&mut w);
        }
        Ok(w.into_vec())
    }

    /// Decodes a config embedded in a larger object (`StateV1`).
    pub fn decode_from(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.magic(GENESIS_MAGIC)?;
        let lane_id = r.array()?;
        let backstop_key = r.array()?;
        let treasury_key = r.array()?;
        let access_mode = AccessMode::from_u8(r.u8()?)?;
        let mut allowlist = Vec::new();
        for _ in 0..r.u16()? {
            allowlist.push(r.array()?);
        }
        let mut oracle_keys = Vec::new();
        for _ in 0..r.u8()? {
            oracle_keys.push(r.array()?);
        }
        let mut cfg = Self {
            lane_id,
            backstop_key,
            treasury_key,
            access_mode,
            allowlist,
            oracle_keys,
            oracle_max_staleness_ms: r.u64()?,
            oracle_max_future_ms: r.u64()?,
            oracle_circuit_breaker_bps: r.u16()?,
            oracle_breaker_bps_per_sec: r.u16()?,
            funding_interval_ms: r.u64()?,
            funding_damping: r.u16()?,
            funding_max_rate_ppm: r.u32()?,
            insurance_fee_share_bps: r.u16()?,
            min_deposit: r.i128()?,
            min_withdrawal: r.i128()?,
            max_accounts: r.u32()?,
            max_orders_per_side: r.u32()?,
            max_open_orders_per_account: r.u16()?,
            max_session_keys: r.u8()?,
            max_txs_per_account_per_block: r.u16()?,
            max_entries_per_block: r.u32()?,
            max_block_bytes: r.u32()?,
            max_pending_withdrawals: r.u32()?,
            exec_cpu_limit: r.u64()?,
            exec_mem_limit: r.u64()?,
            markets: Vec::new(),
        };
        for _ in 0..r.u16()? {
            cfg.markets.push(MarketParamsV1::decode_from(r)?);
        }
        Ok(cfg)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let cfg = Self::decode_from(&mut r)?;
        r.finish()?;
        Ok(cfg)
    }

    /// Market parameters by id.
    pub fn market(&self, market_id: u16) -> Option<&MarketParamsV1> {
        self.markets.iter().find(|m| m.market_id == market_id)
    }

    /// Checks every genesis rule in spec §10.2, in the listed order.
    pub fn validate(&self) -> Result<(), ConfigError> {
        use ConfigError as E;
        if self.backstop_key == self.treasury_key {
            return Err(E::SystemKeysEqual);
        }
        if !strictly_ascending(&self.allowlist) {
            return Err(E::AllowlistNotAscending);
        }
        if self.oracle_keys.is_empty() {
            return Err(E::NoOracleKeys);
        }
        if !strictly_ascending(&self.oracle_keys) {
            return Err(E::OracleKeysNotAscending);
        }
        let ids: Vec<u16> = self.markets.iter().map(|m| m.market_id).collect();
        if !strictly_ascending(&ids) {
            return Err(E::MarketIdsNotAscending);
        }
        if self.funding_interval_ms < 60_000 {
            return Err(E::FundingInterval);
        }
        if self.funding_damping < 1 {
            return Err(E::FundingDamping);
        }
        if self.funding_max_rate_ppm > 10_000 {
            return Err(E::FundingMaxRate);
        }
        if self.insurance_fee_share_bps > 10_000 {
            return Err(E::InsuranceShare);
        }
        if self.oracle_circuit_breaker_bps < 1 {
            return Err(E::CircuitBreaker);
        }
        if self.oracle_max_staleness_ms < 1_000 {
            return Err(E::OracleStaleness);
        }
        if self.min_deposit < 1 {
            return Err(E::MinDeposit);
        }
        if self.min_withdrawal < 1 {
            return Err(E::MinWithdrawal);
        }
        let caps_positive = self.max_accounts > 0
            && self.max_orders_per_side > 0
            && self.max_open_orders_per_account > 0
            && self.max_session_keys > 0
            && self.max_txs_per_account_per_block > 0
            && self.max_entries_per_block > 0
            && self.max_block_bytes > 0
            && self.max_pending_withdrawals > 0;
        if !caps_positive {
            return Err(E::ZeroCap);
        }
        if self.max_accounts < 3 {
            return Err(E::TooFewAccounts);
        }
        let block_bytes = self.max_block_bytes as usize;
        if block_bytes > 48_000 || block_bytes + 1_024 > MAX_BATCH_BYTES {
            return Err(E::BlockBytes);
        }
        if self.exec_cpu_limit > 400_000_000 {
            return Err(E::ExecCpuLimit);
        }
        if self.exec_mem_limit > 41_943_040 {
            return Err(E::ExecMemLimit);
        }
        if self.markets.is_empty() || self.markets.len() > MAX_MARKETS {
            return Err(E::MarketCount);
        }
        for m in &self.markets {
            let id = m.market_id;
            if m.tick <= 0 {
                return Err(E::Tick { market_id: id });
            }
            if !(0 < m.mmf_bps && m.mmf_bps < m.imf_bps && m.imf_bps <= 10_000) {
                return Err(E::MarginBps { market_id: id });
            }
            if m.taker_fee_bps > 1_000 || m.maker_fee_bps > 1_000 || m.liq_fee_bps > 1_000 {
                return Err(E::FeeBps { market_id: id });
            }
            if m.liq_fee_bps > m.mmf_bps {
                return Err(E::LiqFeeAboveMaintenance { market_id: id });
            }
            if !(0 < m.band_bps && m.band_bps <= 5_000) {
                return Err(E::BandBps { market_id: id });
            }
            if !(0 < m.max_position_lots && m.max_position_lots <= m.max_oi_lots) {
                return Err(E::PositionLimits { market_id: id });
            }
            if m.impact_lots <= 0 {
                return Err(E::ImpactLots { market_id: id });
            }
            if !m.symbol.is_ascii() {
                return Err(E::SymbolNotAscii { market_id: id });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn symbol(s: &str) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[..s.len()].copy_from_slice(s.as_bytes());
        out
    }

    /// The §10.3 testnet lane with placeholder keys.
    pub(crate) fn testnet_like() -> GenesisConfigV1 {
        let market = |id, sym, lot, dec, imf, mmf, pos, oi, impact| MarketParamsV1 {
            market_id: id,
            symbol: symbol(sym),
            tick: 1000,
            imf_bps: imf,
            mmf_bps: mmf,
            taker_fee_bps: 5,
            maker_fee_bps: 0,
            liq_fee_bps: 100,
            band_bps: 500,
            max_position_lots: pos,
            max_oi_lots: oi,
            impact_lots: impact,
            display_lot_base_units: lot,
            display_base_decimals: dec,
        };
        GenesisConfigV1 {
            lane_id: [0x11; 32],
            backstop_key: [0x21; 32],
            treasury_key: [0x22; 32],
            access_mode: AccessMode::Open,
            allowlist: Vec::new(),
            oracle_keys: alloc::vec![[0x31; 32]],
            oracle_max_staleness_ms: 30_000,
            oracle_max_future_ms: 5_000,
            oracle_circuit_breaker_bps: 1_000,
            oracle_breaker_bps_per_sec: 10,
            funding_interval_ms: 3_600_000,
            funding_damping: 8,
            funding_max_rate_ppm: 500,
            insurance_fee_share_bps: 3_000,
            min_deposit: 10_000_000,
            min_withdrawal: 10_000_000,
            max_accounts: 1024,
            max_orders_per_side: 256,
            max_open_orders_per_account: 32,
            max_session_keys: 4,
            max_txs_per_account_per_block: 50,
            max_entries_per_block: 256,
            max_block_bytes: 24_000,
            max_pending_withdrawals: 512,
            exec_cpu_limit: 400_000_000,
            exec_mem_limit: 41_943_040,
            markets: alloc::vec![
                market(1, "BTC-PERP", 10_000, 8, 1000, 500, 20_000, 200_000, 1_000),
                market(2, "ETH-PERP", 100_000, 8, 1000, 500, 50_000, 500_000, 2_000),
                market(
                    3,
                    "XLM-PERP",
                    100_000_000,
                    7,
                    2000,
                    1000,
                    100_000,
                    1_000_000,
                    1_000
                ),
            ],
        }
    }

    #[test]
    fn testnet_config_round_trips_and_validates() {
        let cfg = testnet_like();
        assert_eq!(cfg.validate(), Ok(()));
        let bytes = cfg.encode().unwrap();
        // 8+32×3+1+2+1+32 fixed-ish head, 3 markets × 71 bytes at the end.
        assert_eq!(&bytes[..8], b"CVGENES1");
        assert_eq!(
            bytes.len(),
            8 + 96
                + 1
                + 2
                + 1
                + 32
                + 8
                + 8
                + 2
                + 2
                + 8
                + 2
                + 4
                + 2
                + 16
                + 16
                + 4
                + 4
                + 2
                + 1
                + 2
                + 4
                + 4
                + 4
                + 8
                + 8
                + 2
                + 3 * MARKET_PARAMS_LEN
        );
        assert_eq!(GenesisConfigV1::decode(&bytes), Ok(cfg.clone()));
        assert_eq!(cfg.markets[2].symbol_str(), "XLM-PERP");
    }

    #[test]
    fn strict_decoding() {
        let bytes = testnet_like().encode().unwrap();
        let mut bad_mode = bytes.clone();
        bad_mode[104] = 2;
        assert_eq!(
            GenesisConfigV1::decode(&bad_mode),
            Err(DecodeError::BadEnum)
        );
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(
            GenesisConfigV1::decode(&trailing),
            Err(DecodeError::TrailingBytes)
        );
        assert_eq!(
            GenesisConfigV1::decode(&bytes[..bytes.len() - 1]),
            Err(DecodeError::UnexpectedEnd)
        );
    }

    #[test]
    fn every_genesis_rule_rejects() {
        use ConfigError as E;
        let check = |f: &dyn Fn(&mut GenesisConfigV1), want: ConfigError| {
            let mut c = testnet_like();
            f(&mut c);
            assert_eq!(c.validate(), Err(want));
        };
        check(&|c| c.treasury_key = c.backstop_key, E::SystemKeysEqual);
        check(
            &|c| c.allowlist = alloc::vec![[2; 32], [1; 32]],
            E::AllowlistNotAscending,
        );
        check(
            &|c| c.allowlist = alloc::vec![[1; 32], [1; 32]],
            E::AllowlistNotAscending,
        );
        check(&|c| c.oracle_keys.clear(), E::NoOracleKeys);
        check(
            &|c| c.oracle_keys = alloc::vec![[5; 32], [4; 32]],
            E::OracleKeysNotAscending,
        );
        check(&|c| c.markets[1].market_id = 1, E::MarketIdsNotAscending);
        check(&|c| c.funding_interval_ms = 59_999, E::FundingInterval);
        check(&|c| c.funding_damping = 0, E::FundingDamping);
        check(&|c| c.funding_max_rate_ppm = 10_001, E::FundingMaxRate);
        check(&|c| c.insurance_fee_share_bps = 10_001, E::InsuranceShare);
        check(&|c| c.oracle_circuit_breaker_bps = 0, E::CircuitBreaker);
        check(&|c| c.oracle_max_staleness_ms = 999, E::OracleStaleness);
        check(&|c| c.min_deposit = 0, E::MinDeposit);
        check(&|c| c.min_withdrawal = 0, E::MinWithdrawal);
        check(&|c| c.max_orders_per_side = 0, E::ZeroCap);
        check(&|c| c.max_session_keys = 0, E::ZeroCap);
        check(&|c| c.max_pending_withdrawals = 0, E::ZeroCap);
        check(&|c| c.max_accounts = 2, E::TooFewAccounts);
        check(&|c| c.max_block_bytes = 48_001, E::BlockBytes);
        check(&|c| c.exec_cpu_limit = 400_000_001, E::ExecCpuLimit);
        check(&|c| c.exec_mem_limit = 41_943_041, E::ExecMemLimit);
        check(&|c| c.markets.clear(), E::MarketCount);
        check(
            &|c| {
                let m = c.markets[0];
                c.markets = (1..=5)
                    .map(|i| MarketParamsV1 { market_id: i, ..m })
                    .collect();
            },
            E::MarketCount,
        );
        check(&|c| c.markets[0].tick = 0, E::Tick { market_id: 1 });
        check(&|c| c.markets[0].mmf_bps = 0, E::MarginBps { market_id: 1 });
        check(
            &|c| c.markets[0].mmf_bps = 1000,
            E::MarginBps { market_id: 1 },
        );
        check(
            &|c| c.markets[0].imf_bps = 10_001,
            E::MarginBps { market_id: 1 },
        );
        check(
            &|c| c.markets[1].taker_fee_bps = 1_001,
            E::FeeBps { market_id: 2 },
        );
        check(
            &|c| c.markets[1].liq_fee_bps = 501,
            E::LiqFeeAboveMaintenance { market_id: 2 },
        );
        check(
            &|c| c.markets[2].band_bps = 5_001,
            E::BandBps { market_id: 3 },
        );
        check(&|c| c.markets[2].band_bps = 0, E::BandBps { market_id: 3 });
        check(
            &|c| c.markets[2].max_position_lots = c.markets[2].max_oi_lots + 1,
            E::PositionLimits { market_id: 3 },
        );
        check(
            &|c| c.markets[2].max_position_lots = 0,
            E::PositionLimits { market_id: 3 },
        );
        check(
            &|c| c.markets[0].impact_lots = 0,
            E::ImpactLots { market_id: 1 },
        );
        check(
            &|c| c.markets[0].symbol[15] = 0xFF,
            E::SymbolNotAscii { market_id: 1 },
        );
    }

    #[test]
    fn boundaries_are_inclusive_where_the_spec_says_so() {
        let mut c = testnet_like();
        c.funding_interval_ms = 60_000;
        c.funding_max_rate_ppm = 10_000;
        c.max_accounts = 3;
        c.max_block_bytes = 48_000;
        c.exec_cpu_limit = 400_000_000;
        c.markets[0].band_bps = 5_000;
        c.markets[0].imf_bps = 10_000;
        c.markets[0].liq_fee_bps = c.markets[0].mmf_bps;
        assert_eq!(c.validate(), Ok(()));
    }
}
