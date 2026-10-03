//! The plan: what a deployment needs against what Stellar and the host have
//! (spec §20.3). Pure: the readers and the node-config renderer feed it, and
//! `apply` carries it out. There is no state file: the lane file and the
//! chain are the whole truth, and the host reports what it runs.
//!
//! Anything the settlement constructor fixed (the lane, its genesis, the
//! engine, the admin, USDC, the params) can't change on a deployed lane: the
//! plan reports it as a problem, and the fix is to destroy the lane or give it
//! a new name. Problems block `apply`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use crate::address::strkey;
use crate::manifest::{Network, Provider};

pub type Key = [u8; 32];

/// The settlement constructor's `Params`, `min_deposit` resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Params {
    pub force_inclusion_window_secs: u64,
    pub escape_timeout_secs: u64,
    pub min_rotation_delay_secs: u64,
    pub signer_retention_epochs: u32,
    pub min_deposit: i128,
}

/// A weighted signer set, keys ascending (spec §13.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignerSet {
    pub signers: Vec<(Key, u32)>,
    pub threshold: u32,
}

/// The lane file's deployment, resolved: identities to keys, addresses derived.
#[derive(Clone, Debug)]
pub struct Desired {
    pub lane_name: String,
    pub env: String,
    pub network: Network,
    pub lane_id: Key,
    pub config_hash: Key,
    pub genesis_state_hash: Key,
    pub engine_wasm_hash: Key,
    pub admin: Key,
    pub relayer: Key,
    /// The settlement token's contract.
    pub token: Key,
    /// The Stellar asset behind it (code, issuer), when it is a Stellar Asset
    /// Contract that `apply` may deploy.
    pub token_asset: Option<(String, Key)>,
    pub settlement: Key,
    /// Named in the lane file (deployed before this tool), not derived.
    pub settlement_pinned: bool,
    pub settlement_wasm: Key,
    pub params: Params,
    pub signers: SignerSet,
    /// `validator-<name>` for each validator, in file order.
    pub validators: Vec<String>,
    pub host: DesiredHost,
    /// The lane file's vars and values (sensitive ones masked), shown in
    /// the plan; empty when it declares none.
    pub vars: String,
}

#[derive(Clone, Debug)]
pub struct DesiredHost {
    pub provider: Provider,
    /// The release's commit.
    pub release: String,
    /// What the release's node binary runs on (`uname -sm` form, e.g.
    /// "Linux x86_64"), when it is known.
    pub platform: Option<String>,
    /// Every generated file (path under the lane's config dir → sha256).
    pub files: BTreeMap<String, Key>,
}

/// What Stellar has.
#[derive(Clone, Debug, Default)]
pub struct Chain {
    /// Accounts that exist.
    pub accounts: BTreeSet<Key>,
    pub token_exists: bool,
    pub settlement_wasm_uploaded: bool,
    pub settlement: Option<OnChain>,
}

/// A settlement contract as the ledger has it.
#[derive(Clone, Debug)]
pub struct OnChain {
    pub code: Key,
    pub admin: Key,
    pub token: Key,
    pub lane_id: Key,
    pub engine_wasm_hash: Key,
    pub genesis_state_hash: Key,
    pub config_hash: Key,
    pub params: Params,
    pub epoch: u64,
    pub signers: SignerSet,
    /// The epoch the lane file's signer set was installed at, if it ever was
    /// (`SignersEpoch(hash)`): a set can be installed only once.
    pub desired_set_epoch: Option<u64>,
    pub frozen: bool,
}

/// What the host has. An empty host has nothing of the lane yet.
#[derive(Clone, Debug, Default)]
pub struct Host {
    /// Prerequisites the host lacks.
    pub missing: Vec<String>,
    /// `uname -sm`, when the host reports it (ssh hosts).
    pub platform: Option<String>,
    pub release: Option<String>,
    pub files: BTreeMap<String, Key>,
    pub nodes: BTreeMap<String, NodeState>,
    /// The lane's stores exist on the host.
    pub has_data: bool,
}

#[derive(Clone, Debug, Default)]
pub struct NodeState {
    pub running: bool,
    /// Its `/v1/status`, when it answers.
    pub report: Option<NodeReport>,
    /// The fingerprint of the config files it was started with
    /// ([`fingerprint`]); a node started on older files needs a restart.
    pub started_with: Option<Key>,
}

