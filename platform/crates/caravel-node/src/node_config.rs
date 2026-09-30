//! Node configuration (not consensus): where the lane file, engine Wasm and
//! database are, which network and settlement contract checkpoints are for,
//! and the validator set. Secrets never live in these files: the internal API
//! token and signing keys come from environment variables or key files.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use caravel_runtime::checkpoint::{network_id, settlement_addr_hash, HeaderIds};
use serde::Deserialize;

use crate::lane_toml::{parse_account, LaneFile};

/// The testnet passphrase (spec §3.1). Mainnet is refused (spec §0.3 rule 6).
pub const TESTNET_PASSPHRASE: &str = "Test SDF Network ; September 2015";
const MAINNET_PASSPHRASE: &str = "Public Global Stellar Network ; September 2015";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SequencerFile {
    pub sequencer: SequencerSection,
    #[serde(default)]
    pub signers: Option<SignersSection>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SequencerSection {
    /// `host:port` for the public and internal API.
    pub listen: String,
    /// The lane TOML (spec §10.3), relative to this file.
    pub lane: PathBuf,
    /// The engine Wasm of record (DEC-020, DEC-033), relative to this file.
    pub engine_wasm: PathBuf,
    /// The sha256 it must have: `engine_wasm_hash` in the settlement config.
    pub engine_wasm_sha256: String,
    /// SQLite file, relative to this file.
    pub db: PathBuf,
    pub network_passphrase: String,
    /// `C...` settlement contract; its address is bound into every header.
    pub settlement_contract: String,
    /// Name of the environment variable that holds the internal API bearer token.
    #[serde(default = "default_token_env")]
    pub internal_token_env: String,
    /// `wasm` (consensus) or `native` (debugging only; refused with `production`).
    #[serde(default = "default_executor")]
    pub executor: String,
    #[serde(default)]
    pub production: bool,
    #[serde(default = "default_mempool")]
    pub mempool_max: usize,
    #[serde(default = "default_mempool_per_account")]
    pub mempool_max_per_account: usize,
    /// Browser origins allowed to call the public API (CORS).
    #[serde(default)]
    pub cors_origins: Vec<String>,
}

fn default_token_env() -> String {
    "CARAVEL_INTERNAL_TOKEN".into()
}

fn default_executor() -> String {
    "wasm".into()
}

fn default_mempool() -> usize {
    10_000
}

fn default_mempool_per_account() -> usize {
    256
}

/// The validator set the settlement contract holds for `epoch` (spec §13.4).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignersSection {
    pub epoch: u64,
    pub threshold: u32,
    pub validators: Vec<ValidatorEntry>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ValidatorEntry {
    pub url: String,
    /// `G...` ed25519 key.
    pub key: String,
    #[serde(default = "one")]
    pub weight: u32,
}

fn one() -> u32 {
    1
}

/// A validator with its parsed key and its index in the key-sorted set.
#[derive(Debug, Clone)]
pub struct Validator {
    pub index: u32,
    pub url: String,
    pub key: [u8; 32],
    pub weight: u32,
}

#[derive(Debug, Clone)]
pub struct Signers {
    pub epoch: u64,
    pub threshold: u32,
    /// Sorted by key, so `index` is the contract's `signer_index`.
    pub validators: Vec<Validator>,
}

/// A loaded, checked sequencer configuration.
pub struct SequencerConfig {
    pub listen: String,
    pub lane: LaneFile,
    pub engine_wasm: PathBuf,
    pub engine_wasm_hash: [u8; 32],
    pub db: PathBuf,
    pub network_passphrase: String,
    pub settlement_contract: [u8; 32],
    pub internal_token: String,
    pub native: bool,
    pub production: bool,
    pub mempool_max: usize,
    pub mempool_max_per_account: usize,
    pub signers: Option<Signers>,
    pub cors_origins: Vec<String>,
}

impl SequencerConfig {
    /// Loads the config to run the sequencer (the internal token must be set).
    pub fn load(path: &Path) -> Result<Self> {
        Self::load_with(path, true)
    }

    /// Loads the config for offline tools such as `check-store`.
    pub fn load_offline(path: &Path) -> Result<Self> {
        Self::load_with(path, false)
    }

    fn load_with(path: &Path, need_token: bool) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let file: SequencerFile =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let dir = path.parent().unwrap_or(Path::new("."));
        let s = file.sequencer;
        check_network(&s.network_passphrase)?;
        let native = match s.executor.as_str() {
            "wasm" => false,
            "native" => true,
            other => bail!("executor must be \"wasm\" or \"native\", not {other:?}"),
        };
        if native && s.production {
            bail!("the native executor is for debugging only and is refused with production = true (spec §14.5)");
        }
        let internal_token = if need_token {
            let t = std::env::var(&s.internal_token_env)
                .map_err(|_| anyhow!("set {} to the internal API token", s.internal_token_env))?;
            if t.len() < 16 {
                bail!("{} must be at least 16 characters", s.internal_token_env);
            }
            t
        } else {
            String::new()
        };
        let lane = LaneFile::load(&dir.join(&s.lane))?;
        lane.check_node_settings()?;
        let signers = file.signers.map(parse_signers).transpose()?;
        Ok(Self {
            listen: s.listen,
            lane,
            engine_wasm: dir.join(s.engine_wasm),
            engine_wasm_hash: parse_hash(&s.engine_wasm_sha256)?,
            db: dir.join(s.db),
            network_passphrase: s.network_passphrase,
            settlement_contract: parse_contract(&s.settlement_contract)?,
            internal_token,
            native,
            production: s.production,
            mempool_max: s.mempool_max,
            mempool_max_per_account: s.mempool_max_per_account,
            signers,
            cors_origins: s.cors_origins,
        })
    }

    pub fn header_ids(&self) -> HeaderIds {
        HeaderIds {
            network_id: network_id(&self.network_passphrase),
            settlement_addr_hash: settlement_addr_hash(&self.settlement_contract),
            engine_wasm_hash: self.engine_wasm_hash,
        }
    }
}

