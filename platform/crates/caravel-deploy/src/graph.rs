//! The plan as a graph of resources (M0.6, C-15, DEC-087).
//!
//! A deployment is a set of resources, each with an address
//! (`account.admin`, `contract.settlement`, `file.sequencer.toml`,
//! `node.validator-2`, …) and a kind. Each one compares what the lane file
//! wants with what Stellar and the host have, and gives the steps that close
//! the gap and the problems that block it. Edges say what comes first:
//!
//! - `Order`: the target reads the source's outcome, or must run after it
//!   (the settlement contract before the host's data, validators before a
//!   signer rotation, the rotation before the sequencer);
//! - `Restart`: a changed source restarts the target node (a file it reads,
//!   the release, a wiped store).
//!
//! Steps come out in a topological order (Kahn's algorithm), ties broken by
//! the kind's rank and then the order resources were declared in, which is
//! the order `plan` always printed. Problems come out in declaration order.
//! There is still no state file: every resource is read back from the lane
//! file, Stellar and the host.

use std::collections::{BTreeMap, BTreeSet};

use crate::plan::{
    c_short, fingerprint, g_short, hex, short, target_epoch, Chain, Desired, Host, Options, Plan,
    Problem, Step,
};

/// What a resource is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A Stellar account the deployment pays from (friendbot funds it).
    Account,
    /// The settlement token's contract.
    Token,
    /// The settlement contract's Wasm, uploaded once.
    Wasm,
    /// The settlement contract.
    Contract,
    /// The signer set the settlement contract checks checkpoints against.
    Signers,
    /// What the host must have before anything runs there.
    Host,
    /// The lane's stores on the host.
    HostData,
    /// The release the host runs.
    Release,
    /// A generated file on the host.
    File,
    /// A node: the sequencer, a validator, the relayer.
    Node,
    /// A node the lane file no longer names.
    Orphan,
}

impl Kind {
    /// The tie-break between resources that are ready at once: Stellar
    /// before the host, files before the nodes that read them. The signer
    /// set ranks with the nodes, as its rotation sits between them.
    fn rank(self) -> u8 {
        match self {
            Kind::Account => 0,
            Kind::Token => 1,
            Kind::Wasm => 2,
            Kind::Contract => 3,
            Kind::Host => 4,
            Kind::HostData => 5,
            Kind::Release => 6,
            Kind::File => 7,
            Kind::Signers | Kind::Node => 8,
            Kind::Orphan => 9,
        }
    }
}

/// How an edge ties two resources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    Order,
    Restart,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub kind: EdgeKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    /// e.g. `account.admin`, `file.sequencer.toml`, `node.validator-2`.
    pub addr: String,
    pub kind: Kind,
}

/// A deployment's resources and the edges between them.
#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub resources: Vec<Resource>,
    pub edges: Vec<Edge>,
    index: BTreeMap<String, usize>,
}

impl Graph {
    fn add(&mut self, addr: impl Into<String>, kind: Kind) -> usize {
        let addr = addr.into();
        let i = self.resources.len();
        self.index.insert(addr.clone(), i);
        self.resources.push(Resource { addr, kind });
        i
    }

    fn edge(&mut self, from: usize, to: usize, kind: EdgeKind) {
        if from != to && !self.edges.contains(&Edge { from, to, kind }) {
            self.edges.push(Edge { from, to, kind });
        }
    }

    /// The resource at `addr`.
    pub fn get(&self, addr: &str) -> Option<usize> {
        self.index.get(addr).copied()
    }

    /// `of` runs after `on` (an `Order` edge), whatever their kinds' ranks
    /// say. A cycle shows up in [`Graph::order`].
    pub fn depends_on(&mut self, of: &str, on: &str) -> Result<(), String> {
        let of_i = self.resolve_one(of)?;
        let on_i = self.resolve_one(on)?;
        self.edge(on_i, of_i, EdgeKind::Order);
        Ok(())
    }

    fn resolve_one(&self, addr: &str) -> Result<usize, String> {
        self.get(addr).ok_or_else(|| self.unknown(addr))
    }

