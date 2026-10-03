//! `plan` and `apply` over one deployment: resolve the lane file (genesis,
//! identities, addresses, the release), read Stellar and the host, render the
//! node configs with the target epoch, diff, and carry the steps out.

use std::collections::BTreeMap;
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use caravel_node::stellar_rpc::Rpc;
use caravel_runtime::checkpoint::sha256;

use crate::address::{asset_contract_id, contract_id, settlement_salt, strkey};
use crate::chain::{self, Extra};
use crate::host::HostProvider;
use crate::local::Local;
use crate::manifest::{Manifest, Network, Provider, Token};
use crate::plan::{
    self, Chain, Desired, DesiredHost, Host, Key, NodeReport, Params, Plan, SignerSet, Step,
};
use crate::release::{self, Release};
use crate::render::{self, engine_file, validator_node, Resolved};
use crate::stellar::{self, Cli};
use crate::template::{GenesisHashes, Template};

/// A deployment resolved and read, ready to diff.
pub struct Prepared {
    pub m: Manifest,
    /// What expressions read once keys and addresses are known
    /// (`crate::attrs`): relayer feeds and outputs.
    pub attrs: BTreeMap<String, caravel_lanefile::expr::Value>,
    pub desired: Desired,
    /// `validator-<name>` → its public key.
    pub validator_keys: BTreeMap<String, Key>,
    pub files: BTreeMap<String, String>,
    pub release: Release,
    pub cli: Cli,
    pub host_provider: HostProvider,
    pub chain: Chain,
    pub extra: Extra,
    pub host: Host,
    pub notes: Vec<String>,
    /// The deployment's other hosts (C-22): their providers and files.
    pub others: Vec<OtherRun>,
}

/// Another host of a deployment, ready to change.
pub struct OtherRun {
    pub name: String,
    pub provider: HostProvider,
    pub files: BTreeMap<String, String>,
}

fn parse_hex(h: &str, what: &str) -> Result<Key> {
    let v: Vec<u8> = (0..32)
        .map(|i| {
            h.get(2 * i..2 * i + 2)
                .and_then(|b| u8::from_str_radix(b, 16).ok())
        })
        .collect::<Option<_>>()
        .ok_or_else(|| anyhow!("{what} {h:?} is not a sha256 in hex"))?;
    Ok(v.try_into().expect("32 bytes"))
}

/// The deployment's identities, as keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keys {
    pub admin: Key,
    pub relayer: Key,
    /// In file order.
    pub validators: Vec<Key>,
    /// Declared accounts, by name.
    pub accounts: BTreeMap<String, Key>,
}

impl Keys {
    /// From the Stellar CLI keystore.
    pub fn from_keystore(m: &Manifest) -> Result<Self> {
        Ok(Self {
            admin: Cli::public_key(&m.env.admin)?,
            relayer: Cli::public_key(&m.env.relayer.account)?,
            validators: m
                .env
                .validators
                .iter()
                .map(|v| Cli::public_key(&v.key))
                .collect::<Result<Vec<_>>>()?,
            accounts: m
                .env
                .accounts
                .iter()
                .map(|(name, a)| Ok((name.clone(), Cli::public_key(a.identity_of(name))?)))
                .collect::<Result<_>>()?,
        })
    }
}

/// Where a deployment lives on Stellar, known from the lane file and its keys
/// alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Addresses {
    /// The settlement token's contract.
    pub token: Key,
    /// The Stellar asset behind it, when it is a Stellar Asset Contract that
    /// `apply` may deploy.
    pub token_asset: Option<(String, Key)>,
    pub settlement: Key,
    /// Named in the lane file (deployed before this tool), not derived.
    pub settlement_pinned: bool,
}

