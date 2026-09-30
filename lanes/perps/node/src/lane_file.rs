//! A perps lane file (spec §10.3, DEC-054) to `GenesisConfigV1` (spec §10.2).
//!
//! Two layouts give the same config bytes:
//! - the platform layout: the generic `[lane]`, `[app]`, `[node]`,
//!   `[access]`, `[limits]` and the perps settings under `[perps.*]`;
//! - the M0 layout, with no `[app]` and the perps sections at the top level,
//!   which lane #1's M0 nodes read.

use anyhow::{anyhow, bail, Result};
use caravel_node::lane_toml::{
    parse_account, sorted_keys, Access, LaneFile, LaneSection, Limits, NodeSection,
};
use caravel_types::config::{AccessMode, GenesisConfigV1, MarketParamsV1};
use serde::Deserialize;

/// `[perps]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerpsSection {
    pub accounts: Accounts,
    pub oracle: Oracle,
    pub funding: Funding,
    pub fees: Fees,
    pub limits: PerpsLimits,
    pub markets: Vec<Market>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Accounts {
    pub backstop_key: String,
    pub treasury_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Oracle {
    pub keys: Vec<String>,
    pub max_staleness_ms: u64,
    pub max_future_ms: u64,
    pub circuit_breaker_bps: u16,
    pub breaker_bps_per_sec: u16,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Funding {
    pub interval_ms: u64,
    pub damping: u16,
    pub max_rate_ppm: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fees {
    pub insurance_share_bps: u16,
}

/// `[perps.limits]`: the order book's.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerpsLimits {
    pub max_orders_per_side: u32,
    pub max_open_orders_per_account: u16,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Market {
    pub market_id: u16,
    pub symbol: String,
    pub display_lot_base_units: i64,
    pub display_base_decimals: u8,
    pub tick: i64,
    pub imf_bps: u16,
    pub mmf_bps: u16,
    pub taker_fee_bps: u16,
    pub maker_fee_bps: u16,
    pub liq_fee_bps: u16,
    pub band_bps: u16,
    pub max_position_lots: i64,
    pub max_oi_lots: i64,
    pub impact_lots: i64,
}

/// The M0 layout.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct M0File {
    #[allow(dead_code)]
    lane: LaneSection,
    #[allow(dead_code)]
    node: NodeSection,
    accounts: Accounts,
    access: Access,
    oracle: Oracle,
    funding: Funding,
    fees: Fees,
    limits: M0Limits,
    markets: Vec<Market>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct M0Limits {
    min_deposit: i64,
    min_withdrawal: i64,
    max_accounts: u32,
    max_orders_per_side: u32,
    max_open_orders_per_account: u16,
    max_session_keys: u8,
    max_txs_per_account_per_block: u16,
    max_entries_per_block: u32,
    max_block_bytes: u32,
    max_pending_withdrawals: u32,
    exec_cpu_limit: u64,
    exec_mem_limit: u64,
}

fn symbol(s: &str) -> Result<[u8; 16]> {
    if !s.is_ascii() || s.len() > 16 {
        bail!("market symbol {s:?} must be ASCII and at most 16 bytes");
    }
    let mut out = [0u8; 16];
    out[..s.len()].copy_from_slice(s.as_bytes());
    Ok(out)
}

/// The sections of either layout.
fn sections(file: &LaneFile) -> Result<(Access, Limits, PerpsSection)> {
    if file.app.is_some() {
        return Ok((file.access()?, file.limits()?, file.app_section()?));
    }
    let m0: M0File = file.raw.clone().try_into()?;
    let l = m0.limits;
    Ok((
        m0.access,
        Limits {
            min_deposit: l.min_deposit,
            min_withdrawal: l.min_withdrawal,
            max_accounts: l.max_accounts,
            max_session_keys: l.max_session_keys,
            max_txs_per_account_per_block: l.max_txs_per_account_per_block,
            max_entries_per_block: l.max_entries_per_block,
            max_block_bytes: l.max_block_bytes,
            max_pending_withdrawals: l.max_pending_withdrawals,
            exec_cpu_limit: l.exec_cpu_limit,
            exec_mem_limit: l.exec_mem_limit,
        },
        PerpsSection {
            accounts: m0.accounts,
            oracle: m0.oracle,
            funding: m0.funding,
            fees: m0.fees,
            limits: PerpsLimits {
                max_orders_per_side: l.max_orders_per_side,
                max_open_orders_per_account: l.max_open_orders_per_account,
            },
            markets: m0.markets,
        },
    ))
}

/// The consensus config. It is validated against every §10.2 rule.
pub fn genesis_config(file: &LaneFile) -> Result<GenesisConfigV1> {
    let (access, l, p) = sections(file)?;
    let access_mode = match access.mode.as_str() {
        "open" => AccessMode::Open,
        "allowlist" => AccessMode::Allowlist,
        other => bail!("access.mode must be \"open\" or \"allowlist\", got {other:?}"),
    };
    let markets = p
        .markets
        .iter()
        .map(|m| {
            Ok(MarketParamsV1 {
                market_id: m.market_id,
                symbol: symbol(&m.symbol)?,
                tick: m.tick,
                imf_bps: m.imf_bps,
                mmf_bps: m.mmf_bps,
                taker_fee_bps: m.taker_fee_bps,
                maker_fee_bps: m.maker_fee_bps,
                liq_fee_bps: m.liq_fee_bps,
                band_bps: m.band_bps,
                max_position_lots: m.max_position_lots,
                max_oi_lots: m.max_oi_lots,
                impact_lots: m.impact_lots,
                display_lot_base_units: m.display_lot_base_units,
                display_base_decimals: m.display_base_decimals,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let cfg = GenesisConfigV1 {
        lane_id: file.lane_id(),
        backstop_key: parse_account(&p.accounts.backstop_key)?,
        treasury_key: parse_account(&p.accounts.treasury_key)?,
        access_mode,
        allowlist: sorted_keys("access.allowlist", &access.allowlist)?,
        oracle_keys: sorted_keys("perps.oracle.keys", &p.oracle.keys)?,
        oracle_max_staleness_ms: p.oracle.max_staleness_ms,
        oracle_max_future_ms: p.oracle.max_future_ms,
        oracle_circuit_breaker_bps: p.oracle.circuit_breaker_bps,
        oracle_breaker_bps_per_sec: p.oracle.breaker_bps_per_sec,
        funding_interval_ms: p.funding.interval_ms,
        funding_damping: p.funding.damping,
        funding_max_rate_ppm: p.funding.max_rate_ppm,
        insurance_fee_share_bps: p.fees.insurance_share_bps,
        min_deposit: i128::from(l.min_deposit),
        min_withdrawal: i128::from(l.min_withdrawal),
        max_accounts: l.max_accounts,
        max_orders_per_side: p.limits.max_orders_per_side,
        max_open_orders_per_account: p.limits.max_open_orders_per_account,
        max_session_keys: l.max_session_keys,
        max_txs_per_account_per_block: l.max_txs_per_account_per_block,
        max_entries_per_block: l.max_entries_per_block,
        max_block_bytes: l.max_block_bytes,
        max_pending_withdrawals: l.max_pending_withdrawals,
        exec_cpu_limit: l.exec_cpu_limit,
        exec_mem_limit: l.exec_mem_limit,
        markets,
    };
    cfg.validate()
        .map_err(|e| anyhow!("config breaks a genesis rule (spec §10.2): {e:?}"))?;
    Ok(cfg)
}
