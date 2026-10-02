//! The lane file (`lanes/*/config/lane.*.toml`, spec §10.3, DEC-054).
//!
//! Its generic sections are the platform's:
//! - `[lane] name`;
//! - `[app] template, engine_wasm_sha256`;
//! - `[node]` (not consensus);
//! - `[access]` and `[limits]`.
//!
//! The app's own settings are in a table named after the template
//! (`[perps.*]`). The app turns the file into its genesis config
//! (`NodeApp::genesis_config`). A file without `[app]` is an M0 file, which
//! only the app that wrote it reads. Every node, and the replay CLI, derives
//! the consensus config from the file with this code, so they all hash the
//! same bytes (spec §16.2).
//!
//! `[env.<name>]` tables describe deployments (where and how a lane runs).
//! They are removed before anything else reads the file, so they never reach
//! genesis or the app's parser.

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use caravel_core::preimage::lane_id_preimage;
use caravel_runtime::checkpoint::sha256;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::app::NodeApp;

/// The sections every lane file has.
const GENERIC: [&str; 5] = ["lane", "app", "node", "access", "limits"];

/// The deployment tables, `[env.<name>]`; also a reserved template name.
pub const ENV: &str = "env";

#[derive(Clone, Debug)]
pub struct LaneFile {
    pub lane: LaneSection,
    /// `None` for an M0 file.
    pub app: Option<AppSection>,
    pub node: NodeSection,
    /// The whole file without `[env]`, for the app's parser.
    pub raw: toml::Table,
    /// `[env.<name>]`: one table per deployment. Not consensus.
    pub env: toml::Table,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneSection {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSection {
    /// The app this lane runs; its node binary refuses any other.
    pub template: String,
    /// The engine Wasm of record; the node refuses to run another.
    pub engine_wasm_sha256: String,
}

/// Node settings: not consensus, not hashed (spec §10.1).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeSection {
    pub block_time_ms: u64,
    pub checkpoint_every_blocks: u64,
    pub max_batch_bytes: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Access {
    /// "open" or "allowlist".
    pub mode: String,
    pub allowlist: Vec<String>,
}

/// The platform's limits: accounts, deposits and withdrawals, session keys,
/// blocks and execution.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub min_deposit: i64,
    pub min_withdrawal: i64,
    pub max_accounts: u32,
    pub max_session_keys: u8,
    pub max_txs_per_account_per_block: u16,
    pub max_entries_per_block: u32,
    pub max_block_bytes: u32,
    pub max_pending_withdrawals: u32,
    pub exec_cpu_limit: u64,
    pub exec_mem_limit: u64,
}

/// A `G...` account strkey as its raw ed25519 key.
pub fn parse_account(s: &str) -> Result<[u8; 32]> {
    stellar_strkey::ed25519::PublicKey::from_string(s)
        .map(|k| k.0)
        .map_err(|e| anyhow!("{s:?} is not a G... account key: {e:?}"))
}

/// Parses keys and sorts them ascending, as §10.2 requires; duplicates are an error.
pub fn sorted_keys(what: &str, keys: &[String]) -> Result<Vec<[u8; 32]>> {
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

fn section<T: DeserializeOwned>(raw: &toml::Table, name: &str) -> Result<T> {
    raw.get(name)
        .ok_or_else(|| anyhow!("missing [{name}]"))?
        .clone()
        .try_into()
        .with_context(|| format!("[{name}]"))
}

impl LaneFile {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut raw: toml::Table = toml::from_str(text)?;
        let env = match raw.remove(ENV) {
            None => toml::Table::new(),
            Some(toml::Value::Table(t)) => t,
            Some(_) => bail!("[{ENV}] must be a table of deployments, [{ENV}.<name>]"),
        };
        let app: Option<AppSection> = raw.get("app").map(|_| section(&raw, "app")).transpose()?;
        if let Some(app) = &app {
            if app.template == ENV {
                bail!("[app] template {ENV:?} is reserved for the deployment tables");
            }
            if let Some(extra) = raw
                .keys()
                .find(|k| !GENERIC.contains(&k.as_str()) && **k != app.template)
            {
                bail!(
                    "unknown section [{extra}]: a {} lane file has {} and [{}]",
                    app.template,
                    GENERIC.map(|g| format!("[{g}]")).join(", "),
                    app.template
                );
            }
        }
        Ok(Self {
            lane: section(&raw, "lane")?,
            node: section(&raw, "node")?,
            app,
            raw,
            env,
        })
    }

    /// `[access]`.
    pub fn access(&self) -> Result<Access> {
        section(&self.raw, "access")
    }

    /// `[limits]`.
    pub fn limits(&self) -> Result<Limits> {
        section(&self.raw, "limits")
    }

    /// The app's own table, `[<template>]`.
    pub fn app_section<T: DeserializeOwned>(&self) -> Result<T> {
        let app = self
            .app
            .as_ref()
            .ok_or_else(|| anyhow!("the lane file has no [app] section"))?;
        section(&self.raw, &app.template)
    }

    /// `[app] engine_wasm_sha256`, if the file names one.
    pub fn engine_wasm_hash(&self) -> Result<Option<[u8; 32]>> {
        self.app
            .as_ref()
            .map(|a| crate::node_config::parse_hash(&a.engine_wasm_sha256))
            .transpose()
            .context("[app] engine_wasm_sha256")
    }

    /// Refuses an engine Wasm the lane file does not name.
    pub fn check_engine(&self, engine_wasm_hash: &[u8; 32]) -> Result<()> {
        match self.engine_wasm_hash()? {
            Some(want) if want != *engine_wasm_hash => bail!(
                "the lane file's [app] engine_wasm_sha256 is {}, but the node runs {}",
                hex(&want),
                hex(engine_wasm_hash)
            ),
            _ => Ok(()),
        }
    }

    /// `lane_id = H(TAG_LANE_ID || utf8(name))` (spec §9.1).
    pub fn lane_id(&self) -> [u8; 32] {
        sha256(&lane_id_preimage(&self.lane.name))
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
        if n.max_batch_bytes as usize > caravel_core::batch::MAX_BATCH_BYTES {
            bail!(
                "node.max_batch_bytes must be at most {}",
                caravel_core::batch::MAX_BATCH_BYTES
            );
        }
        Ok(())
    }

    /// Refuses a lane file for another app.
    pub fn check_template<A: NodeApp>(&self) -> Result<()> {
        match &self.app {
            Some(a) if a.template != A::TEMPLATE => bail!(
                "the lane file runs template {:?}; this node runs {:?}",
                a.template,
                A::TEMPLATE
            ),
            _ => Ok(()),
        }
    }
}

/// What `genesis` reports.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct GenesisReport {
    pub lane_name: String,
    pub lane_id: String,
    /// `H(genesis config bytes)`.
    pub config_hash: String,
    /// `H(genesis state bytes)`.
    pub genesis_state_hash: String,
    pub config_bytes: usize,
    pub state_bytes: usize,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Builds the genesis config and state from a lane file, with the app's
/// native genesis. Nodes check the Wasm gives the same state before they run.
pub fn genesis<A: NodeApp>(app: &A, file: &LaneFile) -> Result<(GenesisReport, Vec<u8>, Vec<u8>)> {
    file.check_template::<A>()?;
    file.check_node_settings()?;
    let config_bytes = app.genesis_config(file)?;
    let state = app
        .native_genesis(&config_bytes)
        .map_err(|f| anyhow!("engine genesis failed: {f:?}"))?;
    let report = GenesisReport {
        lane_name: file.lane.name.clone(),
        lane_id: hex(&file.lane_id()),
        config_hash: hex(&sha256(&config_bytes)),
        genesis_state_hash: hex(&sha256(&state)),
        config_bytes: config_bytes.len(),
        state_bytes: state.len(),
    };
    Ok((report, config_bytes, state))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"
[lane]
name = "demo-0"

[app]
template = "demo"
engine_wasm_sha256 = "0101010101010101010101010101010101010101010101010101010101010101"

[node]
block_time_ms = 1000
checkpoint_every_blocks = 10
max_batch_bytes = 96000

[access]
mode = "open"
allowlist = []

[limits]
min_deposit = 10000000
min_withdrawal = 10000000
max_accounts = 256
max_session_keys = 4
max_txs_per_account_per_block = 50
max_entries_per_block = 256
max_block_bytes = 12000
max_pending_withdrawals = 512
exec_cpu_limit = 200000000
exec_mem_limit = 41943040

[demo]
greeting = "hi"
"#;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Demo {
        greeting: String,
    }