/// Only the admin's public key is needed (the settlement address and a local
/// token derive from it), so a lane's users don't need the operators' keys.
pub fn addresses(m: &Manifest, admin: &Key) -> Result<Addresses> {
    let passphrase = m.env.network.passphrase();
    let (token, token_asset) = match &m.env.token {
        Token::Named(_) => (
            stellar_strkey::Contract::from_string(crate::versions::testnet_usdc())
                .map_err(|_| anyhow!("versions.json testnet.usdc_sac"))?
                .0,
            None,
        ),
        Token::Asset { asset } => {
            let (code, issuer) = crate::manifest::parse_asset(asset)
                .ok_or_else(|| anyhow!("token.asset {asset:?}"))?;
            (
                asset_contract_id(passphrase, &code, &issuer),
                Some((code, issuer)),
            )
        }
        Token::Contract { contract } => (
            stellar_strkey::Contract::from_string(contract)
                .map_err(|_| anyhow!("token.contract {contract:?}"))?
                .0,
            None,
        ),
        Token::Local { local } => (
            asset_contract_id(passphrase, local, admin),
            Some((local.clone(), *admin)),
        ),
    };
    let (settlement, settlement_pinned) = match &m.env.settlement {
        Some(c) => (
            stellar_strkey::Contract::from_string(c)
                .map_err(|_| anyhow!("settlement {c:?}"))?
                .0,
            true,
        ),
        None => (
            contract_id(passphrase, admin, &settlement_salt(&m.lane.lane_id())),
            false,
        ),
    };
    Ok(Addresses {
        token,
        token_asset,
        settlement,
        settlement_pinned,
    })
}

