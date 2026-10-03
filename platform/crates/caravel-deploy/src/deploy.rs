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
    let host = host_provider.read(&m).await?;
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
    pub fn graph(&self) -> crate::graph::Graph {
        crate::graph::build(&self.desired, &self.host)
    }

    fn identity_of(&self, who: &str) -> &str {
        match who {
            "admin" => &self.m.env.admin,
            _ => &self.m.env.relayer.account,
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
    fn write_keys(&self) -> Result<()> {
        let validators = self
            .m
            .env
            .validators
            .iter()
            .map(|v| Ok((validator_node(&v.name), Cli::secret(&v.key)?)))
            .collect::<Result<Vec<_>>>()?;
        let mut env = vec![(
            "CARAVEL_RELAYER_SECRET".to_string(),
            Cli::secret(&self.m.env.relayer.account)?,
        )];
        for (var, id) in &self.m.env.relayer.feed_keys {
            env.push((var.clone(), Cli::secret(id)?));
        }
        self.host_provider.write_keys(&validators, &env)
    }

    async fn start(&self, node: &str) -> Result<()> {
        let fingerprint = plan::fingerprint(&self.desired.host.files, node);
        self.host_provider.start(node, &fingerprint)?;
        let result = match self.port_of(node) {
            Some(port) => {
                self.host_provider
                    .wait_healthy(port, &self.want_report(node), Duration::from_secs(90))
                    .await
            }
            None => {
                tokio::time::sleep(Duration::from_secs(3)).await;
                if self
                    .host_provider
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
        result.map_err(|e| {
            anyhow!(
                "{e:#}\n--- {node} log\n{}",
                self.host_provider.log_tail(node)
            )
        })
    }

    /// Carries out `plan`'s steps in order. Each Stellar step is checked
    /// against what the plan expected (the derived address, the Wasm hash).
    pub async fn apply(&self, plan: &Plan) -> Result<()> {
        if !plan.problems.is_empty() {
            bail!("the plan has problems; apply changes nothing until they are fixed");
        }
        let d = &self.desired;
        let admin = self.m.env.admin.as_str();
        let mut keys_written = false;
        for step in &plan.steps {
            eprintln!("→ {}", step_line(step));
            match step {
                Step::Fund { who, .. } => self.cli.fund(self.identity_of(who))?,
                Step::DeployToken { code, issuer, .. } => {
                    self.cli.deploy_asset(admin, code, issuer)?
                }
                Step::UploadWasm { hash } => {
                    let got = self
                        .cli
                        .upload(admin, &self.release.wasm_path("settlement.wasm"))?;
                    if got != *hash {
                        bail!(
                            "uploaded Wasm hashes to {}, not the planned one",
                            strkey(&got)
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
                Step::WipeHostData => self.host_provider.wipe(&self.all_nodes())?,
                Step::InstallRelease { .. } => self.host_provider.install_release(&self.release)?,
                Step::WriteFile { path } => {
                    self.host_provider.write_file(path, &self.files[path])?
                }
                Step::Start { node } | Step::Restart { node } => {
                    if !keys_written {
                        self.write_keys()?;
                        keys_written = true;
                    }
                    self.host_provider.stop(node)?;
                    self.start(node).await?;
                }
                Step::Stop { node } => self.host_provider.stop(node)?,
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
            let Step::WriteFile { path } = step else {
                continue;
            };
            let old = self.host_provider.read_file(path)?;
            let (a, b) = (dir.join("host"), dir.join("file"));
            std::fs::write(&a, old.as_deref().unwrap_or(""))?;
            std::fs::write(&b, &self.files[path])?;
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

/// One step as `apply` prints it.
fn step_line(s: &Step) -> String {
    match s {
        Step::Fund { who, .. } => format!("fund {who}"),
        Step::DeployToken { contract, .. } => format!("create token contract {}", strkey(contract)),
        Step::UploadWasm { .. } => "upload the settlement Wasm".into(),
        Step::DeploySettlement { contract } => format!("deploy settlement {}", strkey(contract)),
        Step::WipeHostData => "wipe the host's lane data".into(),
        Step::InstallRelease { to, .. } => format!("install release {to}"),
        Step::WriteFile { path } => format!("write {path}"),
        Step::Start { node } => format!("start {node}"),
        Step::Restart { node } => format!("restart {node}"),
        Step::Stop { node } => format!("stop {node}"),
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