    fn unknown(&self, addr: &str) -> String {
        let hint =
            caravel_lanefile::did_you_mean(addr, self.resources.iter().map(|r| r.addr.as_str()))
                .map(|m| format!(": did you mean {m}?"))
                .unwrap_or_else(|| ": `caravel graph` lists them".into());
        format!("no resource {addr:?} in this deployment{hint}")
    }

    /// The resources `pattern` names: an address, or a pattern where `*`
    /// stands for any characters (`node.*`, `node.validator-*`,
    /// `*.settlement`).
    pub fn resolve(&self, pattern: &str) -> Result<Vec<usize>, String> {
        let found: Vec<usize> = if pattern.contains('*') {
            self.resources
                .iter()
                .enumerate()
                .filter(|(_, r)| glob(pattern, &r.addr))
                .map(|(i, _)| i)
                .collect()
        } else {
            self.get(pattern).into_iter().collect()
        };
        if found.is_empty() {
            return Err(self.unknown(pattern));
        }
        Ok(found)
    }

    /// `targets` and everything they depend on, through edges of both kinds.
    pub fn upstream(&self, targets: &[usize]) -> BTreeSet<usize> {
        let mut seen: BTreeSet<usize> = targets.iter().copied().collect();
        let mut todo: Vec<usize> = targets.to_vec();
        while let Some(i) = todo.pop() {
            for e in self.edges.iter().filter(|e| e.to == i) {
                if seen.insert(e.from) {
                    todo.push(e.from);
                }
            }
        }
        seen
    }

    /// Sources of the edges of `kind` into `to`.
    fn sources(&self, to: usize, kind: EdgeKind) -> impl Iterator<Item = usize> + '_ {
        self.edges
            .iter()
            .filter(move |e| e.to == to && e.kind == kind)
            .map(|e| e.from)
    }

    /// Every resource once, each after all its sources (Kahn's algorithm;
    /// ties go to the lowest rank, then the first declared). `Err` names the
    /// resources on a cycle.
    pub fn order(&self) -> Result<Vec<usize>, Vec<String>> {
        let n = self.resources.len();
        let mut incoming = vec![0usize; n];
        for e in &self.edges {
            incoming[e.to] += 1;
        }
        let key = |i: usize| (self.resources[i].kind.rank(), i);
        let mut ready: BTreeSet<(u8, usize)> =
            (0..n).filter(|i| incoming[*i] == 0).map(key).collect();
        let mut out = Vec::with_capacity(n);
        while let Some(first) = ready.pop_first() {
            let i = first.1;
            out.push(i);
            for e in self.edges.iter().filter(|e| e.from == i) {
                incoming[e.to] -= 1;
                if incoming[e.to] == 0 {
                    ready.insert(key(e.to));
                }
            }
        }
        if out.len() < n {
            let stuck = (0..n)
                .filter(|i| incoming[*i] > 0)
                .map(|i| self.resources[i].addr.clone())
                .collect();
            return Err(stuck);
        }
        Ok(out)
    }

    /// The graph in Graphviz's DOT, for `caravel graph`.
    pub fn dot(&self) -> String {
        let mut o = String::from("digraph lane {\n  rankdir=LR;\n");
        for r in &self.resources {
            o.push_str(&format!("  \"{}\";\n", r.addr));
        }
        for e in &self.edges {
            let style = match e.kind {
                EdgeKind::Order => "",
                EdgeKind::Restart => " [style=dashed, label=\"restart\"]",
            };
            o.push_str(&format!(
                "  \"{}\" -> \"{}\"{style};\n",
                self.resources[e.from].addr, self.resources[e.to].addr
            ));
        }
        o.push_str("}\n");
        o
    }
}