/// The lane file's deployment as `plan` compares it, before its files are
/// rendered (`host.files` is empty).
pub fn desired(
    m: &Manifest,
    genesis: &GenesisHashes,
    keys: &Keys,
    a: &Addresses,
    engine_wasm_hash: Key,
    settlement_wasm: Key,
    release: &str,
) -> Result<Desired> {
    let mut signers: Vec<(Key, u32)> = keys
        .validators
        .iter()
        .zip(&m.env.validators)
        .map(|(k, v)| (*k, v.weight))
        .collect();
    signers.sort();
    let sp = &m.env.settlement_params;
    // The Stellar asset behind the settlement token, whoever deploys its
    // contract (none for a SEP-41 contract).
    let settlement_asset = match &m.env.token {
        Token::Named(_) => Some((
            "USDC".to_string(),
            stellar_strkey::ed25519::PublicKey::from_string(crate::versions::testnet_usdc_issuer())
                .map_err(|_| anyhow!("versions.json testnet.usdc_issuer"))?
                .0,
        )),
        Token::Contract { .. } => None,
        _ => a.token_asset.clone(),
    };
    // Declared tokens: each issuer as a key, and the identity that mints
    // when the lane file has it.
    let passphrase = m.env.network.passphrase();
    let mut declared_tokens = BTreeMap::new();
    for (name, t) in &m.env.tokens {
        let (issuer, minter) = if t.issuer == "admin" {
            (keys.admin, Some(m.env.admin.clone()))
        } else if let Some(a) = m.env.accounts.get(&t.issuer) {
            (
                *keys
                    .accounts
                    .get(&t.issuer)
                    .ok_or_else(|| anyhow!("no key for account {}", t.issuer))?,
                Some(a.identity_of(&t.issuer).to_string()),
            )
        } else {
            (
                stellar_strkey::ed25519::PublicKey::from_string(&t.issuer)
                    .map_err(|_| anyhow!("tokens.{name}.issuer {:?}", t.issuer))?
                    .0,
                None,
            )
        };
        declared_tokens.insert(
            name.clone(),
            crate::plan::DeclaredToken {
                name: name.clone(),
                code: t.code.clone(),
                issuer,
                minter,
                contract: asset_contract_id(passphrase, &t.code, &issuer),
            },
        );
    }
    // The settlement token as a holding's source: minted by the admin when
    // it issues it.
    let settlement_minter = settlement_asset
        .as_ref()
        .filter(|(_, issuer)| *issuer == keys.admin)
        .map(|_| m.env.admin.clone());
    let mut accounts = Vec::new();
    for (name, spec) in &m.env.accounts {
        let key = *keys
            .accounts
            .get(name)
            .ok_or_else(|| anyhow!("no key for account {name}"))?;
        let asset_of = |t: &str| -> Result<(String, Key)> {
            if t == "settlement" {
                settlement_asset.clone().ok_or_else(|| {
                    anyhow!("accounts.{name}: the settlement token is no Stellar asset to trust")
                })
            } else if let Some(d) = declared_tokens.get(t) {
                Ok((d.code.clone(), d.issuer))
            } else {
                crate::manifest::parse_asset(t)
                    .ok_or_else(|| anyhow!("accounts.{name}.trustlines: {t:?}"))
            }
        };
        let trustlines = spec
            .trustlines
            .iter()
            .map(|t| asset_of(t))
            .collect::<Result<Vec<_>>>()?;
        let mut balances = Vec::new();
        for (token, amount) in &spec.balances {
            let want = caravel_node::plugin::parse_amount(amount, Some(7))
                .with_context(|| format!("accounts.{name}.balances.{token}"))?;
            let (code, issuer) = asset_of(token)?;
            let (contract, minter) = match declared_tokens.get(token) {
                Some(d) => (d.contract, d.minter.clone()),
                None => (a.token, settlement_minter.clone()),
            };
            balances.push(crate::plan::Holding {
                token: token.clone(),
                code,
                issuer,
                contract,
                minter,
                want,
            });
        }
        accounts.push(crate::plan::DeclaredAccount {
            name: name.clone(),
            identity: spec.identity_of(name).to_string(),
            key,
            fund: spec.fund.unwrap_or(true),
            trustlines,
            balances,
            depends_on: spec.depends_on.clone(),
        });
    }
    // Declared contracts: the Wasm's hash (from its file, or as given), and
    // the address from the deployer and a salt that is the lane's.
    let lane_dir = m
        .doc
        .as_ref()
        .and_then(|doc| doc.sources.0.first())
        .and_then(|src| src.path.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_default();
    let mut contracts = Vec::new();
    for (name, c) in &m.env.contracts {
        let (wasm, wasm_file) = match c.wasm_hash() {
            Some(h) => (h, None),
            None => {
                let path = lane_dir.join(&c.wasm);
                let bytes = std::fs::read(&path).with_context(|| {
                    format!("contracts.{name}.wasm: reading {}", path.display())
                })?;
                (caravel_runtime::checkpoint::sha256(&bytes), Some(path))
            }
        };
        let (deployer, deployer_key) = match c.deployer() {
            "admin" => (m.env.admin.clone(), keys.admin),
            d => (
                m.env
                    .accounts
                    .get(d)
                    .map_or(d, |a| a.identity_of(d))
                    .to_string(),
                *keys
                    .accounts
                    .get(d)
                    .ok_or_else(|| anyhow!("no key for account {d}"))?,
            ),
        };
        let salt =
            crate::address::contract_salt(&genesis.lane_id, name, c.salt.as_deref().unwrap_or(""));
        contracts.push(crate::plan::DeclaredContract {
            name: name.clone(),
            wasm,
            wasm_file,
            deployer,
            deployer_key,
            salt,
            address: contract_id(passphrase, &deployer_key, &salt),
            args: c.arg_strings(),
            depends_on: c.depends_on.clone(),
        });
    }
    // A declared token that is the settlement token is `token.settlement`.
    let tokens: Vec<crate::plan::DeclaredToken> = declared_tokens
        .into_values()
        .filter(|t| t.contract != a.token)
        .collect();
    Ok(Desired {
        lane_name: m.lane.lane.name.clone(),
        env: m.env_name.clone(),
        network: m.env.network,
        lane_id: genesis.lane_id,
        config_hash: genesis.config_hash,
        genesis_state_hash: genesis.genesis_state_hash,
        engine_wasm_hash,
        admin: keys.admin,
        relayer: keys.relayer,
        token: a.token,
        token_asset: a.token_asset.clone(),
        settlement_asset,
        settlement: a.settlement,
        settlement_pinned: a.settlement_pinned,
        settlement_wasm,
        params: Params {
            force_inclusion_window_secs: sp.force_inclusion_window_secs,
            escape_timeout_secs: sp.escape_timeout_secs,
            min_rotation_delay_secs: sp.min_rotation_delay_secs,
            signer_retention_epochs: sp.signer_retention_epochs,
            min_deposit: m.min_deposit()?,
        },
        signers: SignerSet {
            signers,
            threshold: m.env.threshold,
        },
        validators: m
            .env
            .validators
            .iter()
            .map(|v| validator_node(&v.name))
            .collect(),
        host: DesiredHost {
            provider: m.env.host.provider,
            release: release.to_string(),
            platform: None,
            files: BTreeMap::new(),
        },
        vars: m.vars.clone(),
        accounts,
        tokens,
        contracts,
        others: m
            .env
            .other_hosts()
            .into_iter()
            .map(|(name, spec)| crate::plan::OtherHost {
                nodes: m
                    .env
                    .validators
                    .iter()
                    .map(|v| validator_node(&v.name))
                    .filter(|n| m.env.host_of(n) == name)
                    .collect(),
                host: DesiredHost {
                    provider: spec.provider,
                    release: release.to_string(),
                    platform: None,
                    files: BTreeMap::new(),
                },
                name,
            })
            .collect(),
        primary_host: m.env.primary_host(),
    })
}

/// Where users reach the lane's API: the sequencer on this machine, or the
/// host's public URL.
pub fn api_url(m: &Manifest) -> Option<String> {
    match m.env.host.provider {
        Provider::Local => Some(format!("http://127.0.0.1:{}", m.env.sequencer.port)),
        Provider::Ssh => m
            .env
            .host
            .public_url
            .as_deref()
            .map(|u| u.trim_end_matches('/').to_string()),
    }
}

/// Validator `i`'s API (0-based, in file order), where users reach it.
pub fn validator_url(m: &Manifest, i: usize) -> Option<String> {
    match m.env.host.provider {
        Provider::Local => Some(format!("http://127.0.0.1:{}", render::validator_port(m, i))),
        Provider::Ssh => api_url(m).map(|u| format!("{u}/validators/{}", m.env.validators[i].name)),
    }
}

/// The host a deployment runs on, without reading it.
pub fn host_provider(m: &Manifest, template: &str, state_root: &Path) -> Result<HostProvider> {
    Ok(match m.env.host.provider {
        Provider::Local => HostProvider::Local(Local::new(m, template, state_root)?),
        Provider::Ssh => HostProvider::Ssh(crate::ssh::Ssh::new(m, template, state_root)?),
    })
}

/// The host `node` runs on (C-22), without reading it.
pub fn node_host_provider(
    m: &Manifest,
    template: &str,
    state_root: &Path,
    node: &str,
) -> Result<HostProvider> {
    HostProvider::named(m, template, state_root, &m.env.host_of(node))
}

/// Resolves the deployment and reads what Stellar and the host have.
/// `for_apply` starts the local network if it isn't up; `plan` never does.
/// A local host keeps its processes under `<state_root>/.caravel/`.
#[allow(clippy::too_many_arguments)]
pub async fn prepare(
    t: &dyn Template,
    lane_path: &Path,
    env: &str,
    inputs: &crate::manifest::Inputs,
    source: &crate::cli::ReleaseArgs,
    for_apply: bool,
    state_root: &Path,
) -> Result<Prepared> {
    Cli::check_version()?;
    let mut m = Manifest::load_with(lane_path, env, inputs)?;
    let mut notes = Vec::new();
    let template = m
        .lane
        .app
        .as_ref()
        .map(|a| a.template.clone())
        .ok_or_else(|| anyhow!("the lane file needs [app] (template and engine_wasm_sha256)"))?;
    if template != t.name() {
        bail!(
            "the lane file runs the {template} template, not {}",
            t.name()
        );
    }
    let genesis = t.genesis(&m.lane)?;
    let engine_wasm_hash = m
        .lane
        .engine_wasm_hash()?
        .ok_or_else(|| anyhow!("[app] engine_wasm_sha256 is required"))?;

    let mut release = Release::locate(source.release_dir.as_deref(), &template)?;
    if let Some(w) = &source.wasm_dir {
        release.use_wasm_from(w)?;
    }
    if release.wasm_hash(&engine_file(&template))? != engine_wasm_hash {
        bail!(
            "the release's {} is not the engine the lane file names",
            engine_file(&template)
        );
    }
    let record = parse_hex(
        crate::versions::settlement_wasm(),
        "the settlement build of record",
    )?;
    let settlement_wasm = match &m.env.settlement_wasm {
        Some(h) => parse_hex(h, "settlement_wasm")?,
        None => {
            let built = release.wasm_hash("settlement.wasm")?;
            if built != record {
                if m.env.network != Network::Local {
                    bail!(
                        "the release's settlement.wasm is not the build of record {}… (DEC-033): use the CI release (--release-dir)",
                        &crate::versions::settlement_wasm()[..8]
                    );
                }
                notes.push("this machine's settlement.wasm is not the Linux build of record (DEC-033); fine on a local network".into());
                built
            } else {
                record
            }
        }
    };

    let keys = Keys::from_keystore(&m)?;
    let addrs = addresses(&m, &keys.admin)?;
    // A template may need a token with a set number of decimals (perps: 7).
    // A Stellar Asset Contract always has 7; another contract is asked.
    if let Some(want) = t.token_decimals() {
        let have = match &m.env.token {
            Token::Contract { .. } => Cli::new(&m).token_decimals(&m.env.admin, &addrs.token)?,
            _ => 7,
        };
        if have != want {
            bail!(
                "the {} template needs a token with {want} decimals; {} has {have}",
                template,
                crate::address::strkey(&addrs.token)
            );
        }
    }
    let host_provider = match m.env.host.provider {
        Provider::Local => HostProvider::Local(Local::new(&m, &template, state_root)?),
        Provider::Ssh => HostProvider::Ssh(crate::ssh::Ssh::new(&m, &template, state_root)?),
    };
    let mut others: Vec<OtherRun> = m
        .env
        .other_hosts()
        .into_iter()
        .map(|(name, _)| {
            Ok(OtherRun {
                provider: HostProvider::named(&m, &template, state_root, &name)?,
                name,
                files: BTreeMap::new(),
            })
        })
        .collect::<Result<_>>()?;
    let mut desired = desired(
        &m,
        &genesis,
        &keys,
        &addrs,
        engine_wasm_hash,
        settlement_wasm,
        &release.commit,
    )?;
    desired.host.platform = release::binary_platform(&release.node_binary)?;
    // What the lane file left for now: values that need keys and addresses.
    let attrs = crate::attrs::attributes(&m, &keys, &addrs, Some(&genesis), Some(&release.commit));
    m.finish(&attrs)?;
    // Contracts' arguments, now with the values they were waiting for.
    for c in &mut desired.contracts {
        if let Some(spec) = m.env.contracts.get(&c.name) {
            c.args = spec.arg_strings();
        }
    }

    if for_apply {
        stellar::ensure_local_network(&m)?;
    }
    let rpc = Rpc::new(m.rpc_url())?;
    let (chain, extra) = match chain::read(&rpc, &desired).await {
        Ok(c) => c,
        Err(e) if m.env.network == Network::Local && !for_apply => {
            notes.push(format!(
                "the local network does not answer ({e:#}); apply starts it"
            ));
            (Chain::default(), Extra::default())
        }
        Err(e) => return Err(e.context(format!("reading Stellar at {}", m.rpc_url()))),
    };
    let epoch = plan::target_epoch(&desired, &chain);
    let resolved = Resolved {
        template,
        engine_wasm_hash,
        settlement: addrs.settlement,
        validator_keys: keys.validators.clone(),
        web: release.web.is_some(),
    };
    let files = render::render(&m, &resolved, &host_provider.root_str(), epoch)?;
    desired.host.files = files
        .iter()
        .map(|(k, v)| (k.clone(), sha256(v.as_bytes())))
        .collect();
    let mut host = host_provider.read(&m).await?;
    // Each other host: its own files, and what it has (C-22).
    let platform = desired.host.platform.clone();
    for o in &mut others {
        o.files = render::render_for(&m, &resolved, &o.name, &o.provider.root_str(), epoch)?;
        let want = desired
            .others
            .iter_mut()
            .find(|d| d.name == o.name)
            .expect("each other host is desired");
        want.host.files = o
            .files
            .iter()
            .map(|(k, v)| (k.clone(), sha256(v.as_bytes())))
            .collect();
        want.host.platform = platform.clone();
        host.others
            .insert(o.name.clone(), o.provider.read(&m).await?);
    }
    let cli = Cli::new(&m);
    let validator_keys = m
        .env
        .validators
        .iter()
        .zip(&keys.validators)
        .map(|(v, k)| (validator_node(&v.name), *k))
        .collect();
    Ok(Prepared {
        m,
        attrs,
        desired,
        validator_keys,
        files,
        release,
        cli,
        host_provider,
        chain,
        extra,
        host,
        notes,
        others,
    })
}

impl Prepared {
    pub fn plan(&self) -> Plan {
        plan::diff(&self.desired, &self.chain, &self.host)
    }

    /// The plan narrowed by `--target` or forced by `--replace`.
    pub fn plan_with(&self, opts: &plan::Options) -> Result<Plan> {
        plan::diff_with(&self.desired, &self.chain, &self.host, opts).map_err(|e| anyhow!(e))
    }

    /// The deployment's resources and their edges (`caravel graph`).
    pub fn graph(&self) -> Result<crate::graph::Graph> {
        crate::graph::build(&self.desired, &self.host).map_err(|e| anyhow!(e))
    }

    fn identity_of<'a>(&'a self, who: &'a str) -> &'a str {
        match who {
            "admin" => &self.m.env.admin,
            "relayer" => &self.m.env.relayer.account,
            name => self
                .m
                .env
                .accounts
                .get(name)
                .map_or(name, |a| a.identity_of(name)),
        }
    }

    /// Every node of the lane, and every node the host runs for it.
    pub fn all_nodes(&self) -> Vec<String> {
        let mut all: Vec<String> = vec!["sequencer".into(), "relayer".into()];
        all.extend(self.desired.validators.iter().cloned());
        for n in self.host.nodes.keys() {
            if !all.contains(n) {
                all.push(n.clone());
            }
        }
        all
    }

    fn want_report(&self, node: &str) -> NodeReport {
        NodeReport {
            lane_id: self.desired.lane_id,
            config_hash: self.desired.config_hash,
            settlement: self.desired.settlement,
            engine_wasm_hash: self.desired.engine_wasm_hash,
            key: self.validator_keys.get(node).copied(),
            epoch: None,
            release: None,
        }
    }

    fn port_of(&self, node: &str) -> Option<u16> {
        if node == "sequencer" {
            return Some(self.m.env.sequencer.port);
        }
        self.m
            .env
            .validators
            .iter()
            .position(|v| validator_node(&v.name) == node)
            .map(|i| render::validator_port(&self.m, i))
    }

    /// Exports the keys the host's nodes hold, from the user's keystore.
    /// Host `host`'s provider: `None` (or its own name) is the sequencer's.
    pub fn provider(&self, host: Option<&str>) -> &HostProvider {
        match host {
            Some(h) if h != self.desired.primary_host => self
                .others
                .iter()
                .find(|o| o.name == h)
                .map_or(&self.host_provider, |o| &o.provider),
            _ => &self.host_provider,
        }
    }

    /// The provider of the host `node` runs on.
    pub fn provider_of(&self, node: &str) -> &HostProvider {
        self.provider(self.desired.host_of(node))
    }

    /// Host `host`'s rendered files.
    fn files_of(&self, host: Option<&str>) -> &BTreeMap<String, String> {
        match host {
            Some(h) if h != self.desired.primary_host => self
                .others
                .iter()
                .find(|o| o.name == h)
                .map_or(&self.files, |o| &o.files),
            _ => &self.files,
        }
    }

    /// The keys host `host` needs: its validators' only, and on the
    /// sequencer's host the relayer's and the feed modules' too.
    fn write_keys(&self, host: Option<&str>) -> Result<()> {
        let validators = self
            .m
            .env
            .validators
            .iter()
            .filter(|v| self.desired.host_of(&validator_node(&v.name)) == host)
            .map(|v| Ok((validator_node(&v.name), Cli::secret(&v.key)?)))
            .collect::<Result<Vec<_>>>()?;
        let mut env = Vec::new();
        if host.is_none() {
            env.push((
                "CARAVEL_RELAYER_SECRET".to_string(),
                Cli::secret(&self.m.env.relayer.account)?,
            ));
            for (var, id) in &self.m.env.relayer.feed_keys {
                env.push((var.clone(), Cli::secret(id)?));
            }
        }
        self.provider(host).write_keys(&validators, &env)
    }

    async fn start(&self, node: &str) -> Result<()> {
        let on = self.desired.host_of(node);
        let files = match on {
            None => &self.desired.host.files,
            Some(h) => {
                &self
                    .desired
                    .others
                    .iter()
                    .find(|o| o.name == h)
                    .expect("a host")
                    .host
                    .files
            }
        };
        let fingerprint = plan::fingerprint(files, node);
        let provider = self.provider(on);
        provider.start(node, &fingerprint)?;
        let host_name = self.m.env.host_of(node);
        let addr = self.m.env.listen_on(&host_name, node);
        let result = match self.port_of(node) {
            Some(port) => {
                provider
                    .wait_healthy(
                        &addr,
                        port,
                        &self.want_report(node),
                        Duration::from_secs(90),
                    )
                    .await
            }
            None => {
                tokio::time::sleep(Duration::from_secs(3)).await;
                if provider
                    .read(&self.m)
                    .await?
                    .nodes
                    .get(node)
                    .is_some_and(|n| n.running)
                {
                    Ok(())
                } else {
                    Err(anyhow!("{node} exited"))
                }
            }
        };
        result.map_err(|e| anyhow!("{e:#}\n--- {node} log\n{}", provider.log_tail(node)))
    }

    /// Carries out `plan`'s steps in order. Each Stellar step is checked
    /// against what the plan expected (the derived address, the Wasm hash).
    pub async fn apply(&self, plan: &Plan) -> Result<()> {
        if !plan.problems.is_empty() {
            bail!("the plan has problems; apply changes nothing until they are fixed");
        }
        let d = &self.desired;
        let admin = self.m.env.admin.as_str();
        // Each host's keys, once, before the first node it starts.
        let mut keys_written: std::collections::BTreeSet<Option<String>> = Default::default();
        for step in &plan.steps {
            eprintln!("→ {}", step_line(step));
            match step {
                Step::Fund { who, .. } => self.cli.fund(self.identity_of(who))?,
                Step::Trust { who, code, issuer } => {
                    self.cli.change_trust(self.identity_of(who), code, issuer)?
                }
                Step::Mint {
                    key,
                    amount,
                    contract,
                    minter,
                    ..
                } => self.cli.mint(minter, contract, key, *amount)?,
                Step::DeployToken { code, issuer, .. } => {
                    self.cli.deploy_asset(admin, code, issuer)?
                }
                Step::UploadWasm { name, hash } => {
                    let path = match d.contracts.iter().find(|c| c.name == *name) {
                        Some(c) => c
                            .wasm_file
                            .clone()
                            .ok_or_else(|| anyhow!("contract {name} has no Wasm file"))?,
                        None => self.release.wasm_path("settlement.wasm"),
                    };
                    let got = self.cli.upload(admin, &path)?;
                    if got != *hash {
                        bail!(
                            "uploaded Wasm hashes to {}, not the planned one",
                            strkey(&got)
                        );
                    }
                }
                Step::DeployContract {
                    contract,
                    deployer,
                    wasm,
                    salt,
                    args,
                    ..
                } => {
                    let got = self.cli.deploy_contract(deployer, wasm, salt, args)?;
                    if got != *contract {
                        bail!(
                            "the contract was deployed at {}, not at the planned {}",
                            strkey(&got),
                            strkey(contract)
                        );
                    }
                }
                Step::DeploySettlement { contract } => {
                    let got = self.cli.deploy_settlement(
                        admin,
                        &d.settlement_wasm,
                        &settlement_salt(&d.lane_id),
                        &d.admin,
                        &d.token,
                        [
                            &d.lane_id,
                            &d.engine_wasm_hash,
                            &d.genesis_state_hash,
                            &d.config_hash,
                        ],
                        &d.signers,
                        &d.params,
                    )?;
                    if got != *contract {
                        bail!(
                            "the contract was deployed at {}, not at the planned {}",
                            strkey(&got),
                            strkey(contract)
                        );
                    }
                }
                Step::WipeHostData { host } => {
                    let nodes: Vec<String> = self
                        .all_nodes()
                        .into_iter()
                        .filter(|n| self.desired.host_of(n) == host.as_deref())
                        .collect();
                    self.provider(host.as_deref()).wipe(&nodes)?
                }
                Step::InstallRelease { host, .. } => self
                    .provider(host.as_deref())
                    .install_release(&self.release)?,
                Step::WriteFile { host, path } => {
                    let files = self.files_of(host.as_deref());
                    self.provider(host.as_deref())
                        .write_file(path, &files[path])?
                }
                Step::Start { node } | Step::Restart { node } => {
                    let on = self.desired.host_of(node).map(String::from);
                    if keys_written.insert(on.clone()) {
                        self.write_keys(on.as_deref())?;
                    }
                    self.provider_of(node).stop(node)?;
                    self.start(node).await?;
                }
                Step::Stop { node, host } => match host {
                    // Moved: stopped where it ran, and its key goes with it.
                    Some(h) => {
                        self.provider(Some(h)).stop(node)?;
                        let on = (*h != self.desired.primary_host).then(|| h.clone());
                        if keys_written.insert(on.clone()) {
                            self.write_keys(on.as_deref())?;
                        }
                    }
                    None => self.provider_of(node).stop(node)?,
                },
                Step::RotateSigners { .. } => {
                    self.cli.rotate_signers(admin, &d.settlement, &d.signers)?
                }
            }
        }
        Ok(())
    }
}

impl Prepared {
    /// A unified diff of each file the plan would write: the host's version
    /// against the one the lane file gives.
    pub fn file_diffs(&self, plan: &Plan) -> Result<String> {
        let dir = std::env::temp_dir().join(format!("caravel-diff-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let mut out = String::new();
        for step in &plan.steps {
            let Step::WriteFile { host, path } = step else {
                continue;
            };
            let old = self.provider(host.as_deref()).read_file(path)?;
            let (a, b) = (dir.join("host"), dir.join("file"));
            std::fs::write(&a, old.as_deref().unwrap_or(""))?;
            std::fs::write(&b, &self.files_of(host.as_deref())[path])?;
            let d = std::process::Command::new("diff")
                .args(["-u", "--label"])
                .arg(if old.is_some() {
                    format!("host/{path}")
                } else {
                    "/dev/null (new on the host)".into()
                })
                .args(["--label", &format!("lane-file/{path}")])
                .arg(&a)
                .arg(&b)
                .output()?;
            out += &String::from_utf8_lossy(&d.stdout);
        }
        let _ = std::fs::remove_dir_all(&dir);
        Ok(out)
    }
}

fn on(host: &Option<String>) -> String {
    host.as_deref()
        .map(|h| format!(" on {h}"))
        .unwrap_or_default()
}

/// One step as `apply` prints it.
fn step_line(s: &Step) -> String {
    match s {
        Step::Fund { who, .. } => format!("fund {who}"),
        Step::Trust { who, code, .. } => format!("{who} trusts {code}"),
        Step::Mint {
            who, amount, code, ..
        } => {
            format!(
                "mint {} {code} to {who}",
                crate::flows::format_units(*amount, 7)
            )
        }
        Step::DeployToken { contract, .. } => format!("create token contract {}", strkey(contract)),
        Step::UploadWasm { name, .. } => format!("upload the {name} Wasm"),
        Step::DeployContract { name, contract, .. } => {
            format!("deploy {name} {}", strkey(contract))
        }
        Step::DeploySettlement { contract } => format!("deploy settlement {}", strkey(contract)),
        Step::WipeHostData { host } => format!(
            "wipe {}'s lane data",
            host.as_deref()
                .map_or("the host".into(), |h| format!("host {h}"))
        ),
        Step::InstallRelease { host, to, .. } => format!("install release {to}{}", on(host)),
        Step::WriteFile { host, path } => format!("write {path}{}", on(host)),
        Step::Start { node } => format!("start {node}"),
        Step::Restart { node } => format!("restart {node}"),
        Step::Stop { node, host } => format!("stop {node}{}", on(host)),
        Step::RotateSigners { to_epoch, .. } => format!("rotate signers to epoch {to_epoch}"),
    }
}

/// Asks before changing anything, unless `yes`.
pub fn confirm(steps: usize, yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        bail!("apply changes things on Stellar and the host: pass --yes to run without a prompt");
    }
    eprint!("Apply these {steps} step(s)? [y/N] ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("reading the answer")?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes"))
}

/// Destroy can't be undone: the operator types the lane's name.
pub fn confirm_destroy(lane: &str) -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("destroy freezes the lane for good: pass --yes to run without a prompt");
    }
    eprint!("This freezes lane {lane} for good; users exit with their proofs. Type the lane's name to go on: ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("reading the answer")?;
    Ok(line.trim() == lane)
}
