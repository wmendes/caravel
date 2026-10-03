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
    /// The host it runs on (`[hosts.<name>]`); the sequencer's when not given.
    #[serde(default)]
    pub host: Option<String>,
    /// Run by someone else at this URL (C-23): in the signer set and asked
    /// to sign, never deployed. `key` then names an identity added from its
    /// public key (`stellar keys add <name> --public-key G…`).
    #[serde(default)]
    pub url: Option<String>,
}

impl ValidatorSpec {
    /// Run elsewhere, by someone else (C-23).
    pub fn external(&self) -> bool {
        self.url.is_some()
    }

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
    /// The host it (and the relayer) runs on, with several hosts.
    #[serde(default)]
    pub host: Option<String>,
    /// The identity that signs each `/v1/sign` request (C-23, DEC-095):
    /// needed once a validator runs on another host or elsewhere.
    #[serde(default)]
    pub key: Option<String>,
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
            host: None,
            key: None,
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
    /// The address other hosts of the deployment reach this one's nodes at
    /// (a VPC or VPN address). Nodes another host calls bind it (C-22).
    #[serde(default)]
    pub private_address: Option<String>,
}

fn default_root() -> String {
    "/opt/caravel".into()
}

/// A declared account, `[env.<name>.accounts.<n>]` (M0.6, C-18): a
/// Stellar CLI identity the deployment funds, trusts assets from and tops
/// up.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AccountSpec {
    /// The Stellar CLI identity; the account's name when not given.
    #[serde(default)]
    pub identity: Option<String>,
    /// Friendbot funds it when the network doesn't have it (default true).
    #[serde(default)]
    pub fund: Option<bool>,
    /// Assets it trusts: `"settlement"` (the settlement token's asset) or
    /// `"CODE:ISSUER"`.
    #[serde(default)]
    pub trustlines: Vec<String>,
    /// Balances to top up to, at least: `{ settlement = "100" }` in token
    /// units. Minted by the admin, so the admin must issue the token.
    #[serde(default)]
    pub balances: BTreeMap<String, String>,
    /// Addresses (`account.bob`, `contract.settlement`) applied before it.
    #[serde(default)]
    pub depends_on: Vec<String>,
}

impl AccountSpec {
    /// The identity it is, by the account's name.
    pub fn identity_of<'a>(&'a self, name: &'a str) -> &'a str {
        self.identity.as_deref().unwrap_or(name)
    }
}

/// A declared token, `[env.<name>.tokens.<n>]` (M0.6, C-19): a Stellar
/// asset and its Stellar Asset Contract, which `apply` deploys when the
/// network has none.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TokenSpec {
    /// The asset code, 1–12 letters or digits.
    pub code: String,
    /// Who issues it: `"admin"`, a declared account's name, or a `G…`
    /// address (then nothing in the file can mint it).
    pub issuer: String,
}

/// `lifecycle` on a deployment or a contract (C-20).
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Lifecycle {
    /// `caravel destroy` refuses while it is set.
    #[serde(default)]
    pub prevent_destroy: bool,
}

/// A declared contract, `[env.<name>.contracts.<n>]` (M0.6, C-20): any
/// Wasm, deployed once at an address derived from its deployer and salt.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ContractSpec {
    /// A `.wasm` file (relative to the lane file), or the sha256 of Wasm
    /// already uploaded to the network.
    pub wasm: String,
    /// Who deploys it: `"admin"` (the default) or a declared account.
    #[serde(default)]
    pub deployer: Option<String>,
    /// Part of its address: change it for a new contract.
    #[serde(default)]
    pub salt: Option<String>,
    /// Its constructor's arguments, by name. Used once, when it is deployed:
    /// they can't be read back.
    #[serde(default)]
    pub args: BTreeMap<String, toml::Value>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub lifecycle: Lifecycle,
}

impl ContractSpec {
    pub fn deployer(&self) -> &str {
        self.deployer.as_deref().unwrap_or("admin")
    }