/// Whether `text` matches `pattern`, where `*` is any run of characters.
fn glob(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !text.starts_with(first) || text.len() < first.len() + last.len() || !text.ends_with(last) {
        return false;
    }
    let mut rest = &text[first.len()..text.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

/// The nodes a changed file restarts.
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

/// The deployment's nodes, in the order they start: the sequencer, the
/// validators in file order, the relayer.
fn nodes(d: &Desired) -> Vec<String> {
    let mut all: Vec<String> = vec!["sequencer".into()];
    all.extend(d.validators.iter().cloned());
    all.push("relayer".into());
    all
}

/// The graph of `d`, with the nodes `host` runs that `d` doesn't name.
/// `Err` when a `depends_on` names nothing.
pub fn build(d: &Desired, host: &Host) -> Result<Graph, String> {
    use EdgeKind::{Order, Restart};
    let mut g = Graph::default();
    let admin = g.add("account.admin", Kind::Account);
    let relayer_account = g.add("account.relayer", Kind::Account);
    let account_ids: Vec<usize> = d
        .accounts
        .iter()
        .map(|a| g.add(format!("account.{}", a.name), Kind::Account))
        .collect();
    let token = g.add("token.settlement", Kind::Token);
    let wasm = g.add("wasm.settlement", Kind::Wasm);
    let contract = g.add("contract.settlement", Kind::Contract);
    let signers = g.add("signers.settlement", Kind::Signers);
    let host_r = g.add("host", Kind::Host);
    let data = g.add("host.data", Kind::HostData);
    let release = g.add("release", Kind::Release);
    let files: Vec<(String, usize)> = d
        .host
        .files
        .keys()
        .map(|p| (p.clone(), g.add(format!("file.{p}"), Kind::File)))
        .collect();
    let all = nodes(d);
    // Declared in the order they start: validators, then the sequencer and
    // the relayer.
    let mut declared: Vec<String> = d.validators.clone();
    declared.push("sequencer".into());
    declared.push("relayer".into());
    let node: BTreeMap<String, usize> = declared
        .iter()
        .map(|n| (n.clone(), g.add(format!("node.{n}"), Kind::Node)))
        .collect();
    for (name, state) in &host.nodes {
        if state.running && !all.contains(name) {
            g.add(format!("node.{name}"), Kind::Orphan);
        }
    }

    // Stellar: the contract needs its admin, its token and its Wasm.
    g.edge(admin, contract, Order);
    g.edge(token, contract, Order);
    g.edge(wasm, contract, Order);
    g.edge(contract, signers, Order);
    // The host: its data goes with the contract; nothing installs on a host
    // that isn't ready.
    g.edge(contract, data, Order);
    g.edge(host_r, data, Order);
    g.edge(data, release, Order);
    for (_, f) in &files {
        g.edge(release, *f, Order);
    }
    for &n in node.values() {
        // A node checks what it reports against the contract.
        g.edge(contract, n, Order);
        g.edge(data, n, Restart);
        g.edge(release, n, Restart);
    }
    for (path, f) in &files {
        for target in nodes_of(path, &all) {
            if let Some(&n) = node.get(&target) {
                g.edge(*f, n, Restart);
            }
        }
    }
    // A new validator is up before the rotation names it, and the sequencer
    // restarts after it with the new epoch.
    for v in &d.validators {
        g.edge(node[v], signers, Order);
    }
    g.edge(signers, node["sequencer"], Order);
    g.edge(relayer_account, node["relayer"], Order);
    // A declared account that trusts the settlement token, or is topped up
    // with it, follows the token (and the admin, who mints).
    for (a, &i) in d.accounts.iter().zip(&account_ids) {
        let settlement = d.settlement_asset.as_ref();
        if a.balance.is_some() || a.trustlines.iter().any(|t| Some(t) == settlement) {
            g.edge(token, i, Order);
        }
        if a.balance.is_some() {
            g.edge(admin, i, Order);
        }
    }
    for a in &d.accounts {
        for on in &a.depends_on {
            g.depends_on(&format!("account.{}", a.name), on)
                .map_err(|e| format!("accounts.{}.depends_on: {e}", a.name))?;
        }
    }
    Ok(g)
}

/// What one resource's comparison gave.
#[derive(Clone, Debug, Default)]
struct Outcome {
    steps: Vec<Step>,
    problems: Vec<Problem>,
}

impl Outcome {
    fn changes(&self) -> bool {
        !self.steps.is_empty()
    }
}

/// A declared account against the chain: funded, trusting its assets, and
/// holding at least its balance of the settlement token (minted by the
/// admin, who must issue it). An issuer needs no trustline to its own asset.
fn declared_outcome(d: &Desired, chain: &Chain, a: &crate::plan::DeclaredAccount) -> Outcome {
    let mut o = Outcome::default();
    if !chain.accounts.contains(&a.key) {
        if a.fund {
            o.steps.push(Step::Fund {
                who: a.name.clone(),
                key: a.key,
            });
        } else {
            o.problems.push(Problem::AccountMissing {
                who: a.name.clone(),
            });
        }
    }
    for (code, issuer) in &a.trustlines {
        if *issuer != a.key
            && !chain
                .trustlines
                .contains_key(&(a.key, code.clone(), *issuer))
        {
            o.steps.push(Step::Trust {
                who: a.name.clone(),
                code: code.clone(),
                issuer: *issuer,
            });
        }
    }
    if let (Some(want), Some((code, issuer))) = (a.balance, &d.settlement_asset) {
        if *issuer != a.key {
            let have = chain
                .trustlines
                .get(&(a.key, code.clone(), *issuer))
                .copied()
                .unwrap_or(0);
            if have < want {
                if *issuer == d.admin {
                    o.steps.push(Step::Mint {
                        who: a.name.clone(),
                        key: a.key,
                        code: code.clone(),
                        amount: want - have,
                    });
                } else {
                    o.problems.push(Problem::CannotMint {
                        who: a.name.clone(),
                        code: code.clone(),
                        have,
                        want,
                    });
                }
            }
        }
    }
    o
}

/// The settlement contract against the lane file: a new one to deploy, or
/// what the constructor fixed and the code it runs.
fn contract_outcome(d: &Desired, chain: &Chain) -> Outcome {
    let mut o = Outcome::default();
    match &chain.settlement {
        None if d.settlement_pinned => o.problems.push(Problem::SettlementMissing {
            contract: d.settlement,
        }),
        None => o.steps.push(Step::DeploySettlement {
            contract: d.settlement,
        }),
        Some(oc) => {
            if oc.code != d.settlement_wasm {
                let known = crate::versions::known_settlement_builds()
                    .iter()
                    .any(|h| *h == hex(&oc.code));
                o.problems.push(if known {
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
                    o.problems.push(Problem::Immutable {
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
                o.problems.push(Problem::Frozen);
            }
        }
    }
    o
}

/// Why a resource can't be replaced, if it can't.
fn unreplaceable(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Node | Kind::File | Kind::Release => None,
        Kind::Contract => Some(
            "a settlement contract can't be replaced in place: its address derives from the admin and the lane's name, so a new one is a new lane (destroy this one, or give the lane a new name)",
        ),
        Kind::Account => Some("an account isn't replaced: its key is the lane file's identity"),
        Kind::Token => Some("the settlement token is fixed when the contract is deployed"),
        Kind::Wasm => Some("an uploaded Wasm is content-addressed: there is nothing to replace"),
        Kind::Signers => Some("the signer set changes through the lane file's validators (a rotation)"),
        Kind::Host | Kind::HostData => Some(
            "the host's data is replaced with the contract (`caravel destroy --wipe`, then apply)",
        ),
        Kind::Orphan => Some("a node the lane file no longer names is stopped, not replaced"),
    }
}

/// The plan for `d` against what the chain and the host have: every
/// resource compared, the steps in the graph's order. `opts` narrows it to
/// targets and what they depend on, or forces replacements.
pub fn diff(d: &Desired, chain: &Chain, host: &Host, opts: &Options) -> Result<Plan, String> {
    let g = build(d, host)?;
    let order = g.order().map_err(|cycle| {
        format!(
            "the deployment's resources depend on each other in a cycle: {}",
            cycle.join(", ")
        )
    })?;
    let mut replace = BTreeSet::new();
    for pattern in &opts.replace {
        for i in g.resolve(pattern)? {
            if let Some(why) = unreplaceable(g.resources[i].kind) {
                return Err(format!("{} can't be replaced: {why}", g.resources[i].addr));
            }
            replace.insert(i);
        }
    }
    let scope: Option<BTreeSet<usize>> = if opts.target.is_empty() {
        None
    } else {
        let mut targets = Vec::new();
        for pattern in &opts.target {
            targets.extend(g.resolve(pattern)?);
        }
        Some(g.upstream(&targets))
    };
    let target = target_epoch(d, chain);
    let mut out: Vec<Outcome> = vec![Outcome::default(); g.resources.len()];
    for &i in &order {
        let r = &g.resources[i];
        let mut o = Outcome::default();
        match (r.kind, r.addr.as_str()) {
            (Kind::Account, "account.admin" | "account.relayer") => {
                let (who, key) = match r.addr.as_str() {
                    "account.admin" => ("admin", d.admin),
                    _ => ("relayer", d.relayer),
                };
                if !chain.accounts.contains(&key) {
                    o.steps.push(Step::Fund {
                        who: who.into(),
                        key,
                    });
                }
            }
            (Kind::Account, addr) => {
                let name = addr.trim_start_matches("account.");
                if let Some(a) = d.accounts.iter().find(|a| a.name == name) {
                    o = declared_outcome(d, chain, a);
                }
            }
            (Kind::Token, _) => {
                if !chain.token_exists {
                    match &d.token_asset {
                        Some((code, issuer)) => o.steps.push(Step::DeployToken {
                            contract: d.token,
                            code: code.clone(),
                            issuer: *issuer,
                        }),
                        None => o.problems.push(Problem::TokenMissing { contract: d.token }),
                    }
                }
            }
            (Kind::Wasm, _) => {
                // Uploaded only for a contract to deploy.
                if chain.settlement.is_none()
                    && !d.settlement_pinned
                    && !chain.settlement_wasm_uploaded
                {
                    o.steps.push(Step::UploadWasm {
                        hash: d.settlement_wasm,
                    });
                }
            }
            (Kind::Contract, _) => o = contract_outcome(d, chain),
            (Kind::Signers, _) => {
                if let Some(oc) = &chain.settlement {
                    if oc.signers != d.signers {
                        match oc.desired_set_epoch {
                            Some(epoch) => o.problems.push(Problem::SignersReused { epoch }),
                            None => o.steps.push(Step::RotateSigners {
                                from_epoch: oc.epoch,
                                to_epoch: oc.epoch + 1,
                            }),
                        }
                    }
                }
            }
            (Kind::Host, _) => {
                if !host.missing.is_empty() {
                    o.problems.push(Problem::HostNotReady {
                        missing: host.missing.clone(),
                    });
                }
                // A release that would be installed must run there (a macOS
                // build never reaches a Linux host).
                if let (Some(want), Some(have)) = (&d.host.platform, &host.platform) {
                    if want != have && host.release.as_deref() != Some(d.host.release.as_str()) {
                        o.problems.push(Problem::WrongPlatform {
                            release: want.clone(),
                            host: have.clone(),
                        });
                    }
                }
            }
            (Kind::HostData, _) => {
                // The stores belong to a contract that is gone.
                let fresh = g.sources(i, EdgeKind::Order).any(|s| {
                    out[s]
                        .steps
                        .iter()
                        .any(|st| matches!(st, Step::DeploySettlement { .. }))
                });
                if fresh && host.has_data {
                    o.steps.push(Step::WipeHostData);
                }
            }
            (Kind::Release, _) => {
                if host.release.as_deref() != Some(d.host.release.as_str()) || replace.contains(&i)
                {
                    o.steps.push(Step::InstallRelease {
                        from: host.release.clone(),
                        to: d.host.release.clone(),
                    });
                }
            }
            (Kind::File, addr) => {
                let path = addr.trim_start_matches("file.");
                if host.files.get(path) != d.host.files.get(path) || replace.contains(&i) {
                    o.steps.push(Step::WriteFile { path: path.into() });
                }
            }
            (Kind::Node, addr) => {
                let name = addr.trim_start_matches("node.");
                let state = host.nodes.get(name).cloned().unwrap_or_default();
                let report = state.report.as_ref();
                let restart = g.sources(i, EdgeKind::Restart).any(|s| out[s].changes());
                // Nodes run what the contract has; when the lane file
                // disagrees with the contract, that is the problem to report,
                // not each node.
                let immutable = g.get("contract.settlement").is_some_and(|c| {
                    out[c]
                        .problems
                        .iter()
                        .any(|p| matches!(p, Problem::Immutable { .. }))
                });
                let stale = state
                    .started_with
                    .is_some_and(|f| f != fingerprint(&d.host.files, name))
                    // A sequencer on another epoch than the chain will have.
                    || report.and_then(|r| r.epoch).is_some_and(|e| e != target)
                    // A binary from another release than the host has.
                    || report.and_then(|r| r.release.as_deref()).is_some_and(|c| {
                        !c.starts_with(d.host.release.as_str()) && !d.host.release.starts_with(c)
                    });
                if !state.running {
                    o.steps.push(Step::Start { node: name.into() });
                } else if restart || stale || replace.contains(&i) {
                    o.steps.push(Step::Restart { node: name.into() });
                } else if let Some(r) = report.filter(|_| !immutable) {
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
                            o.problems.push(Problem::NodeMismatch {
                                node: name.into(),
                                field,
                            });
                        }
                    }
                }
            }
            (Kind::Orphan, addr) => o.steps.push(Step::Stop {
                node: addr.trim_start_matches("node.").into(),
            }),
        }
        out[i] = o;
    }
    let planned = |i: &usize| scope.as_ref().is_none_or(|s| s.contains(i));
    let steps = order
        .iter()
        .filter(|i| planned(i))
        .flat_map(|&i| out[i].steps.clone())
        .collect();
    let problems = (0..out.len())
        .filter(planned)
        .flat_map(|i| out[i].problems.clone())
        .collect();
    Ok(Plan {
        target_epoch: target,
        steps,
        problems,
        targets: opts.target.clone(),
        replaced: opts.replace.clone(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use proptest::prelude::*;

    use super::*;
    use crate::manifest::{Network, Provider};
    use crate::plan::{
        legacy_diff, DesiredHost, Key, NodeReport, NodeState, OnChain, Params, SignerSet,
    };

    fn key(n: u8) -> Key {
        [n; 32]
    }

    /// Choices drawn from random bytes, so one byte string is one scenario.
    struct Draw<'a>(&'a [u8], usize);

    impl Draw<'_> {
        fn byte(&mut self) -> u8 {
            let b = self.0[self.1 % self.0.len()];
            self.1 += 1;
            b
        }
        fn yes(&mut self) -> bool {
            self.byte().is_multiple_of(2)
        }
        fn rarely(&mut self) -> bool {
            self.byte().is_multiple_of(5)
        }
        fn pick<T: Clone>(&mut self, xs: &[T]) -> T {
            xs[self.byte() as usize % xs.len()].clone()
        }
    }

    fn params() -> Params {
        Params {
            force_inclusion_window_secs: 20,
            escape_timeout_secs: 30,
            min_rotation_delay_secs: 3600,
            signer_retention_epochs: 2,
            min_deposit: 10_000_000,
        }
    }

    fn signer_set(keys: &[u8]) -> SignerSet {
        let mut s: Vec<_> = keys.iter().map(|k| (key(*k), 1)).collect();
        s.sort();
        SignerSet {
            signers: s,
            threshold: 2,
        }
    }

    /// A deployment, Stellar and a host, drawn from `bytes`.
    fn scenario(bytes: &[u8]) -> (Desired, Chain, Host) {
        let mut r = Draw(bytes, 0);
        let names: Vec<&str> = r.pick(&[
            vec!["1", "2", "3"],
            vec!["1", "2", "4"],
            vec!["1"],
            vec!["2", "3", "4", "5"],
        ]);
        let validators: Vec<String> = names.iter().map(|n| format!("validator-{n}")).collect();
        let mut files = BTreeMap::new();
        files.insert("lane.toml".to_string(), key(0x11));
        files.insert("sequencer.toml".to_string(), key(0x21));
        files.insert("relayer.json".to_string(), key(0x31));
        if r.yes() {
            files.insert("systemd/caravel-sequencer.service".to_string(), key(0x51));
            files.insert("systemd/caravel-validator@.service".to_string(), key(0x52));
            files.insert("Caddyfile".to_string(), key(0x53));
        }
        for n in &names {
            files.insert(format!("validator-{n}.toml"), key(0x40 + n.as_bytes()[0]));
        }
        let d = Desired {
            lane_name: "demo".into(),
            env: "local".into(),
            network: Network::Local,
            lane_id: key(1),
            config_hash: key(2),
            genesis_state_hash: key(3),
            engine_wasm_hash: key(4),
            admin: key(0xA0),
            relayer: key(0xA1),
            token: key(0xB0),
            token_asset: r.yes().then(|| ("USDC".to_string(), key(0xA0))),
            settlement_asset: Some(("USDC".to_string(), key(0xA0))),
            settlement: key(0xC0),
            settlement_pinned: r.rarely(),
            settlement_wasm: key(0xD0),
            params: params(),
            signers: signer_set(
                &names
                    .iter()
                    .map(|n| 0x60 + n.as_bytes()[0])
                    .collect::<Vec<_>>(),
            ),
            validators: validators.clone(),
            host: DesiredHost {
                provider: Provider::Local,
                release: "0a1b2c3d".into(),
                platform: r.pick(&[None, Some("Linux x86_64".to_string())]),
                files,
            },
            vars: String::new(),
            accounts: vec![],
        };

        let mut accounts = BTreeSet::new();
        if r.yes() {
            accounts.insert(d.admin);
        }
        if r.yes() {
            accounts.insert(d.relayer);
        }
        let settlement = (!r.rarely()).then(|| {
            let mut p = params();
            if r.rarely() {
                p.escape_timeout_secs += 1;
            }
            if r.rarely() {
                p.min_deposit += 1;
            }
            let pick = |r: &mut Draw, want: Key, other: Key| if r.rarely() { other } else { want };
            let same_set = r.yes();
            OnChain {
                code: r.pick(&[d.settlement_wasm, d.settlement_wasm, key(0xD1)]),
                admin: pick(&mut r, d.admin, key(0xAF)),
                token: pick(&mut r, d.token, key(0xBF)),
                lane_id: pick(&mut r, d.lane_id, key(0x91)),
                engine_wasm_hash: pick(&mut r, d.engine_wasm_hash, key(0x94)),
                genesis_state_hash: pick(&mut r, d.genesis_state_hash, key(0x93)),
                config_hash: pick(&mut r, d.config_hash, key(0x92)),
                params: p,
                epoch: 1 + (r.byte() % 3) as u64,
                signers: if same_set {
                    d.signers.clone()
                } else {
                    signer_set(&[0x61, 0x62, 0x63])
                },
                desired_set_epoch: r.pick(&[None, Some(1), Some(2)]),
                frozen: r.rarely(),
            }
        });
        let chain = Chain {
            accounts,
            trustlines: BTreeMap::new(),
            token_exists: r.yes(),
            settlement_wasm_uploaded: r.yes(),
            settlement,
        };

        let mut host_files = BTreeMap::new();
        for (p, h) in &d.host.files {
            match r.byte() % 4 {
                0 => {}
                1 => {
                    host_files.insert(p.clone(), key(0xEE));
                }
                _ => {
                    host_files.insert(p.clone(), *h);
                }
            }
        }
        if r.rarely() {
            host_files.insert("validator-9.toml".into(), key(0x49));
        }
        let mut nodes = BTreeMap::new();
        for n in [
            "sequencer",
            "relayer",
            "validator-1",
            "validator-2",
            "validator-3",
            "validator-4",
            "validator-5",
            "validator-9",
        ] {
            if r.byte() % 3 == 0 {
                continue;
            }
            let report = (!r.rarely()).then(|| NodeReport {
                lane_id: if r.rarely() { key(0x91) } else { d.lane_id },
                config_hash: if r.rarely() { key(0x92) } else { d.config_hash },
                settlement: if r.rarely() { key(0xC1) } else { d.settlement },
                engine_wasm_hash: d.engine_wasm_hash,
                key: None,
                epoch: r.pick(&[None, Some(1), Some(2)]),
                release: r.pick(&[None, Some("0a1b2c3d".to_string()), Some("ffff".to_string())]),
            });
            let started_with = match r.byte() % 3 {
                0 => None,
                1 => Some(fingerprint(&d.host.files, n)),
                _ => Some(key(0x77)),
            };
            nodes.insert(
                n.to_string(),
                NodeState {
                    running: !r.rarely(),
                    report,
                    started_with,
                },
            );
        }
        let host = Host {
            missing: if r.rarely() {
                vec!["caddy".into()]
            } else {
                vec![]
            },
            platform: r.pick(&[
                None,
                Some("Linux x86_64".to_string()),
                Some("Darwin arm64".to_string()),
            ]),
            release: r.pick(&[
                None,
                Some("0a1b2c3d".to_string()),
                Some("99887766".to_string()),
            ]),
            files: host_files,
            nodes,
            has_data: r.yes(),
        };
        (d, chain, host)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2000))]

        /// The graph plans exactly what the fixed-order diff did: the same
        /// steps in the same order, the same problems in the same order.
        #[test]
        fn the_graph_plans_what_the_fixed_order_did(bytes in prop::collection::vec(any::<u8>(), 96)) {
            let (d, chain, host) = scenario(&bytes);
            prop_assert_eq!(diff(&d, &chain, &host, &Options::default()).unwrap(), legacy_diff(&d, &chain, &host));
        }
    }

    #[test]
    fn addresses_and_order() {
        let (d, chain, mut host) = scenario(&[0; 96]);
        host.nodes.clear();
        let g = build(&d, &host).unwrap();
        let order: Vec<&str> = g
            .order()
            .unwrap()
            .iter()
            .map(|&i| g.resources[i].addr.as_str())
            .collect();
        let at = |a: &str| order.iter().position(|x| *x == a).unwrap();
        assert!(at("account.admin") < at("contract.settlement"));
        assert!(at("contract.settlement") < at("host.data"));
        assert!(at("release") < at("file.lane.toml"));
        assert!(at("node.validator-1") < at("signers.settlement"));
        assert!(at("signers.settlement") < at("node.sequencer"));
        assert!(at("node.sequencer") < at("node.relayer"));
        let _ = chain;
        // lane.toml restarts every node; a validator's file only its node.
        let restarts = |file: &str| -> Vec<&str> {
            let f = g.get(file).unwrap();
            g.edges
                .iter()
                .filter(|e| e.from == f && e.kind == EdgeKind::Restart)
                .map(|e| g.resources[e.to].addr.as_str())
                .collect()
        };
        assert_eq!(restarts("file.lane.toml").len(), d.validators.len() + 2);
        assert_eq!(restarts("file.validator-1.toml"), ["node.validator-1"]);
        assert!(g
            .dot()
            .contains("\"file.sequencer.toml\" -> \"node.sequencer\" [style=dashed"));
    }

    #[test]
    fn globs() {
        assert!(glob("node.*", "node.validator-1"));
        assert!(glob("node.validator-*", "node.validator-12"));
        assert!(glob("*.settlement", "contract.settlement"));
        assert!(glob("file.*.toml", "file.validator-2.toml"));
        assert!(!glob("file.*.toml", "file.relayer.json"));
        assert!(!glob("node.*", "nodes.x"));
        assert!(glob("a*a", "aa") && !glob("a*a", "a"));
    }

    #[test]
    fn depends_on_reorders_and_a_cycle_is_named() {
        let (d, _, mut host) = scenario(&[0; 96]);
        host.nodes.clear();
        let mut g = build(&d, &host).unwrap();
        let at = |g: &Graph, a: &str| {
            let order = g.order().unwrap();
            order
                .iter()
                .position(|&i| g.resources[i].addr == a)
                .unwrap()
        };
        assert!(at(&g, "node.sequencer") < at(&g, "node.relayer"));
        g.depends_on("node.sequencer", "node.relayer").unwrap();
        assert!(at(&g, "node.relayer") < at(&g, "node.sequencer"));
        // The contract needs the admin; making the admin need a node closes a loop.
        g.depends_on("account.admin", "node.relayer").unwrap();
        let stuck = g.order().unwrap_err();
        assert!(stuck.contains(&"account.admin".to_string()), "{stuck:?}");
        assert!(g.depends_on("node.nope", "node.relayer").is_err());
    }

    #[test]
    fn a_cycle_names_its_resources() {
        let mut g = Graph::default();
        let a = g.add("a", Kind::Node);
        let b = g.add("b", Kind::Node);
        let c = g.add("c", Kind::File);
        g.edge(a, b, EdgeKind::Order);
        g.edge(b, a, EdgeKind::Order);
        g.edge(c, a, EdgeKind::Order);
        assert_eq!(g.order().unwrap_err(), ["a", "b"]);
    }
}
