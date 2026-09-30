//! A deployment: one `[env.<name>]` table of a lane file (spec §20.3).
//!
//! The lane's own sections say what the lane is; an `[env.<name>]` table says
//! where it runs and who operates it. Keys are named, never written: every
//! key field is a Stellar CLI identity (`stellar keys`), and a secret key
//! anywhere in the table is refused.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use caravel_node::lane_toml::LaneFile;
use serde::Deserialize;

/// The networks a lane may run on. Mainnet is refused (testnet only).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    /// A local quickstart (`stellar container start local`).
    Local,
    Testnet,
}

impl Network {
    pub fn passphrase(self) -> &'static str {
        match self {
            Self::Local => "Standalone Network ; February 2017",
            Self::Testnet => "Test SDF Network ; September 2015",
        }
    }

    /// The RPC used when the table names none.
    pub fn default_rpc(self) -> &'static str {
        match self {
            Self::Local => "http://localhost:8000/rpc",
            Self::Testnet => crate::versions::testnet_rpc(),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Testnet => "testnet",
        }
    }
}

/// The token a lane settles in: what its settlement contract holds, and what
/// deposits, withdrawals and escapes pay. The contract works with any token
/// that has the SEP-41 interface (it only calls `transfer` and `balance`).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Token {
    /// A token the tool knows by name: `"circle-usdc"`, Circle's testnet USDC
    /// (`versions.json` `testnet.usdc_sac`).
    Named(String),
    /// A Stellar asset, `"CODE:ISSUER"`, through its Stellar Asset Contract;
    /// `apply` deploys that contract when the network has none yet (anyone may).
    Asset { asset: String },
    /// A SEP-41 token contract, `"C…"`.
    Contract { contract: String },
    /// A local network's test asset, `CODE:<admin>`, issued by the admin.
    Local { local: String },
}

/// Circle's testnet USDC.
pub const CIRCLE_USDC: &str = "circle-usdc";

/// An asset code: 1 to 12 letters or digits.
pub fn asset_code_ok(code: &str) -> bool {
    (1..=12).contains(&code.len()) && code.chars().all(|c| c.is_ascii_alphanumeric())
}

/// `CODE:ISSUER`.
pub fn parse_asset(asset: &str) -> Option<(String, [u8; 32])> {
    let (code, issuer) = asset.split_once(':')?;
    let issuer = stellar_strkey::ed25519::PublicKey::from_string(issuer)
        .ok()?
        .0;
    asset_code_ok(code).then(|| (code.to_string(), issuer))
}

/// The settlement constructor's `Params` (spec §13).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SettlementParams {
    pub force_inclusion_window_secs: u64,
    pub escape_timeout_secs: u64,
    pub min_rotation_delay_secs: u64,
    pub signer_retention_epochs: u32,
    /// Defaults to `[limits] min_deposit`.
    #[serde(default)]
    pub min_deposit: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ValidatorSpec {
    /// Names the node: `validator-<name>`, its config and its unit.
    pub name: String,
    /// The identity whose key signs checkpoints; it must be exportable,
    /// because the node on the host holds it.
    pub key: String,
    #[serde(default = "one")]
    pub weight: u32,
    /// Defaults to the sequencer's port + n for a validator named `n`, so a
    /// replacement never takes the port of the one it replaces.
    #[serde(default)]
    pub port: Option<u16>,
}

impl ValidatorSpec {
    /// Its port, given the sequencer's.
    pub fn port(&self, sequencer: u16) -> Option<u16> {
        self.port.or_else(|| {
            let n: u16 = self.name.parse().ok().filter(|n| (1..=999).contains(n))?;
            sequencer.checked_add(n)
        })
    }
}