    /// The Wasm's sha256 when `wasm` is one (not a file).
    pub fn wasm_hash(&self) -> Option<[u8; 32]> {
        let w = &self.wasm;
        (w.len() == 64 && w.chars().all(|c| c.is_ascii_hexdigit())).then(|| {
            let v: Vec<u8> = (0..32)
                .map(|i| u8::from_str_radix(&w[2 * i..2 * i + 2], 16).expect("hex"))
                .collect();
            v.try_into().expect("32 bytes")
        })
    }

    /// Its arguments as `stellar contract deploy -- --name value` takes them:
    /// strings as they are, numbers and booleans as written, lists and maps
    /// as JSON.
    pub fn arg_strings(&self) -> Vec<(String, String)> {
        self.args
            .iter()
            .map(|(k, v)| {
                let v = match v {
                    toml::Value::String(s) => s.clone(),
                    toml::Value::Array(_) | toml::Value::Table(_) => {
                        serde_json::to_string(v).unwrap_or_default()
                    }
                    other => other.to_string(),
                };
                (k.clone(), v)
            })
            .collect()
    }
}

/// One `[env.<name>]` table.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EnvSpec {
    /// The deployment `caravel` picks when none is named (at most one).
    #[serde(default)]
    pub default: bool,
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
    /// This deployment's `[node]` settings, over the lane file's (they
    /// aren't consensus, so deployments may differ).
    #[serde(default)]
    pub node: Option<toml::Table>,
    /// Declared accounts, by name (`account.<name>`).
    #[serde(default)]
    pub accounts: BTreeMap<String, AccountSpec>,
    /// Declared tokens, by name (`token.<name>`).
    #[serde(default)]
    pub tokens: BTreeMap<String, TokenSpec>,
    /// Declared contracts, by name (`contract.<name>`).
    #[serde(default)]
    pub contracts: BTreeMap<String, ContractSpec>,
    /// `prevent_destroy`: `caravel destroy` refuses this deployment.
    #[serde(default)]
    pub lifecycle: Lifecycle,
    /// Several hosts, by name (C-22). `host` is the sequencer's (and
    /// shorthand for one host named `default`).
    #[serde(default)]
    pub hosts: BTreeMap<String, HostSpec>,
}

/// A lane file and one of its deployments.
#[derive(Debug)]
pub struct Manifest {
    pub lane: LaneFile,
    pub env_name: String,
    pub env: EnvSpec,
    /// The lane file's vars and their values (`name = value, …`, sensitive
    /// ones masked); empty when it declares none.
    pub vars: String,
    /// The lane file language's document and inputs, for what is computed
    /// once keys and addresses are known (relayer feeds, outputs).
    pub doc: Option<caravel_lanefile::LaneDoc>,
    pub inputs: Inputs,
    /// Values left for then: paths in the deployment (under `relayer.feeds`).
    pub deferred: Vec<String>,
}

/// One `[env.<name>]` table, as listed before any is chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvInfo {
    pub name: String,
    /// `default = true`.
    pub default: bool,
    /// `network`, as written.
    pub network: Option<String>,
    /// `host.provider`, as written.
    pub provider: Option<String>,
}

/// The lane file's deployments, in name order.
pub fn envs(lane: &LaneFile) -> Vec<EnvInfo> {
    lane.env
        .iter()
        .map(|(name, t)| {
            let s = |v: Option<&toml::Value>| v.and_then(|v| v.as_str()).map(String::from);
            EnvInfo {
                name: name.clone(),
                default: t.get("default").and_then(|v| v.as_bool()) == Some(true),
                network: s(t.get("network")),
                provider: s(t.get("host").and_then(|h| h.get("provider"))),
            }
        })
        .collect()
}

/// A lane file from disk, with its includes and inheritance resolved
/// (`caravel_lanefile`): every deployment merged, its expressions not yet
/// evaluated (for listing deployments; a [`Manifest`] evaluates its own).
pub fn load_lane(path: &Path) -> Result<LaneFile> {
    let doc = caravel_lanefile::LaneDoc::load(path).map_err(anyhow::Error::new)?;
    LaneFile::from_table(doc.to_table()).with_context(|| format!("parsing {}", path.display()))
}

