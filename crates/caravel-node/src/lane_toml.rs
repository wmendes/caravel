//! The lane file (`config/lane.*.toml`, spec §10.3) and its conversion to
//! `GenesisConfigV1` (spec §10.2).
//!
//! Every node, and the replay CLI, derives the consensus config from the TOML
//! with this code, so they all hash the same bytes (spec §16.2).

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use caravel_types::config::{AccessMode, GenesisConfigV1, MarketParamsV1};
use caravel_types::preimage::lane_id_preimage;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneFile {
    pub lane: LaneSection,
    pub node: NodeSection,
    pub accounts: Accounts,
    pub access: Access,
    pub oracle: Oracle,
    pub funding: Funding,
    pub fees: Fees,
    pub limits: Limits,
    pub markets: Vec<Market>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneSection {
    pub name: String,
}

/// Node settings: not consensus, not hashed (spec §10.1).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeSection {
    pub block_time_ms: u64,
    pub checkpoint_every_blocks: u64,
    pub max_batch_bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Accounts {
    pub backstop_key: String,
    pub treasury_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Access {
    pub mode: String,
    pub allowlist: Vec<String>,
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub min_deposit: i64,
    pub min_withdrawal: i64,
    pub max_accounts: u32,
    pub max_orders_per_side: u32,
    pub max_open_orders_per_account: u16,
    pub max_session_keys: u8,
    pub max_txs_per_account_per_block: u16,
    pub max_entries_per_block: u32,
    pub max_block_bytes: u32,
    pub max_pending_withdrawals: u32,
    pub exec_cpu_limit: u64,
    pub exec_mem_limit: u64,
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

/// A `G...` account strkey as its raw ed25519 key.
pub fn parse_account(s: &str) -> Result<[u8; 32]> {
    stellar_strkey::ed25519::PublicKey::from_string(s)
        .map(|k| k.0)
        .map_err(|e| anyhow!("{s:?} is not a G... account key: {e:?}"))
}

/// Parses keys and sorts them ascending, as §10.2 requires; duplicates are an error.
fn sorted_keys(what: &str, keys: &[String]) -> Result<Vec<[u8; 32]>> {
    let mut out = keys
        .iter()
        .map(|k| parse_account(k))
        .collect::<Result<Vec<_>>>()?;
    out.sort();
    if out.windows(2).any(|w| w[0] == w[1]) {
        bail!("{what} lists the same key twice");
    }
    Ok(out)
}

fn symbol(s: &str) -> Result<[u8; 16]> {
    if !s.is_ascii() || s.len() > 16 {
        bail!("market symbol {s:?} must be ASCII and at most 16 bytes");
    }
    let mut out = [0u8; 16];
    out[..s.len()].copy_from_slice(s.as_bytes());
    Ok(out)
}

impl LaneFile {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// `lane_id = H(TAG_LANE_ID || utf8(name))` (spec §9.1).
    pub fn lane_id(&self) -> [u8; 32] {
        caravel_perps::Crypto::sha256(
            &caravel_perps::native::NativeCrypto,
            &lane_id_preimage(&self.lane.name),
        )
    }

    /// Checks the node settings (not consensus).
    pub fn check_node_settings(&self) -> Result<()> {
        let n = &self.node;
        if !(200..=5000).contains(&n.block_time_ms) {
            bail!(
                "node.block_time_ms must be in 200..=5000, got {}",
                n.block_time_ms
            );
        }
        if n.checkpoint_every_blocks == 0 {
            bail!("node.checkpoint_every_blocks must be at least 1");
        }
        if n.max_batch_bytes as usize > caravel_types::batch::MAX_BATCH_BYTES {
            bail!(
                "node.max_batch_bytes must be at most {}",
                caravel_types::batch::MAX_BATCH_BYTES
            );
        }
        Ok(())
    }

    /// The consensus config. It is validated against every §10.2 rule.
    pub fn genesis_config(&self) -> Result<GenesisConfigV1> {
        let access_mode = match self.access.mode.as_str() {
            "open" => AccessMode::Open,
            "allowlist" => AccessMode::Allowlist,
            other => bail!("access.mode must be \"open\" or \"allowlist\", got {other:?}"),
        };
        let l = &self.limits;
        let markets = self
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
            lane_id: self.lane_id(),
            backstop_key: parse_account(&self.accounts.backstop_key)?,
            treasury_key: parse_account(&self.accounts.treasury_key)?,
            access_mode,
            allowlist: sorted_keys("access.allowlist", &self.access.allowlist)?,
            oracle_keys: sorted_keys("oracle.keys", &self.oracle.keys)?,
            oracle_max_staleness_ms: self.oracle.max_staleness_ms,
            oracle_max_future_ms: self.oracle.max_future_ms,
            oracle_circuit_breaker_bps: self.oracle.circuit_breaker_bps,
            oracle_breaker_bps_per_sec: self.oracle.breaker_bps_per_sec,
            funding_interval_ms: self.funding.interval_ms,
            funding_damping: self.funding.damping,
            funding_max_rate_ppm: self.funding.max_rate_ppm,
            insurance_fee_share_bps: self.fees.insurance_share_bps,
            min_deposit: i128::from(l.min_deposit),
            min_withdrawal: i128::from(l.min_withdrawal),
            max_accounts: l.max_accounts,
            max_orders_per_side: l.max_orders_per_side,
            max_open_orders_per_account: l.max_open_orders_per_account,
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
}

/// What `caravel-node genesis` reports.
#[derive(Debug, serde::Serialize)]
pub struct GenesisReport {
    pub lane_name: String,
    pub lane_id: String,
    /// `H(GenesisConfigV1 bytes)`.
    pub config_hash: String,
    /// `H(genesis StateV1 bytes)`.
    pub genesis_state_hash: String,
    pub config_bytes: usize,
    pub state_bytes: usize,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Builds the genesis config and state from a lane file.
pub fn genesis(file: &LaneFile) -> Result<(GenesisReport, Vec<u8>, Vec<u8>)> {
    file.check_node_settings()?;
    let cfg = file.genesis_config()?;
    let config_bytes = cfg
        .encode()
        .map_err(|_| anyhow!("config does not encode"))?;
    let crypto = caravel_perps::native::NativeCrypto;
    let state = caravel_perps::genesis(&config_bytes, &crypto)
        .map_err(|f| anyhow!("engine genesis failed: {f:?}"))?;
    let sha = |b: &[u8]| caravel_perps::Crypto::sha256(&crypto, b);
    let report = GenesisReport {
        lane_name: file.lane.name.clone(),
        lane_id: hex(&cfg.lane_id),
        config_hash: hex(&sha(&config_bytes)),
        genesis_state_hash: hex(&sha(&state)),
        config_bytes: config_bytes.len(),
        state_bytes: state.len(),
    };
    Ok((report, config_bytes, state))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn testnet() -> LaneFile {
        LaneFile::load(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../config/lane.caravel-perps.testnet.toml"),
        )
        .unwrap()
    }

    #[test]
    fn testnet_file_builds_a_valid_genesis() {
        let (report, config, state) = genesis(&testnet()).unwrap();
        assert_eq!(report.lane_name, "caravel-perps-testnet-0");
        assert_eq!(
            report.lane_id,
            "319b46e9804746739b1e177fae8d79df8f013ec86dde16eaff6c83b374531047"
        );
        assert_eq!(report.config_bytes, config.len());
        assert_eq!(report.state_bytes, state.len());
    }

    /// The TOML and the §10.3 fixture in caravel-types agree on every number;
    /// only the keys and the benchmarked caps differ.
    #[test]
    fn testnet_numbers_match_the_section_10_3_fixture() {
        let mut from_toml = testnet().genesis_config().unwrap();
        let mut fixture = caravel_types::vectors::config();
        // The codec fixture keeps the v0.1.0 caps; the lane runs the caps the
        // T-005 benchmark chose (DEC-028).
        fixture.max_accounts = 256;
        fixture.max_orders_per_side = 128;
        fixture.max_block_bytes = 12_000;
        fixture.exec_cpu_limit = 200_000_000;
        from_toml.backstop_key = fixture.backstop_key;
        from_toml.treasury_key = fixture.treasury_key;
        from_toml.oracle_keys = fixture.oracle_keys.clone();
        assert_eq!(from_toml, fixture);
    }

    #[test]
    fn rejects_bad_files() {
        let text = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../config/lane.caravel-perps.testnet.toml"),
        )
        .unwrap();
        let parse = |t: &str| {
            toml::from_str::<LaneFile>(t)
                .map_err(anyhow::Error::from)
                .and_then(|f| genesis(&f).map(|_| ()))
        };
        assert!(parse(&text.replace("mode = \"open\"", "mode = \"closed\"")).is_err());
        assert!(parse(&text.replace("block_time_ms = 1000", "block_time_ms = 100")).is_err());
        assert!(
            parse(&text.replace("imf_bps = 2000", "imf_bps = 900")).is_err(),
            "mmf 1000 must be below imf"
        );
        assert!(parse(&text.replace(
            "GAG5OK7U7GLFB2BQWIZPNSPVOTITXV3AJBQAHZJWT6RFABBONNLJES7U",
            "GBAD"
        ))
        .is_err());
        assert!(
            parse(&format!("{text}\n[extra]\nx = 1\n")).is_err(),
            "unknown sections are rejected"
        );
    }
}