fn one() -> u32 {
    1
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SequencerSpec {
    /// A validator named `n` listens on `port + n` unless it sets its own.
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub cors_origins: Vec<String>,
    #[serde(default)]
    pub production: bool,
}

fn default_port() -> u16 {
    8080
}

impl Default for SequencerSpec {
    fn default() -> Self {
        Self {
            port: default_port(),
            cors_origins: Vec::new(),
            production: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RelayerSpec {
    /// The identity that pays for checkpoint transactions.
    pub account: String,
    /// Env var → identity, for keys the app's feed modules read.
    #[serde(default)]
    pub feed_keys: BTreeMap<String, String>,
    /// The app's feed modules, passed to the relayer as they are; a relative
    /// `module` path is under the host's release (`relayer-feeds/<template>/…`).
    #[serde(default)]
    pub feeds: Vec<toml::Table>,
    /// The relayer's loop intervals, `{ inbox, checkpoints }` in ms.
    #[serde(default)]
    pub intervals_ms: Option<toml::Table>,
}

/// How often validators poll the sequencer and Stellar.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ValidatorPolling {
    #[serde(default = "default_sequencer_ms")]
    pub sequencer_ms: u64,
    #[serde(default = "default_stellar_secs")]
    pub stellar_secs: u64,
}

fn default_sequencer_ms() -> u64 {
    200
}

fn default_stellar_secs() -> u64 {
    10
}

impl Default for ValidatorPolling {
    fn default() -> Self {
        Self {
            sequencer_ms: default_sequencer_ms(),
            stellar_secs: default_stellar_secs(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Processes on this machine, under `.caravel/`.
    Local,
    /// A Linux host the team already has, over ssh.
    Ssh,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    #[default]
    Ssh,
    /// `gcloud compute ssh --tunnel-through-iap`; `address` is the VM name.
    GcloudIap,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HostSpec {
    pub provider: Provider,
    /// `user@host` for ssh, the VM name for gcloud-iap.
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub transport: Transport,
    /// gcloud-iap only.
    #[serde(default)]
    pub project: Option<String>,
    /// gcloud-iap only.
    #[serde(default)]
    pub zone: Option<String>,
    /// Where the lane's public API is served, e.g. `https://lane.example`.
    #[serde(default)]
    pub public_url: Option<String>,
    #[serde(default = "default_root")]
    pub root: String,
}

fn default_root() -> String {
    "/opt/caravel".into()
}

/// One `[env.<name>]` table.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EnvSpec {
    pub network: Network,
    #[serde(default)]
    pub rpc_url: Option<String>,
    /// The identity that deploys the settlement contract and holds its admin
    /// powers. It signs on this machine, so Ledger or the secure store work.
    pub admin: String,
    pub token: Token,
    /// A settlement contract deployed before this tool (lane #1); new lanes
    /// derive the address from the admin and the lane.
    #[serde(default)]
    pub settlement: Option<String>,
    /// The settlement build the contract runs, if not the build of record.
    #[serde(default)]
    pub settlement_wasm: Option<String>,
    pub threshold: u32,
    pub settlement_params: SettlementParams,
    pub validators: Vec<ValidatorSpec>,
    #[serde(default)]
    pub sequencer: SequencerSpec,
    #[serde(default)]
    pub validator_polling: ValidatorPolling,
    pub relayer: RelayerSpec,
    pub host: HostSpec,
}

/// A lane file and one of its deployments.
#[derive(Debug)]
pub struct Manifest {
    pub lane: LaneFile,
    pub env_name: String,
    pub env: EnvSpec,
}

impl Manifest {
    pub fn load(path: &Path, env: &str) -> Result<Self> {
        let lane = LaneFile::load(path)?;
        Self::from_lane(lane, env).with_context(|| format!("{} [env.{env}]", path.display()))
    }

    pub fn parse(text: &str, env: &str) -> Result<Self> {
        Self::from_lane(LaneFile::parse(text)?, env).with_context(|| format!("[env.{env}]"))
    }

    fn from_lane(lane: LaneFile, env: &str) -> Result<Self> {
        let table = lane.env.get(env).ok_or_else(|| {
            let known: Vec<_> = lane.env.keys().map(|k| format!("[env.{k}]")).collect();
            if known.is_empty() {
                anyhow!("the lane file has no deployments; add an [env.{env}] table")
            } else {
                anyhow!("no [env.{env}]; the file has {}", known.join(", "))
            }
        })?;
        let table = table
            .as_table()
            .ok_or_else(|| anyhow!("[env.{env}] must be a table"))?;
        let problems = secrets_in(table, "");
        if !problems.is_empty() {
            bail!("{}", problems.join("\n"));
        }
        if let Some(n) = table.get("network").and_then(|v| v.as_str()) {
            if matches!(n, "mainnet" | "pubnet" | "public") {
                bail!("network {n:?}: Caravel runs on testnet only (spec §0)");
            }
        }
        let spec: EnvSpec = toml::Value::Table(table.clone()).try_into()?;
        let problems = spec.check();
        if !problems.is_empty() {
            bail!("{}", problems.join("\n"));
        }
        Ok(Self {
            lane,
            env_name: env.to_string(),
            env: spec,
        })
    }

    /// The settlement's `min_deposit`: the table's, or `[limits] min_deposit`.
    pub fn min_deposit(&self) -> Result<i128> {
        match self.env.settlement_params.min_deposit {
            Some(v) => Ok(i128::from(v)),
            None => Ok(i128::from(self.lane.limits()?.min_deposit)),
        }
    }

    pub fn rpc_url(&self) -> &str {
        self.env
            .rpc_url
            .as_deref()
            .unwrap_or(self.env.network.default_rpc())
    }
}

/// A Stellar CLI identity name.
fn identity_ok(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && s.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Why a key field is not an identity name.
fn key_problem(field: &str, v: &str) -> Option<String> {
    if identity_ok(v) && !looks_like_seed(v) {
        return None;
    }
    Some(format!(
        "{field} = {:?}: name a Stellar CLI identity (`stellar keys generate <name>` or `stellar keys add <name>`), never a key",
        shorten(v)
    ))
}

fn looks_like_seed(s: &str) -> bool {
    (s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()))
        || stellar_strkey::ed25519::PublicKey::from_string(s).is_ok()
}

fn shorten(s: &str) -> String {
    if s.len() > 12 {
        format!("{}…", &s[..6])
    } else {
        s.to_string()
    }
}

/// A Stellar secret key, or something shaped like a seed phrase.
fn is_secret(s: &str) -> bool {
    stellar_strkey::ed25519::PrivateKey::from_string(s).is_ok()
        || s.split_whitespace().count() >= 12
}

/// The path of every string in the table that holds a secret.
fn secrets_in(table: &toml::Table, path: &str) -> Vec<String> {
    fn walk(v: &toml::Value, here: String, out: &mut Vec<String>) {
        match v {
            toml::Value::String(s) if is_secret(s) => out.push(format!(
                "{here} holds a secret: keep keys in the Stellar keystore and name the identity here"
            )),
            toml::Value::Table(t) => {
                for (k, v) in t {
                    let next = if here.is_empty() { k.clone() } else { format!("{here}.{k}") };
                    walk(v, next, out);
                }
            }
            toml::Value::Array(a) => {
                for (i, v) in a.iter().enumerate() {
                    walk(v, format!("{here}[{i}]"), out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(
        &toml::Value::Table(table.clone()),
        path.to_string(),
        &mut out,
    );
    out
}

impl EnvSpec {
    /// Every rule the table breaks, so one run lists them all.
    pub fn check(&self) -> Vec<String> {
        let mut p = Vec::new();
        p.extend(key_problem("admin", &self.admin));
        p.extend(key_problem("relayer.account", &self.relayer.account));
        for (var, id) in &self.relayer.feed_keys {
            if !var
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                p.push(format!(
                    "relayer.feed_keys: {var:?} is not an environment variable name"
                ));
            }
            p.extend(key_problem(&format!("relayer.feed_keys.{var}"), id));
        }
        match (&self.token, self.network) {
            (Token::Named(n), _) if n != CIRCLE_USDC => p.push(format!(
                "token = {n:?}: a known token is \"{CIRCLE_USDC}\"; otherwise use {{ asset = \"CODE:ISSUER\" }} or {{ contract = \"C…\" }}"
            )),
            (Token::Named(_), Network::Local) => p.push(format!(
                "token = \"{CIRCLE_USDC}\" is on testnet; a local network uses {{ local = \"USDC\" }}"
            )),
            (Token::Asset { asset }, _) if parse_asset(asset).is_none() => p.push(format!(
                "token.asset = {asset:?}: CODE:ISSUER, a 1–12 character code and a G… issuer"
            )),
            (Token::Contract { contract }, _) if stellar_strkey::Contract::from_string(contract).is_err() => {
                p.push(format!("token.contract = {contract:?} is not a C… contract address"))
            }
            (Token::Local { local }, _) if !asset_code_ok(local) => {
                p.push(format!("token.local = {local:?}: an asset code of 1–12 letters or digits"))
            }
            (Token::Local { .. }, Network::Testnet) => p.push(
                "token.local is for local networks; on testnet name a real token (\"circle-usdc\", an asset or a contract)".into(),
            ),
            _ => {}
        }
        if let Some(c) = &self.settlement {
            if stellar_strkey::Contract::from_string(c).is_err() {
                p.push(format!("settlement = {c:?} is not a C... contract address"));
            }
        }
        if let Some(h) = &self.settlement_wasm {
            if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
                p.push(format!("settlement_wasm = {h:?} is not a sha256 in hex"));
            }
        }
        if self.validators.is_empty() {
            p.push("[[validators]]: a lane needs at least one validator".into());
        }
        let mut names = std::collections::BTreeSet::new();
        let mut keys = std::collections::BTreeSet::new();
        for v in &self.validators {
            if !identity_ok(&v.name) {
                p.push(format!(
                    "validator name {:?}: letters, digits, '-', '_', '.'",
                    v.name
                ));
            }
            if !names.insert(&v.name) {
                p.push(format!("validator name {:?} appears twice", v.name));
            }
            if !keys.insert(&v.key) {
                p.push(format!("validator key {:?} appears twice", v.key));
            }
            if v.weight == 0 {
                p.push(format!("validator {:?}: weight must be at least 1", v.name));
            }
            p.extend(key_problem(&format!("validator {:?} key", v.name), &v.key));
        }
        let total: u64 = self.validators.iter().map(|v| u64::from(v.weight)).sum();
        if self.threshold == 0 || u64::from(self.threshold) > total {
            p.push(format!(
                "threshold {} must be between 1 and the validators' total weight {total}",
                self.threshold
            ));
        }
        let sp = &self.settlement_params;
        for (name, v) in [
            (
                "force_inclusion_window_secs",
                sp.force_inclusion_window_secs,
            ),
            ("escape_timeout_secs", sp.escape_timeout_secs),
        ] {
            if v == 0 {
                p.push(format!("settlement_params.{name} must be positive"));
            }
        }
        if sp.min_deposit.is_some_and(|m| m < 1) {
            p.push("settlement_params.min_deposit must be at least 1 stroop".into());
        }
        if self.sequencer.port < 1024 {
            p.push(format!(
                "sequencer.port {} must be above 1023",
                self.sequencer.port
            ));
        }
        let mut ports = std::collections::BTreeSet::from([self.sequencer.port]);
        for v in &self.validators {
            match v.port(self.sequencer.port) {
                None => p.push(format!(
                    "validator {:?}: set its port (a validator named n defaults to the sequencer's port + n)",
                    v.name
                )),
                Some(port) if !ports.insert(port) => {
                    p.push(format!("validator {:?}: port {port} is already taken", v.name))
                }
                Some(_) => {}
            }
        }
        let h = &self.host;
        match (h.provider, h.transport) {
            (Provider::Local, _) if h.address.is_some() => {
                p.push("host.address: the local provider runs on this machine".into())
            }
            (Provider::Ssh, _) if h.address.is_none() => p.push(
                "host.address: the ssh provider needs user@host (or the VM name for gcloud-iap)"
                    .into(),
            ),
            (Provider::Ssh, Transport::GcloudIap) if h.project.is_none() || h.zone.is_none() => {
                p.push("host: transport = \"gcloud-iap\" needs project and zone".into())
            }
            _ => {}
        }
        // The root goes into shell commands on the host (`rm -rf <root>/data/*`
        // among them), so it is a plain absolute path, never `/` itself.
        let plain = h.root.starts_with('/')
            && h.root.len() > 1
            && !h.root.ends_with('/')
            && h.root
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.'))
            && !h.root.split('/').any(|seg| seg == "..");
        if !plain {
            p.push(format!(
                "host.root = {:?}: an absolute path of letters, digits, '/', '_', '-', '.', not / itself",
                h.root
            ));
        }
        p
    }
}