    #[test]
    fn reads_the_generic_sections_and_the_app_s() {
        let f = LaneFile::parse(FILE).unwrap();
        assert_eq!(f.lane.name, "demo-0");
        assert_eq!(f.app.as_ref().unwrap().template, "demo");
        assert_eq!(f.engine_wasm_hash().unwrap(), Some([1; 32]));
        assert!(f.check_engine(&[1; 32]).is_ok());
        assert!(f.check_engine(&[2; 32]).is_err());
        assert_eq!(f.limits().unwrap().max_pending_withdrawals, 512);
        assert_eq!(f.access().unwrap().mode, "open");
        assert_eq!(f.app_section::<Demo>().unwrap().greeting, "hi");
        assert!(f.check_node_settings().is_ok());
        assert_eq!(f.lane_id(), sha256(&lane_id_preimage("demo-0")));
    }

    #[test]
    fn rejects_bad_files() {
        let bad = |t: String| LaneFile::parse(&t).and_then(|f| f.check_node_settings());
        assert!(
            bad(format!("{FILE}\n[extra]\nx = 1\n")).is_err(),
            "unknown sections"
        );
        assert!(bad(FILE.replace("block_time_ms = 1000", "block_time_ms = 100")).is_err());
        assert!(bad(FILE.replace("max_batch_bytes = 96000", "max_batch_bytes = 200000")).is_err());
        assert!(
            bad(FILE.replace("template = \"demo\"\n", "")).is_err(),
            "[app] needs a template"
        );
        assert!(LaneFile::parse(&FILE.replace("max_accounts = 256\n", ""))
            .unwrap()
            .limits()
            .is_err());
        assert!(LaneFile::parse(&FILE.replace("greeting", "greeting2"))
            .unwrap()
            .app_section::<Demo>()
            .is_err());
    }

