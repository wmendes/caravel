//! A payments lane file (DEC-054) to `AppGenesisV1` (spec §20.4.1): the
//! generic sections, plus `[payments] treasury_key, transfer_fee, min_transfer`.

use anyhow::{anyhow, bail, Context, Result};
use caravel_app_sdk::{AccessMode, AppEngine, AppGenesisV1};
use caravel_node::lane_toml::{parse_account, sorted_keys, LaneFile};
use caravel_payments::{Params, Payments};
use serde::Deserialize;

/// `[payments]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaymentsSection {
    /// System account 0: receives the transfer fees.
    pub treasury_key: String,
    /// USDC stroops per transfer, paid by the sender; 0 for free transfers.
    pub transfer_fee: i64,
    pub min_transfer: i64,
}

pub fn genesis_config(file: &LaneFile) -> Result<AppGenesisV1> {
    if file.app.is_none() {
        bail!("a payments lane file needs [app] template = \"payments\"");
    }
    let access = file.access()?;
    let l = file.limits()?;
    let p: PaymentsSection = file.app_section()?;
    let access_mode = match access.mode.as_str() {
        "open" => AccessMode::Open,
        "allowlist" => AccessMode::Allowlist,
        other => bail!("access.mode must be \"open\" or \"allowlist\", got {other:?}"),
    };
    let params = Params {
        transfer_fee: i128::from(p.transfer_fee),
        min_transfer: i128::from(p.min_transfer),
    };
    let cfg = AppGenesisV1 {
        lane_id: file.lane_id(),
        template: Payments::TEMPLATE,
        template_version: 1,
        system_keys: vec![parse_account(&p.treasury_key).context("[payments] treasury_key")?],
        access_mode,
        allowlist: sorted_keys("access.allowlist", &access.allowlist)?,
        min_deposit: i128::from(l.min_deposit),
        min_withdrawal: i128::from(l.min_withdrawal),
        max_accounts: l.max_accounts,
        max_session_keys: l.max_session_keys,
        max_txs_per_account_per_block: l.max_txs_per_account_per_block,
        max_entries_per_block: l.max_entries_per_block,
        max_block_bytes: l.max_block_bytes,
        max_pending_withdrawals: l.max_pending_withdrawals,
        exec_cpu_limit: l.exec_cpu_limit,
        exec_mem_limit: l.exec_mem_limit,
        app_params: params.encode(),
    };
    cfg.validate()
        .map_err(|e| anyhow!("config breaks a genesis rule (spec §20.4.1): {e:?}"))?;
    Payments::params(&cfg.app_params)
        .ok_or_else(|| anyhow!("[payments] needs transfer_fee ≥ 0 and min_transfer ≥ 1"))?;
    Ok(cfg)
}