/// `H(file hashes)` of the config files a node reads.
pub fn fingerprint(files: &BTreeMap<String, Key>, node: &str) -> Key {
    let mut all = Vec::new();
    for f in crate::render::node_files(node) {
        all.extend(files.get(&f).copied().unwrap_or([0; 32]));
    }
    caravel_runtime::checkpoint::sha256(&all)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeReport {
    pub lane_id: Key,
    pub config_hash: Key,
    pub settlement: Key,
    pub engine_wasm_hash: Key,
    /// A validator's key.
    pub key: Option<Key>,
    /// The sequencer's signer epoch.
    pub epoch: Option<u64>,
    /// The commit its binary was built from (CI releases only).
    pub release: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Friendbot funds a new account.
    Fund {
        who: &'static str,
        key: Key,
    },
    /// The Stellar Asset Contract of the settlement token's asset.
    DeployToken {
        contract: Key,
        code: String,
        issuer: Key,
    },
    UploadWasm {
        hash: Key,
    },
    DeploySettlement {
        contract: Key,
    },
    /// Stop every node and remove its store: they belong to a contract that is gone.
    WipeHostData,
    InstallRelease {
        from: Option<String>,
        to: String,
    },
    WriteFile {
        path: String,
    },
    Start {
        node: String,
    },
    Restart {
        node: String,
    },
    Stop {
        node: String,
    },
    /// `admin_rotate_signers`, a testnet-only admin power.
    RotateSigners {
        from_epoch: u64,
        to_epoch: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// A field the constructor fixed differs from the lane file.
    Immutable {
        field: &'static str,
        deployed: String,
        file: String,
    },
    /// The contract runs a known build other than the lane file's.
    CodeDrift {
        code: Key,
        want: Key,
    },
    /// The contract runs a build nobody recorded.
    UnknownCode {
        code: Key,
    },
    Frozen,
    /// The lane file names a settlement contract the network doesn't have.
    SettlementMissing {
        contract: Key,
    },
    /// The settlement token contract isn't on the network.
    TokenMissing {
        contract: Key,
    },
    /// The lane file's signer set was installed before, at this epoch.
    SignersReused {
        epoch: u64,
    },
    HostNotReady {
        missing: Vec<String>,
    },
    /// A node runs something its files don't say.
    /// The release's node binary can't run on the host.
    WrongPlatform {
        release: String,
        host: String,
    },
    NodeMismatch {
        node: String,
        field: &'static str,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
    pub problems: Vec<Problem>,
    /// The signer epoch after apply; the sequencer's config carries it.
    pub target_epoch: u64,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty() && self.problems.is_empty()
    }
}

/// The signer epoch after apply: 1 for a new contract, the next epoch when the
/// set changes to one never installed. The node configs are rendered with it
/// before `diff`.
pub fn target_epoch(d: &Desired, chain: &Chain) -> u64 {
    match &chain.settlement {
        None => 1,
        Some(oc) if oc.signers == d.signers || oc.desired_set_epoch.is_some() => oc.epoch,
        Some(oc) => oc.epoch + 1,
    }
}

pub(crate) fn hex(k: &[u8]) -> String {
    k.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn short(k: &Key) -> String {
    hex(&k[..4]) + "…"
}

pub(crate) fn g_short(k: &Key) -> String {
    let s = stellar_strkey::ed25519::PublicKey(*k).to_string();
    let s = s.as_str();
    format!("{}…{}", &s[..4], &s[s.len() - 4..])
}

pub(crate) fn c_short(k: &Key) -> String {
    let s = strkey(k);
    format!("{}…{}", &s[..4], &s[s.len() - 4..])
}

/// The nodes a changed file restarts.
#[cfg(test)]
fn nodes_of(path: &str, all: &[String]) -> Vec<String> {
    match path {
        "lane.toml" => all.to_vec(),
        "systemd/caravel-sequencer.service" => vec!["sequencer".into()],
        "systemd/caravel-relayer.service" => vec!["relayer".into()],
        "systemd/caravel-validator@.service" => all
            .iter()
            .filter(|n| n.starts_with("validator-"))
            .cloned()
            .collect(),
        "sequencer.toml" => vec!["sequencer".into()],
        "relayer.json" => vec!["relayer".into()],
        p => match p.strip_suffix(".toml") {
            Some(node) if node.starts_with("validator-") => vec![node.to_string()],
            _ => Vec::new(),
        },
    }
}

/// The plan for `d` against what the chain and the host have, from the
/// deployment's resource graph ([`crate::graph`]).
pub fn diff(d: &Desired, chain: &Chain, host: &Host) -> Plan {
    crate::graph::diff(d, chain, host)
}

/// The fixed-order diff the graph replaced (C-15), kept as the reference the
/// graph's plans are checked against until C-16 changes the plan's lines.
#[cfg(test)]
pub(crate) fn legacy_diff(d: &Desired, chain: &Chain, host: &Host) -> Plan {
    let mut steps = Vec::new();
    let mut problems = Vec::new();

    // --- Stellar ---------------------------------------------------------------
    for (who, key) in [("admin", d.admin), ("relayer", d.relayer)] {
        if !chain.accounts.contains(&key) {
            steps.push(Step::Fund { who, key });
        }
    }
    if !chain.token_exists {
        match &d.token_asset {
            Some((code, issuer)) => steps.push(Step::DeployToken {
                contract: d.token,
                code: code.clone(),
                issuer: *issuer,
            }),
            None => problems.push(Problem::TokenMissing { contract: d.token }),
        }
    }
    let mut fresh = false;
    let mut rotate = None;
    match &chain.settlement {
        None if d.settlement_pinned => problems.push(Problem::SettlementMissing {
            contract: d.settlement,
        }),
        None => {
            if !chain.settlement_wasm_uploaded {
                steps.push(Step::UploadWasm {
                    hash: d.settlement_wasm,
                });
            }
            steps.push(Step::DeploySettlement {
                contract: d.settlement,
            });
            fresh = true;
        }
        Some(oc) => {
            if oc.code != d.settlement_wasm {
                let known = crate::versions::known_settlement_builds()
                    .iter()
                    .any(|h| *h == hex(&oc.code));
                problems.push(if known {
                    Problem::CodeDrift {
                        code: oc.code,
                        want: d.settlement_wasm,
                    }
                } else {
                    Problem::UnknownCode { code: oc.code }
                });
            }
            let mut fixed = |field, deployed: String, file: String| {
                if deployed != file {
                    problems.push(Problem::Immutable {
                        field,
                        deployed,
                        file,
                    });
                }
            };
            fixed("lane_id", short(&oc.lane_id), short(&d.lane_id));
            fixed("config_hash", short(&oc.config_hash), short(&d.config_hash));
            fixed(
                "genesis_state_hash",
                short(&oc.genesis_state_hash),
                short(&d.genesis_state_hash),
            );
            fixed(
                "engine_wasm_hash",
                short(&oc.engine_wasm_hash),
                short(&d.engine_wasm_hash),
            );
            fixed("admin", g_short(&oc.admin), g_short(&d.admin));
            fixed("token", c_short(&oc.token), c_short(&d.token));
            let (a, b) = (&oc.params, &d.params);
            fixed(
                "settlement_params.force_inclusion_window_secs",
                a.force_inclusion_window_secs.to_string(),
                b.force_inclusion_window_secs.to_string(),
            );
            fixed(
                "settlement_params.escape_timeout_secs",
                a.escape_timeout_secs.to_string(),
                b.escape_timeout_secs.to_string(),
            );
            fixed(
                "settlement_params.min_rotation_delay_secs",
                a.min_rotation_delay_secs.to_string(),
                b.min_rotation_delay_secs.to_string(),
            );
            fixed(
                "settlement_params.signer_retention_epochs",
                a.signer_retention_epochs.to_string(),
                b.signer_retention_epochs.to_string(),
            );
            fixed(
                "settlement_params.min_deposit",
                a.min_deposit.to_string(),
                b.min_deposit.to_string(),
            );
            if oc.frozen {
                problems.push(Problem::Frozen);
            }
            if oc.signers != d.signers {
                match oc.desired_set_epoch {
                    Some(epoch) => problems.push(Problem::SignersReused { epoch }),
                    None => {
                        rotate = Some(Step::RotateSigners {
                            from_epoch: oc.epoch,
                            to_epoch: oc.epoch + 1,
                        })
                    }
                }
            }
        }
    }

    // Nodes run what the contract has; when the lane file disagrees with the
    // contract, that is the problem to report, not each node.
    let immutable = problems
        .iter()
        .any(|p| matches!(p, Problem::Immutable { .. }));

    // --- Host --------------------------------------------------------------------
    if !host.missing.is_empty() {
        problems.push(Problem::HostNotReady {
            missing: host.missing.clone(),
        });
    }
    // A release that would be installed must run there (a macOS build never
    // reaches a Linux host).
    if let (Some(want), Some(have)) = (&d.host.platform, &host.platform) {
        if want != have && host.release.as_deref() != Some(d.host.release.as_str()) {
            problems.push(Problem::WrongPlatform {
                release: want.clone(),
                host: have.clone(),
            });
        }
    }
    let mut all: Vec<String> = vec!["sequencer".into()];
    all.extend(d.validators.iter().cloned());
    all.push("relayer".into());
    let mut restart: BTreeSet<String> = BTreeSet::new();
    let wipe = fresh && host.has_data;
    if wipe {
        steps.push(Step::WipeHostData);
        restart.extend(all.iter().cloned());
    }
    if host.release.as_deref() != Some(d.host.release.as_str()) {
        steps.push(Step::InstallRelease {
            from: host.release.clone(),
            to: d.host.release.clone(),
        });
        restart.extend(all.iter().cloned());
    }
    for (path, hash) in &d.host.files {
        if host.files.get(path) != Some(hash) {
            steps.push(Step::WriteFile { path: path.clone() });
            restart.extend(nodes_of(path, &all));
        }
    }
    let start_or_restart = |node: &str, steps: &mut Vec<Step>, problems: &mut Vec<Problem>| {
        let state = host.nodes.get(node).cloned().unwrap_or_default();
        let target = target_epoch(d, chain);
        let report = state.report.as_ref();
        let stale = state
            .started_with
            .is_some_and(|f| f != fingerprint(&d.host.files, node))
            // A sequencer on another epoch than the chain will have.
            || report.and_then(|r| r.epoch).is_some_and(|e| e != target)
            // A binary from another release than the host has.
            || report
                .and_then(|r| r.release.as_deref())
                .is_some_and(|c| !c.starts_with(d.host.release.as_str()) && !d.host.release.starts_with(c));
        if !state.running {
            steps.push(Step::Start { node: node.into() });
        } else if restart.contains(node) || stale {
            steps.push(Step::Restart { node: node.into() });
        } else if let Some(r) = state.report.as_ref().filter(|_| !immutable) {
            let checks = [
                ("lane_id", r.lane_id == d.lane_id),
                ("config_hash", r.config_hash == d.config_hash),
                ("settlement", r.settlement == d.settlement),
                (
                    "engine_wasm_sha256",
                    r.engine_wasm_hash == d.engine_wasm_hash,
                ),
            ];
            for (field, ok) in checks {
                if !ok {
                    problems.push(Problem::NodeMismatch {
                        node: node.into(),
                        field,
                    });
                }
            }
        }
    };
    // Validators first, so a new one is up before the rotation names it; the
    // sequencer restarts after it with the new epoch (e2e step 4b).
    for v in &d.validators {
        start_or_restart(v, &mut steps, &mut problems);
    }
    if let Some(r) = rotate {
        steps.push(r);
    }
    start_or_restart("sequencer", &mut steps, &mut problems);
    start_or_restart("relayer", &mut steps, &mut problems);
    for (node, state) in &host.nodes {
        if state.running && !all.contains(node) {
            steps.push(Step::Stop { node: node.clone() });
        }
    }
    Plan {
        target_epoch: target_epoch(d, chain),
        steps,
        problems,
    }
}

impl Plan {
    /// The plan for scripts (`plan --json`): what it would change and what
    /// blocks it. Each step keeps its printed line.
    pub fn to_json(&self, d: &Desired) -> serde_json::Value {
        let steps: Vec<_> = self
            .steps
            .iter()
            .map(|s| {
                let line = step_line(s);
                let mut words = line.split_whitespace();
                let change = words.next().unwrap_or_default().to_string();
                let action = words.next().unwrap_or_default().to_string();
                serde_json::json!({ "change": change, "action": action, "line": line })
            })
            .collect();
        let problems: Vec<_> = self
            .problems
            .iter()
            .map(|p| problem_line(p).trim_start_matches("! ").to_string())
            .collect();
        serde_json::json!({
            "lane": d.lane_name,
            "env": d.env,
            "network": d.network.name(),
            "lane_id": hex(&d.lane_id),
            "config_hash": hex(&d.config_hash),
            "settlement": strkey(&d.settlement),
            "settlement_pinned": d.settlement_pinned,
            "token": strkey(&d.token),
            "target_epoch": self.target_epoch,
            "steps": steps,
            "problems": problems,
        })
    }

    /// The plan as the user reads it.
    pub fn render(&self, d: &Desired) -> String {
        let mut o = String::new();
        let _ = writeln!(
            o,
            "Lane {} · env {} · network {}",
            d.lane_name,
            d.env,
            d.network.name()
        );
        let _ = writeln!(
            o,
            "  lane_id {}  config_hash {}  engine {}",
            short(&d.lane_id),
            short(&d.config_hash),
            short(&d.engine_wasm_hash)
        );
        let _ = writeln!(
            o,
            "  settlement {} ({}), admin {}, token {}",
            strkey(&d.settlement),
            if d.settlement_pinned {
                "named in the lane file"
            } else {
                "derived from the admin and the lane"
            },
            g_short(&d.admin),
            c_short(&d.token)
        );
        let _ = writeln!(
            o,
            "  signers: {} of {} at epoch {}",
            d.signers.threshold,
            d.signers.signers.len(),
            self.target_epoch
        );
        if !d.vars.is_empty() {
            let _ = writeln!(o, "  vars: {}", d.vars);
        }
        if self.steps.is_empty() && self.problems.is_empty() {
            o.push_str("\nNo changes.\n");
            return o;
        }
        if !self.steps.is_empty() {
            o.push('\n');
            for s in &self.steps {
                o.push_str(&step_line(s));
                o.push('\n');
            }
        }
        if !self.problems.is_empty() {
            o.push('\n');
            for p in &self.problems {
                o.push_str(&problem_line(p));
                o.push('\n');
            }
        }
        let _ = writeln!(
            o,
            "\n{} step(s), {} problem(s).{}",
            self.steps.len(),
            self.problems.len(),
            if self.problems.is_empty() {
                ""
            } else {
                " Apply refuses until they are fixed."
            }
        );
        o
    }
}

/// One step, as `plan` prints it.
pub fn step_line(s: &Step) -> String {
    match s {
        Step::Fund { who, key } => format!("+ fund      {who} {} (friendbot)", g_short(key)),
        Step::DeployToken {
            contract,
            code,
            issuer,
        } => format!(
            "+ create    token contract {} (the Stellar Asset Contract of {code}:{})",
            c_short(contract),
            g_short(issuer)
        ),
        Step::UploadWasm { hash } => format!("+ upload    settlement Wasm {}", short(hash)),
        Step::DeploySettlement { contract } => {
            format!("+ deploy    settlement {}", strkey(contract))
        }
        Step::WipeHostData => {
            "- wipe      the host's lane data (it belongs to a contract that is gone)".into()
        }
        Step::InstallRelease { from, to } => match from {
            Some(f) => format!("~ release   {f} → {to}"),
            None => format!("+ release   {to}"),
        },
        Step::WriteFile { path } => format!("~ write     {path}"),
        Step::Start { node } => format!("+ start     {node}"),
        Step::Restart { node } => format!("~ restart   {node}"),
        Step::Stop { node } => format!("- stop      {node}"),
        Step::RotateSigners {
            from_epoch,
            to_epoch,
        } => format!(
            "~ rotate    signers, epoch {from_epoch} → {to_epoch} (admin_rotate_signers: a testnet-only admin power)"
        ),
    }
}

/// One problem, as `plan` prints it.
pub fn problem_line(p: &Problem) -> String {
    match p {
        Problem::Immutable {
            field,
            deployed,
            file,
        } => format!(
            "! {field}: the contract has {deployed}, the lane file gives {file}. The constructor fixed it: destroy this lane, or give the lane a new name"
        ),
        Problem::CodeDrift { code, want } => format!(
            "! the settlement contract runs {}, not {}: it was upgraded outside this tool, or settlement_wasm is wrong",
            short(code),
            short(want)
        ),
        Problem::UnknownCode { code } => format!(
            "! the settlement contract runs {}, a build no one recorded",
            short(code)
        ),
        Problem::Frozen => {
            "! the lane is frozen: nothing can be applied (destroy is done)".into()
        }
        Problem::SettlementMissing { contract } => format!(
            "! the network has no contract at {} (a testnet reset?)",
            strkey(contract)
        ),
        Problem::TokenMissing { contract } => {
            format!("! the network has no token contract at {}", strkey(contract))
        }
        Problem::SignersReused { epoch } => format!(
            "! this signer set was installed at epoch {epoch}; a set can be installed only once, so change a key"
        ),
        Problem::HostNotReady { missing } => {
            format!("! the host lacks: {}", missing.join(", "))
        }
        Problem::WrongPlatform { release, host } => format!(
            "! the release's node binary is built for {release}, and the host is {host}: install the CI release (--release-dir) or build on the host's platform"
        ),
        Problem::NodeMismatch { node, field } => format!(
            "! {node} reports another {field} than its files give"
        ),
    }
}