    #[test]
    fn env_tables_are_set_aside() {
        let with_env = format!(
            "{FILE}\n[env.testnet]\nnetwork = \"testnet\"\n[[env.testnet.validators]]\nname = \"v1\"\n[env.local]\nnetwork = \"local\"\n"
        );
        let f = LaneFile::parse(&with_env).unwrap();
        let plain = LaneFile::parse(FILE).unwrap();
        assert_eq!(f.raw, plain.raw, "[env] never reaches the app's parser");
        assert!(plain.env.is_empty());
        assert_eq!(f.env.keys().collect::<Vec<_>>(), ["local", "testnet"]);
        assert_eq!(f.env["testnet"]["network"].as_str(), Some("testnet"));
        // `env` must be a table, and is no template name.
        assert!(LaneFile::parse(&format!("env = 1\n{FILE}")).is_err());
        let reserved = FILE
            .replace("template = \"demo\"", "template = \"env\"")
            .replace("[demo]\ngreeting = \"hi\"\n", "");
        assert!(LaneFile::parse(&reserved).is_err());
    }

    #[test]
    fn an_m0_file_has_no_app_section() {
        let start = FILE.find("[app]").unwrap();
        let end = FILE.find("[node]").unwrap();
        let f = LaneFile::parse(&format!("{}{}", &FILE[..start], &FILE[end..])).unwrap();
        assert!(f.app.is_none());
        assert_eq!(f.engine_wasm_hash().unwrap(), None);
        assert!(f.app_section::<Demo>().is_err());
    }
}