pub fn check_network(passphrase: &str) -> Result<()> {
    if passphrase == MAINNET_PASSPHRASE {
        bail!("Caravel M0 runs on testnet only (spec §0.3 rule 6)");
    }
    Ok(())
}

pub fn parse_hash(s: &str) -> Result<[u8; 32]> {
    let v = caravel_runtime::sequencer::unhex(s).ok_or_else(|| anyhow!("{s:?} is not hex"))?;
    v.try_into().map_err(|_| anyhow!("{s:?} is not 32 bytes"))
}

pub fn parse_contract(s: &str) -> Result<[u8; 32]> {
    stellar_strkey::Contract::from_string(s)
        .map(|c| c.0)
        .map_err(|e| anyhow!("{s:?} is not a C... contract id: {e:?}"))
}

pub fn parse_signers(s: SignersSection) -> Result<Signers> {
    let mut validators = s
        .validators
        .into_iter()
        .map(|v| {
            Ok(Validator {
                index: 0,
                url: v.url.trim_end_matches('/').to_string(),
                key: parse_account(&v.key)?,
                weight: v.weight,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    validators.sort_by_key(|v| v.key);
    if validators.windows(2).any(|w| w[0].key == w[1].key) {
        bail!("the same validator key is listed twice");
    }
    for (i, v) in validators.iter_mut().enumerate() {
        v.index = i as u32;
    }
    let total: u64 = validators.iter().map(|v| u64::from(v.weight)).sum();
    if validators.is_empty() || s.threshold == 0 || u64::from(s.threshold) > total {
        bail!("signers: need 0 < threshold ≤ total weight");
    }
    Ok(Signers {
        epoch: s.epoch,
        threshold: s.threshold,
        validators,
    })
}