pub use caravel_lanefile::Inputs;

impl Manifest {
    pub fn load(path: &Path, env: &str) -> Result<Self> {
        Self::load_with(path, env, &Inputs::default())
    }

    /// One deployment of a lane file, its expressions evaluated with `inputs`
    /// (`--var`, `--var-file`, `CARAVEL_VAR_*`).
    pub fn load_with(path: &Path, env: &str, inputs: &Inputs) -> Result<Self> {
        let doc = caravel_lanefile::LaneDoc::load(path).map_err(anyhow::Error::new)?;
        Self::from_doc(&doc, env, inputs).with_context(|| format!("{} [env.{env}]", path.display()))
    }

    pub fn parse(text: &str, env: &str) -> Result<Self> {
        let doc = caravel_lanefile::LaneDoc::parse(text).map_err(anyhow::Error::new)?;
        Self::from_doc(&doc, env, &Inputs::default()).with_context(|| format!("[env.{env}]"))
    }

    fn from_doc(doc: &caravel_lanefile::LaneDoc, env: &str, inputs: &Inputs) -> Result<Self> {
        // Secrets are refused where they're written, as well as where they land.
        let mut written = toml::Table::new();
        for (k, v) in &doc.vars {
            written.insert(format!("vars.{k}"), v.clone());
        }
        for (k, v) in &doc.locals {
            written.insert(format!("locals.{k}"), v.clone());
        }
        let problems = secrets_in(&written, "");
        if !problems.is_empty() {
            bail!("{}", problems.join("\n"));
        }
        let section = |k: &str, f: &str| {
            doc.genesis
                .get(k)
                .and_then(|t| t.get(f))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let name = section("lane", "name");
        let lane_id =
            caravel_runtime::checkpoint::sha256(&caravel_core::preimage::lane_id_preimage(&name));
        let before = crate::attrs::before(
            &name,
            &section("app", "template"),
            &lane_id,
            &section("app", "engine_wasm_sha256"),
        );
        let r = doc
            .resolve_env_with(env, inputs, &before)
            .map_err(anyhow::Error::new)?;
        // Values known only after keys and addresses go where they can wait.
        let early: Vec<String> = r
            .deferred
            .iter()
            .filter(|p| !crate::attrs::deferred_ok(p))
            .map(|p| {
                let at = doc
                    .locate(&format!("env.{env}.{p}"))
                    .map(|l| format!("\n    at {l}"))
                    .unwrap_or_default();
                format!(
                    "{p} uses a value known only once the lane's keys and addresses are (account, contract, token, node, …): only relayer feeds, contracts' args and [outputs] can{at}"
                )
            })
            .collect();
        if !early.is_empty() {
            bail!("{}", early.join("\n"));
        }
        let values: toml::Table = r
            .vars
            .iter()
            .map(|(k, v)| (format!("--var {k}"), v.clone()))
            .collect();
        let problems = secrets_in(&values, "");
        if !problems.is_empty() {
            bail!("{}", problems.join("\n"));
        }
        let mut t = doc.genesis.clone();
        t.insert(
            "env".into(),
            toml::Value::Table(toml::Table::from_iter([(
                env.to_string(),
                toml::Value::Table(r.table.clone()),
            )])),
        );
        let locate = |msg: &str| locate_problem(doc, env, &r.table, msg);
        let mut m = Self::from_lane_with(LaneFile::from_table(t)?, env, &locate)?;
        m.vars = r.vars_line();
        m.doc = Some(doc.clone());
        m.inputs = inputs.clone();
        m.deferred = r.deferred;
        Ok(m)
    }

    pub fn from_lane(lane: LaneFile, env: &str) -> Result<Self> {
        Self::from_lane_with(lane, env, &|_| None)
    }

    /// Like [`Manifest::from_lane`]; `locate` says where in the lane file a
    /// problem's value is (`file:line:col`).
    fn from_lane_with(
        lane: LaneFile,
        env: &str,
        locate: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self> {
        let at = |msg: String| match locate(&msg) {
            Some(loc) => format!("{msg}\n    at {loc}"),
            None => msg,
        };
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
        // With `hosts`, `host` is the sequencer's (C-22): filled in from it.
        let mut table = table.clone();
        if let Some(hosts) = table.get("hosts").and_then(|h| h.as_table()).cloned() {
            if table.contains_key("host") {
                bail!(
                    "{}",
                    at("give `host` (one host) or `hosts` (several), not both".into())
                );
            }
            let named = table
                .get("sequencer")
                .and_then(|s| s.get("host"))
                .and_then(|h| h.as_str())
                .map(String::from);
            let primary = match (named, hosts.len()) {
                (Some(n), _) => n,
                (None, 1) => hosts.keys().next().cloned().unwrap_or_default(),
                (None, _) => bail!(
                    "{}",
                    at("with several hosts, say which runs the sequencer and the relayer: [sequencer] host = \"<name>\"".into())
                ),
            };
            let Some(spec) = hosts.get(&primary) else {
                bail!(
                    "{}",
                    at(format!(
                        "sequencer.host = {primary:?}: no [hosts.{primary}] (there are {})",
                        hosts.keys().cloned().collect::<Vec<_>>().join(", ")
                    ))
                );
            };
            table.insert("host".into(), spec.clone());
        }
        let spec: EnvSpec = toml::Value::Table(table.clone())
            .try_into()
            .map_err(|e: toml::de::Error| anyhow!("{}", at(e.to_string().trim().to_string())))?;
        let problems = spec.check();
        if !problems.is_empty() {
            bail!(
                "{}",
                problems.into_iter().map(at).collect::<Vec<_>>().join("\n")
            );
        }
        let mut spec = spec;
        spec.desugar_token();
        // The deployment's [node] over the lane file's: not consensus.
        let lane = match &spec.node {
            None => lane,
            Some(over) => {
                let mut raw = lane.raw.clone();
                let mut node = raw
                    .get("node")
                    .and_then(|n| n.as_table())
                    .cloned()
                    .unwrap_or_default();
                for (k, v) in over {
                    node.insert(k.clone(), v.clone());
                }
                raw.insert("node".into(), toml::Value::Table(node));
                let mut merged = LaneFile::from_table(raw).context("[env] node")?;
                merged.env = lane.env;
                merged
                    .check_node_settings()
                    .with_context(|| format!("[env.{env}.node]"))?;
                merged
            }
        };
        Ok(Self {
            lane,
            env_name: env.to_string(),
            env: spec,
            vars: String::new(),
            doc: None,
            inputs: Inputs::default(),
            deferred: Vec::new(),
        })
    }

    /// Fills in what was left for once keys and addresses are known (the
    /// relayer's feeds), from `attrs` (`crate::attrs::attributes`).
    pub fn finish(
        &mut self,
        attrs: &BTreeMap<String, caravel_lanefile::expr::Value>,
    ) -> Result<()> {
        let Some(doc) = &self.doc else {
            return Ok(());
        };
        if self.deferred.is_empty() {
            return Ok(());
        }
        let r = doc
            .resolve_env_with(&self.env_name, &self.inputs, attrs)
            .map_err(anyhow::Error::new)?;
        let feeds: Vec<toml::Table> = r
            .table
            .get("relayer")
            .and_then(|t| t.get("feeds"))
            .cloned()
            .map(|f| f.try_into())
            .transpose()
            .context("relayer.feeds")?
            .unwrap_or_default();
        self.env.relayer.feeds = feeds;
        // Contracts' arguments may name addresses (C-20).
        if let Some(contracts) = r.table.get("contracts").and_then(|c| c.as_table()) {
            for (name, c) in contracts {
                if let (Some(spec), Some(args)) = (
                    self.env.contracts.get_mut(name),
                    c.get("args").and_then(|a| a.as_table()),
                ) {
                    spec.args = args.clone().into_iter().collect();
                }
            }
        }
        self.deferred.clear();
        Ok(())
    }

    /// The deployment's `[outputs]`, from `attrs`.
    pub fn outputs(
        &self,
        attrs: &BTreeMap<String, caravel_lanefile::expr::Value>,
    ) -> Result<Vec<caravel_lanefile::Output>> {
        match &self.doc {
            None => Ok(Vec::new()),
            Some(doc) => doc
                .outputs(&self.env_name, &self.inputs, attrs)
                .map_err(anyhow::Error::new),
        }
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

/// Where a problem's value is written: the field a message names (its
/// leading `a.b`, or serde's "in `a.b`"), looked up in the lane file's
/// origins, a shorter path when the full one has none.
fn locate_problem(
    doc: &caravel_lanefile::LaneDoc,
    env: &str,
    table: &toml::Table,
    msg: &str,
) -> Option<String> {
    let path = if let Some(i) = msg.find("unknown field `") {
        msg[i + 15..]
            .split('`')
            .next()
            .unwrap_or_default()
            .to_string()
    } else if let Some(i) = msg.find("in `") {
        msg[i + 4..]
            .split('`')
            .next()
            .unwrap_or_default()
            .to_string()
    } else if let Some(rest) = msg.strip_prefix("validator \"") {
        // `validator "<name>" …`: the validator with that name.
        let name = rest.split('"').next().unwrap_or_default();
        let i = table
            .get("validators")
            .and_then(|v| v.as_array())
            .and_then(|a| {
                a.iter()
                    .position(|v| v.get("name").and_then(|n| n.as_str()) == Some(name))
            })?;
        format!("validators[{i}]")
    } else {
        msg.split([' ', '=', ':', ','])
            .next()
            .unwrap_or_default()
            .trim_matches(['[', ']'])
            .to_string()
    };
    let mut p = path.as_str();
    loop {
        if !p.is_empty() {
            if let Some(loc) = doc.locate(&format!("env.{env}.{p}")) {
                return Some(loc);
            }
        }
        match p.rfind(['.', '[']) {
            Some(i) => p = &p[..i],
            None => {
                return doc
                    .locate(&format!("env.{env}.{p}"))
                    .or_else(|| doc.locate(&format!("env.{env}.validators")))
                    .filter(|_| path.starts_with("validators"))
            }
        }
    }
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
    /// The sequencer's host: it runs the sequencer and the relayer, and its
    /// resources keep their one-host addresses (`release`, `file.<path>`).
    pub fn primary_host(&self) -> String {
        match (&self.sequencer.host, self.hosts.len()) {
            (Some(h), _) => h.clone(),
            (None, 1) => self.hosts.keys().next().cloned().unwrap_or_default(),
            _ => "default".into(),
        }
    }

    /// Every other host, by name.
    pub fn other_hosts(&self) -> Vec<(String, HostSpec)> {
        let primary = self.primary_host();
        self.hosts
            .iter()
            .filter(|(n, _)| **n != primary)
            .map(|(n, h)| (n.clone(), h.clone()))
            .collect()
    }

    /// The host `node` runs on: a validator's own, else the sequencer's.
    pub fn host_of(&self, node: &str) -> String {
        node.strip_prefix("validator-")
            .and_then(|name| self.validators.iter().find(|v| v.name == name))
            .and_then(|v| v.host.clone())
            .unwrap_or_else(|| self.primary_host())
    }

    /// A host's spec by name.
    pub fn host_spec(&self, name: &str) -> &HostSpec {
        if name == self.primary_host() {
            &self.host
        } else {
            &self.hosts[name]
        }
    }

    /// The validators this deployment runs (not the ones run elsewhere).
    pub fn run_validators(&self) -> impl Iterator<Item = (usize, &ValidatorSpec)> {
        self.validators
            .iter()
            .enumerate()
            .filter(|(_, v)| !v.external())
    }

    /// `node`'s port: the sequencer's, or a validator's.
    fn port_of(&self, node: &str) -> Option<u16> {
        if node == "sequencer" {
            return Some(self.sequencer.port);
        }
        let name = node.strip_prefix("validator-")?;
        self.validators
            .iter()
            .find(|v| v.name == name)?
            .port(self.sequencer.port)
    }

    /// Where `node` is reached from the nodes on host `from` (C-22, C-23):
    /// on its own host where it listens, between two `local` hosts at
    /// loopback, from another host at its host's private address, else
    /// through its host's public URL; a validator run elsewhere at its
    /// `url`. `None`: it can't be reached from there.
    pub fn url_of(&self, from: &str, node: &str) -> Option<String> {
        if let Some(url) = node
            .strip_prefix("validator-")
            .and_then(|n| self.validators.iter().find(|v| v.name == n))
            .and_then(|v| v.url.as_deref())
        {
            return Some(url.trim_end_matches('/').to_string());
        }
        let port = self.port_of(node)?;
        let to = self.host_of(node);
        let (f, t) = (self.host_spec(from), self.host_spec(&to));
        if from == to {
            return Some(format!("http://{}:{port}", self.listen_on(&to, node)));
        }
        if f.provider == Provider::Local && t.provider == Provider::Local {
            return Some(format!("http://127.0.0.1:{port}"));
        }
        if let Some(a) = &t.private_address {
            return Some(format!("http://{a}:{port}"));
        }
        let public = t.public_url.as_deref()?.trim_end_matches('/');
        Some(match node.strip_prefix("validator-") {
            Some(name) => format!("{public}/validators/{name}"),
            None => public.to_string(),
        })
    }

    /// The sequencer reaches validator `node` through its host's public URL,
    /// so that host's proxy passes its `/v1/sign` (signed, C-23).
    pub fn sign_via_public(&self, node: &str) -> bool {
        let primary = self.primary_host();
        let to = self.host_of(node);
        to != primary
            && self
                .url_of(&primary, node)
                .zip(self.host_spec(&to).public_url.as_deref())
                .is_some_and(|(u, p)| u.starts_with(p.trim_end_matches('/')))
    }

    /// The address a node on `host` listens on: its private address when
    /// another host reaches it there, else loopback.
    pub fn listen_on(&self, host: &str, node: &str) -> String {
        let reached_from_elsewhere = if node == "sequencer" {
            self.run_validators()
                .any(|(_, v)| self.host_of(&format!("validator-{}", v.name)) != host)
        } else {
            self.primary_host() != host
        };
        let spec = self.host_spec(host);
        match (&spec.private_address, spec.provider) {
            (Some(a), Provider::Ssh) if reached_from_elsewhere => a.clone(),
            _ => "127.0.0.1".into(),
        }
    }

    /// `token = "<declared>"` as the form it stands for: issued by the admin,
    /// `{ local = CODE }` (`CODE:<admin>`, on any network); by a `G…`
    /// address, `{ asset = "CODE:G…" }`. Runs after [`EnvSpec::check`].
    pub fn desugar_token(&mut self) {
        if let Token::Named(n) = &self.token {
            if let Some(t) = self.tokens.get(n) {
                self.token = if t.issuer == "admin" {
                    Token::Local {
                        local: t.code.clone(),
                    }
                } else {
                    Token::Asset {
                        asset: format!("{}:{}", t.code, t.issuer),
                    }
                };
            }
        }
    }

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
            (Token::Named(n), _) if self.tokens.contains_key(n) => {
                let issuer = &self.tokens[n].issuer;
                if issuer != "admin" && !issuer.starts_with('G') {
                    p.push(format!(
                        "token = {n:?}: a settlement token declared here is issued by the admin or a G… address, not {issuer:?}"
                    ));
                }
            }
            (Token::Named(n), _) if n != CIRCLE_USDC => p.push(format!(
                "token = {n:?}: a known token is \"{CIRCLE_USDC}\", or a token this deployment declares ([tokens.{n}]); otherwise use {{ asset = \"CODE:ISSUER\" }} or {{ contract = \"C…\" }}"
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
        for (name, c) in &self.contracts {
            let at = format!("contracts.{name}");
            if !identity_ok(name) || name == "settlement" {
                p.push(format!(
                    "{at}: a contract's name is letters, digits, '-', '_', '.', and not `settlement`"
                ));
            }
            if c.wasm_hash().is_none() && !c.wasm.ends_with(".wasm") {
                p.push(format!(
                    "{at}.wasm = {:?}: a .wasm file (relative to the lane file) or the sha256 of uploaded Wasm",
                    c.wasm
                ));
            }
            let d = c.deployer();
            if d != "admin" && !self.accounts.contains_key(d) {
                p.push(format!(
                    "{at}.deployer = {d:?}: \"admin\" or a declared account's name"
                ));
            }
            for k in c.args.keys() {
                if !k.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
                    p.push(format!(
                        "{at}.args.{k}: a constructor argument's name is letters, digits and '_'"
                    ));
                }
            }
        }
        for (name, t) in &self.tokens {
            let at = format!("tokens.{name}");
            if !identity_ok(name) || name == "settlement" {
                p.push(format!(
                    "{at}: a token's name is letters, digits, '-', '_', '.', and not `settlement`"
                ));
            }
            if !asset_code_ok(&t.code) {
                p.push(format!("{at}.code = {:?}: 1–12 letters or digits", t.code));
            }
            let issuer_ok = t.issuer == "admin"
                || self.accounts.contains_key(&t.issuer)
                || stellar_strkey::ed25519::PublicKey::from_string(&t.issuer).is_ok();
            if !issuer_ok {
                p.push(format!(
                    "{at}.issuer = {:?}: \"admin\", a declared account's name, or a G… address",
                    t.issuer
                ));
            }
        }
        for (name, a) in &self.accounts {
            let at = format!("accounts.{name}");
            if !identity_ok(name) {
                p.push(format!(
                    "{at}: an account's name is letters, digits, '-', '_', '.'"
                ));
            }
            if matches!(name.as_str(), "admin" | "relayer") {
                p.push(format!(
                    "{at}: `{name}` is the deployment's own account; name it something else"
                ));
            }
            p.extend(key_problem(&format!("{at}.identity"), a.identity_of(name)));
            for t in &a.trustlines {
                if t == "settlement" && matches!(self.token, Token::Contract { .. }) {
                    p.push(format!(
                        "{at}.trustlines: the settlement token is a contract, not a Stellar asset, so there is no trustline to it"
                    ));
                }
                if t != "settlement" && !self.tokens.contains_key(t) && parse_asset(t).is_none() {
                    p.push(format!(
                        "{at}.trustlines: {t:?} is \"settlement\", a declared token's name, or CODE:ISSUER (a G… issuer)"
                    ));
                }
            }
            for (token, amount) in &a.balances {
                if token != "settlement" && !self.tokens.contains_key(token) {
                    p.push(format!(
                        "{at}.balances.{token}: \"settlement\" or a declared token's name"
                    ));
                }
                if caravel_node::plugin::parse_amount(amount, Some(7)).is_err() {
                    p.push(format!(
                        "{at}.balances.{token} = {amount:?}: an amount in token units, e.g. \"100\" or \"12.5\""
                    ));
                }
                if token == "settlement" && matches!(self.token, Token::Contract { .. }) {
                    p.push(format!(
                        "{at}.balances: the settlement token is a contract this tool can't read balances of; top it up by hand"
                    ));
                }
                if !a.trustlines.iter().any(|t| t == token) {
                    p.push(format!(
                        "{at}.balances.{token}: add \"{token}\" to its trustlines, which a balance needs"
                    ));
                }
            }
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
        for (_, v) in self.run_validators() {
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
        // Several hosts (C-22): each node on a declared one, each host valid,
        // and a node another host calls reachable from it.
        let known: Vec<String> = if self.hosts.is_empty() {
            vec![self.primary_host()]
        } else {
            self.hosts.keys().cloned().collect()
        };
        for v in &self.validators {
            if let Some(h) = &v.host {
                if !known.contains(h) {
                    p.push(format!(
                        "validator {:?}: host = {h:?}, but the deployment has {}",
                        v.name,
                        if self.hosts.is_empty() {
                            "one host (declare [hosts.<name>] for several)".to_string()
                        } else {
                            known
                                .iter()
                                .map(|k| format!("[hosts.{k}]"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    ));
                    continue;
                }
            }
        }
        // Across hosts, both ways (C-22, C-23): the validator follows the
        // sequencer, and the sequencer asks it to sign.
        let primary = self.primary_host();
        for (_, v) in self.run_validators() {
            let node = format!("validator-{}", v.name);
            let h = self.host_of(&node);
            if h == primary || !known.contains(&h) {
                continue;
            }
            for (from, to, what, target) in [
                (h.as_str(), primary.as_str(), "the sequencer", "sequencer"),
                (primary.as_str(), h.as_str(), "its /v1/sign", node.as_str()),
            ] {
                if self.url_of(from, target).is_none() {
                    p.push(format!(
                        "validator {:?} on host {h:?} needs {what} on host {to:?} reached from {from:?}: give [hosts.{to}] a private_address or a public_url",
                        v.name
                    ));
                }
            }
        }
        // A validator run elsewhere: where, and nothing to deploy.
        for v in &self.validators {
            let Some(url) = &v.url else { continue };
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                p.push(format!(
                    "validator {:?}: url = {url:?}: an http(s) URL",
                    v.name
                ));
            }
            if v.host.is_some() || v.port.is_some() {
                p.push(format!(
                    "validator {:?}: run elsewhere (url), so no host or port",
                    v.name
                ));
            }
        }
        // Signed requests (OQ-009): once a validator is on another host or
        // run elsewhere, it answers only requests the sequencer signed.
        let afar = self
            .validators
            .iter()
            .any(|v| v.external() || self.host_of(&format!("validator-{}", v.name)) != primary);
        match &self.sequencer.key {
            Some(k) => p.extend(key_problem("sequencer.key", k)),
            None if afar => p.push(
                "validators on other hosts or run elsewhere answer only requests the sequencer signed: give [sequencer] key = \"<identity>\"".into(),
            ),
            None => {}
        }
        for name in self.hosts.keys() {
            // Its addresses are `host.<name>…`.
            if !identity_ok(name)
                || name.contains('.')
                || matches!(name.as_str(), "data" | "release" | "file")
            {
                p.push(format!(
                    "hosts.{name}: a host's name is letters, digits, '-', '_', and not data, release or file"
                ));
            }
        }
        for (name, h) in self.other_hosts() {
            p.extend(host_problems(&format!("hosts.{name}"), &h));
        }
        let h = &self.host;
        p.extend(host_problems("host", h));
        p
    }
}

/// What a host table gets wrong.
fn host_problems(at: &str, h: &HostSpec) -> Vec<String> {
    let mut p = Vec::new();
    {
        match (h.provider, h.transport) {
            (Provider::Local, _) if h.address.is_some() => p.push(format!(
                "{at}.address: the local provider runs on this machine"
            )),
            (Provider::Ssh, _) if h.address.is_none() => p.push(format!(
                "{at}.address: the ssh provider needs user@host (or the VM name for gcloud-iap)"
            )),
            (Provider::Ssh, Transport::GcloudIap) if h.project.is_none() || h.zone.is_none() => p
                .push(format!(
                    "{at}: transport = \"gcloud-iap\" needs project and zone"
                )),
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
                "{at}.root = {:?}: an absolute path of letters, digits, '/', '_', '-', '.', not / itself",
                h.root
            ));
        }
        if let Some(a) = &h.private_address {
            if a.parse::<std::net::IpAddr>().is_err() {
                p.push(format!("{at}.private_address = {a:?}: an IP address"));
            }
        }
    }
    p
}
