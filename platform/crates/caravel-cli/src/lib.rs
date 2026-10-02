//! `caravel`, the Caravel CLI (M0.6, spec §20.5): one binary for any
//! template's lanes. It finds the lane file and the deployment
//! ([`project`]), asks the template's binary only what the template knows
//! (genesis, bodies, examples: `caravel_node::plugin`), and does the rest
//! itself through the deploy library (`caravel_deploy`).
//!
//! Every command takes `--json` (one JSON document on stdout; progress and
//! notes on stderr). Exit codes are in [`exit`].

pub mod project;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, bail, Context, Result};
use caravel_deploy::cli::ReleaseArgs;
use caravel_deploy::deploy::{self, addresses, api_url, validator_url, Keys};
use caravel_deploy::manifest::{envs, Manifest};
use caravel_deploy::ops::DestroyOptions;
use caravel_deploy::stellar::Cli as Stellar;
use caravel_deploy::template::{self, Plugin, Template};
use caravel_node::lane_toml::LaneFile;
use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};

use project::{EnvFrom, FileFrom};

/// How `caravel` exits.
pub mod exit {
    pub const OK: u8 = 0;
    pub const ERROR: u8 = 1;
    /// A command line clap refused.
    pub const USAGE: u8 = 2;
    /// `plan --exit-code`, `status --exit-code`: the deployment differs from
    /// the lane file.
    pub const CHANGES: u8 = 3;
}

#[derive(Parser, Debug)]
#[command(
    name = "caravel",
    version,
    about = "Caravel: infrastructure as code for Stellar appchains (lanes). Testnet only.",
    long_about = "Caravel: infrastructure as code for Stellar appchains (lanes). Testnet only.\n\nA lane file declares a lane and its deployments ([env.<name>]). `caravel plan` shows what would change on Stellar and on the host, `caravel apply` makes it so, and `caravel destroy` winds the lane down: drain, export every exit with its proof, freeze.\n\nCommands find the lane file (./lane.toml, the one lane*.toml here, or the nearest lane.toml above) and the deployment (CARAVEL_ENV, the one marked default = true, or the only one) unless -f and --env name them."
)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,
    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Args, Debug, Clone, Default)]
pub struct Global {
    /// The lane file, or a directory with a lane.toml (also CARAVEL_FILE).
    #[arg(short = 'f', long = "file", global = true, value_name = "LANE_FILE")]
    pub file: Option<PathBuf>,
    /// The deployment, [env.<name>] (also CARAVEL_ENV).
    #[arg(short = 'e', long, global = true, value_name = "NAME")]
    pub env: Option<String>,
    /// Print JSON on stdout, for scripts; progress and notes go to stderr.
    #[arg(long, global = true)]
    pub json: bool,
    /// A release to install (also CARAVEL_RELEASE_DIR); default: the one
    /// installed with caravel, else this checkout's builds.
    #[arg(long, global = true, value_name = "DIR")]
    pub release_dir: Option<PathBuf>,
    /// Take the contracts from here instead, e.g. the CI contracts-wasm
    /// artifact (also CARAVEL_WASM_DIR).
    #[arg(long, global = true, value_name = "DIR")]
    pub wasm_dir: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Show what `apply` would change on Stellar and on the host. Changes
    /// nothing.
    Plan {
        /// The lane file (the same as -f).
        lane: Option<PathBuf>,
        /// Also show each file it would write, as a diff against the host's.
        #[arg(long)]
        diff: bool,
        /// Exit 3 when there are changes, and 1 when something blocks them.
        #[arg(long)]
        exit_code: bool,
    },
    /// Make Stellar and the host match the deployment. Shows the plan and
    /// asks first, unless --yes. Running it again changes nothing.
    Apply {
        /// The lane file (the same as -f).
        lane: Option<PathBuf>,
        /// Apply without asking.
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// A deployment's health: height, the last accepted checkpoint, when a
    /// freeze would be possible, the relayer's XLM, TTL horizons, and whether
    /// it matches the lane file.
    Status {
        /// The lane file (the same as -f).
        lane: Option<PathBuf>,
        /// Exit 3 when the deployment differs from the lane file.
        #[arg(long)]
        exit_code: bool,
    },
    /// Wind a lane down for good: drain, stop the sequencer and relayer,
    /// export every exit with its proof to exit.json, then freeze once the
    /// contract allows it. A frozen lane can't be restarted.
    Destroy {
        /// The lane file (the same as -f).
        lane: Option<PathBuf>,
        /// Don't ask (the lane can't be restarted afterwards).
        #[arg(short = 'y', long)]
        yes: bool,
        /// Stop after the trigger and print when a freeze becomes possible;
        /// run destroy again then.
        #[arg(long)]
        no_wait: bool,
        /// Also stop the validators (by default they keep serving proofs).
        #[arg(long)]
        stop_validators: bool,
        /// After the freeze, claim every exit in exit.json for its owner.
        #[arg(long)]
        pay_out: bool,
        /// Remove the host's lane data after stopping every node.
        #[arg(long)]
        wipe: bool,
    },
    /// Check the lane file without Stellar or a host: each deployment's
    /// rules, the genesis, and the identities it names.
    Validate {
        /// The lane file (the same as -f).
        lane: Option<PathBuf>,
    },
    /// The lane file's deployments.
    Env {
        #[command(subcommand)]
        cmd: EnvCmd,
    },
    /// A deployment's addresses and URLs, from the lane file and its keys;
    /// one of them with NAME.
    Output {
        /// e.g. settlement, token, api, lane_id.
        name: Option<String>,
    },
    /// This CLI, the templates it finds and the Stellar CLI it needs.
    Version,
    /// Check that this machine can run the deployment: the Stellar CLI,
    /// Node.js, the template, the release and the identities.
    Doctor,
}

#[derive(Subcommand, Debug)]
pub enum EnvCmd {
    /// List them, marking the one commands pick when --env is not given.
    List,
}

/// Runs the CLI from the process's arguments.
pub fn main() -> ExitCode {
    let cli = Cli::parse();
    run(cli)
}

/// Runs a parsed command line; errors are printed (and given as JSON with
/// `--json`).
pub fn run(cli: Cli) -> ExitCode {
    let json = cli.global.json;
    match dispatch(cli) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("error: {e:#}");
            if json {
                println!("{}", json!({ "ok": false, "error": format!("{e:#}") }));
            }
            ExitCode::from(exit::ERROR)
        }
    }
}

/// A command's lane file and deployment.
pub struct Ctx {
    pub lane_path: PathBuf,
    pub lane: LaneFile,
    pub env: String,
    pub env_from: EnvFrom,
    pub file_from: FileFrom,
    pub state_root: PathBuf,
    pub release: ReleaseArgs,
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// The lane file, from `-f` or the positional (they must agree), else found.
fn lane_file(g: &Global, positional: Option<&Path>) -> Result<(PathBuf, FileFrom)> {
    let explicit = match (g.file.as_deref(), positional) {
        (Some(a), Some(b)) if a != b => {
            bail!("two lane files: -f {} and {}", a.display(), b.display())
        }
        (a, b) => a.or(b),
    };
    let cwd = std::env::current_dir()?;
    let var = env_var("CARAVEL_FILE").map(PathBuf::from);
    let home = env_var("HOME").map(PathBuf::from);
    project::find_lane_file(explicit, var.as_deref(), &cwd, home.as_deref())
}

fn release_args(g: &Global) -> ReleaseArgs {
    ReleaseArgs {
        release_dir: g
            .release_dir
            .clone()
            .or_else(|| env_var("CARAVEL_RELEASE_DIR").map(PathBuf::from)),
        wasm_dir: g
            .wasm_dir
            .clone()
            .or_else(|| env_var("CARAVEL_WASM_DIR").map(PathBuf::from)),
    }
}

pub fn context(g: &Global, positional: Option<&Path>) -> Result<Ctx> {
    let (lane_path, file_from) = lane_file(g, positional)?;
    let lane = LaneFile::load(&lane_path)?;
    let (env, env_from) =
        project::choose_env(&lane, g.env.as_deref(), env_var("CARAVEL_ENV").as_deref())?;
    let cwd = std::env::current_dir()?;
    let (state_root, note) = project::state_root(&lane_path, &cwd, &lane.lane.name, &env);
    if let Some(n) = note {
        eprintln!("note: {n}");
    }
    Ok(Ctx {
        lane_path,
        lane,
        env,
        env_from,
        file_from,
        state_root,
        release: release_args(g),
    })
}

impl Ctx {
    /// `lanes/x/lane.toml [env.local] (default = true)`, for stderr.
    fn describe(&self) -> String {
        let shown = std::env::current_dir()
            .ok()
            .and_then(|c| self.lane_path.strip_prefix(c).ok().map(Path::to_path_buf))
            .unwrap_or_else(|| self.lane_path.clone());
        let file = match self.file_from {
            FileFrom::Above => format!("{} (found above)", shown.display()),
            _ => shown.display().to_string(),
        };
        format!("{file} [env.{}] ({})", self.env, self.env_from.describe())
    }

    fn template_name(&self) -> Result<String> {
        self.lane
            .app
            .as_ref()
            .map(|a| a.template.clone())
            .ok_or_else(|| anyhow!("{} has no [app] template", self.lane_path.display()))
    }

    fn template(&self) -> Result<Plugin> {
        Plugin::locate(&self.template_name()?)
    }

    async fn prepare(&self, for_apply: bool) -> Result<deploy::Prepared> {
        eprintln!("{}", self.describe());
        let t = self.template()?;
        let p = deploy::prepare(
            &t,
            &self.lane_path,
            &self.env,
            &self.release,
            for_apply,
            &self.state_root,
        )
        .await?;
        for n in &p.notes {
            eprintln!("note: {n}");
        }
        Ok(p)
    }
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?)
}

fn print_json(v: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

fn dispatch(cli: Cli) -> Result<u8> {
    let g = &cli.global;
    match cli.command {
        Cmd::Plan {
            lane,
            diff,
            exit_code,
        } => {
            let ctx = context(g, lane.as_deref())?;
            runtime()?.block_on(async {
                let p = ctx.prepare(false).await?;
                let plan = p.plan();
                if g.json {
                    let mut v = plan.to_json(&p.desired);
                    if diff {
                        v["diff"] = json!(p.file_diffs(&plan)?);
                    }
                    print_json(&v)?;
                } else {
                    print!("{}", plan.render(&p.desired));
                    if diff {
                        print!("\n{}", p.file_diffs(&plan)?);
                    }
                }
                Ok(
                    match (exit_code, plan.problems.is_empty(), plan.steps.is_empty()) {
                        (false, _, _) | (true, true, true) => exit::OK,
                        (true, false, _) => exit::ERROR,
                        (true, true, false) => exit::CHANGES,
                    },
                )
            })
        }
        Cmd::Apply { lane, yes } => {
            let ctx = context(g, lane.as_deref())?;
            runtime()?.block_on(async {
                let p = ctx.prepare(true).await?;
                let plan = p.plan();
                if g.json {
                    if !yes {
                        bail!("apply --json runs without a prompt: pass --yes");
                    }
                } else {
                    print!("{}", plan.render(&p.desired));
                }
                if !plan.problems.is_empty() {
                    if g.json {
                        print_json(
                            &json!({ "ok": false, "applied": 0, "plan": plan.to_json(&p.desired) }),
                        )?;
                    }
                    bail!("nothing applied: the plan has problems");
                }
                if plan.steps.is_empty() {
                    if g.json {
                        print_json(
                            &json!({ "ok": true, "applied": 0, "plan": plan.to_json(&p.desired) }),
                        )?;
                    }
                    return Ok(exit::OK);
                }
                if !deploy::confirm(plan.steps.len(), yes)? {
                    println!("Nothing applied.");
                    return Ok(exit::OK);
                }
                p.apply(&plan).await?;
                // Read everything back: a finished apply leaves nothing to do.
                let again = ctx.prepare(true).await?;
                let left = again.plan();
                if g.json {
                    print_json(&json!({
                        "ok": left.is_empty(),
                        "applied": plan.steps.len(),
                        "plan": plan.to_json(&p.desired),
                        "remaining": left.to_json(&again.desired),
                    }))?;
                } else if left.is_empty() {
                    println!("\nApplied. The lane matches the lane file.");
                } else {
                    print!(
                        "\nApplied, but the deployment still differs:\n{}",
                        left.render(&again.desired)
                    );
                }
                Ok(if left.is_empty() {
                    exit::OK
                } else {
                    exit::CHANGES
                })
            })
        }
        Cmd::Status { lane, exit_code } => {
            let ctx = context(g, lane.as_deref())?;
            runtime()?.block_on(async {
                let p = ctx.prepare(false).await?;
                if g.json {
                    print_json(&p.status_json().await)?;
                } else {
                    print!("{}", p.render_status().await);
                }
                Ok(if exit_code && !p.plan().is_empty() {
                    exit::CHANGES
                } else {
                    exit::OK
                })
            })
        }
        Cmd::Destroy {
            lane,
            yes,
            no_wait,
            stop_validators,
            pay_out,
            wipe,
        } => {
            let ctx = context(g, lane.as_deref())?;
            runtime()?.block_on(async {
                let p = ctx.prepare(true).await?;
                if !g.json {
                    print!("{}", p.render_status().await);
                } else if !yes {
                    bail!("destroy --json runs without a prompt: pass --yes");
                }
                if !yes && !deploy::confirm_destroy(&p.desired.lane_name)? {
                    println!("Nothing done.");
                    return Ok(exit::OK);
                }
                p.destroy(&DestroyOptions {
                    no_wait,
                    stop_validators,
                    pay_out,
                    wipe,
                })
                .await?;
                if g.json {
                    print_json(&p.status_json().await)?;
                }
                Ok(exit::OK)
            })
        }
        Cmd::Validate { lane } => validate(g, lane.as_deref()),
        Cmd::Env { cmd: EnvCmd::List } => env_list(g),
        Cmd::Output { name } => output(g, name.as_deref()),
        Cmd::Version => version(g),
        Cmd::Doctor => doctor(g),
    }
}

/// `validate`: every deployment (or the one --env names), offline.
fn validate(g: &Global, positional: Option<&Path>) -> Result<u8> {
    let (lane_path, _) = lane_file(g, positional)?;
    let (ok, report) = validate_report(&lane_path, g.env.as_deref())?;
    if g.json {
        print_json(&report)?;
    } else {
        for r in report["checks"].as_array().into_iter().flatten() {
            let mark = if r["ok"] == true { "ok " } else { "ERR" };
            let what = r["check"].as_str().unwrap_or_default();
            let detail = if let Some(e) = r["error"].as_str() {
                e.replace('\n', "\n      ")
            } else if let Some(m) = r["missing_identities"].as_array().filter(|m| !m.is_empty()) {
                let ids: Vec<_> = m.iter().filter_map(|v| v.as_str()).collect();
                format!(
                    "identities missing from the Stellar CLI keystore: {} (create them with `stellar keys generate <name>`)",
                    ids.join(", ")
                )
            } else if let Some(c) = r["config_hash"].as_str() {
                format!("config_hash {}…", &c[..16])
            } else {
                String::new()
            };
            println!("{mark} {what} {detail}");
        }
        if ok {
            println!("\n{} is valid.", lane_path.display());
        }
    }
    Ok(if ok { exit::OK } else { exit::ERROR })
}

/// What `validate` checks, without Stellar or a host: the genesis (through
/// the template's binary), each deployment's rules (`env`: only that one),
/// and whether the Stellar CLI keystore has the identities it names.
pub fn validate_report(lane_path: &Path, env: Option<&str>) -> Result<(bool, Value)> {
    let lane = LaneFile::load(lane_path)?;
    let names: Vec<String> = match env {
        Some(e) => vec![e.to_string()],
        None => envs(&lane).into_iter().map(|e| e.name).collect(),
    };
    let template = lane
        .app
        .as_ref()
        .map(|a| a.template.clone())
        .ok_or_else(|| anyhow!("{} has no [app] template", lane_path.display()))?;
    let mut checks = Vec::new();
    let mut ok = true;
    match Plugin::locate(&template).and_then(|t| t.genesis(&lane)) {
        Ok(h) => checks.push(json!({
            "check": "genesis",
            "ok": true,
            "lane_id": hex(&h.lane_id),
            "config_hash": hex(&h.config_hash),
            "genesis_state_hash": hex(&h.genesis_state_hash),
        })),
        Err(e) => {
            ok = false;
            checks.push(json!({ "check": "genesis", "ok": false, "error": format!("{e:#}") }));
        }
    }
    if names.is_empty() {
        ok = false;
        checks.push(json!({ "check": "deployments", "ok": false, "error": "the lane file has no [env.<name>] table" }));
    }
    for name in &names {
        match Manifest::from_lane(lane.clone(), name) {
            Ok(m) => {
                let missing: Vec<String> = identities(&m)
                    .into_iter()
                    .filter(|id| !Stellar::has_identity(id))
                    .collect();
                ok &= missing.is_empty();
                checks.push(json!({
                    "check": format!("[env.{name}]"),
                    "ok": missing.is_empty(),
                    "rules": true,
                    "missing_identities": missing,
                }));
            }
            Err(e) => {
                ok = false;
                checks.push(json!({ "check": format!("[env.{name}]"), "ok": false, "rules": false, "error": format!("{e:#}") }));
            }
        }
    }
    Ok((
        ok,
        json!({ "ok": ok, "lane_file": lane_path.display().to_string(), "checks": checks }),
    ))
}

/// The identities a deployment names, in a stable order.
pub fn identities(m: &Manifest) -> Vec<String> {
    let mut ids = vec![m.env.admin.clone(), m.env.relayer.account.clone()];
    ids.extend(m.env.validators.iter().map(|v| v.key.clone()));
    ids.extend(m.env.relayer.feed_keys.values().cloned());
    let mut seen = std::collections::BTreeSet::new();
    ids.retain(|i| seen.insert(i.clone()));
    ids
}

fn env_list(g: &Global) -> Result<u8> {
    let (lane_path, _) = lane_file(g, None)?;
    let lane = LaneFile::load(&lane_path)?;
    let list = env_list_json(&lane, g.env.as_deref(), env_var("CARAVEL_ENV").as_deref());
    if g.json {
        print_json(&list)?;
    } else {
        for e in list.as_array().into_iter().flatten() {
            println!(
                "{} {:<12} network {:<8} host {}{}",
                if e["selected"] == true { "*" } else { " " },
                e["name"].as_str().unwrap_or_default(),
                e["network"].as_str().unwrap_or("?"),
                e["provider"].as_str().unwrap_or("?"),
                if e["default"] == true {
                    "  (default)"
                } else {
                    ""
                }
            );
        }
        if list.as_array().is_none_or(|l| l.is_empty()) {
            println!("{} has no deployments ([env.<name>]).", lane_path.display());
        }
    }
    Ok(exit::OK)
}

/// The deployments, marking the one a command would pick.
pub fn env_list_json(lane: &LaneFile, explicit: Option<&str>, var: Option<&str>) -> Value {
    let chosen = project::choose_env(lane, explicit, var)
        .ok()
        .map(|(n, _)| n);
    json!(envs(lane)
        .iter()
        .map(|e| {
            json!({
                "name": e.name,
                "default": e.default,
                "network": e.network,
                "provider": e.provider,
                "selected": chosen.as_deref() == Some(e.name.as_str()),
            })
        })
        .collect::<Vec<_>>())
}

fn hex(k: &[u8]) -> String {
    k.iter().map(|b| format!("{b:02x}")).collect()
}

fn g_addr(k: &[u8; 32]) -> String {
    caravel_runtime::views::g_address(k)
}

fn c_addr(k: &[u8; 32]) -> String {
    caravel_deploy::address::strkey(k)
}

/// `output`: a deployment's addresses and URLs, without Stellar or the host.
fn output(g: &Global, name: Option<&str>) -> Result<u8> {
    let ctx = context(g, None)?;
    let m = Manifest::from_lane(ctx.lane.clone(), &ctx.env)?;
    let keys = Keys::from_keystore(&m)?;
    let a = addresses(&m, &keys)?;
    let validators: Vec<Value> = m
        .env
        .validators
        .iter()
        .zip(&keys.validators)
        .enumerate()
        .map(|(i, (v, k))| json!({ "name": v.name, "key": g_addr(k), "url": validator_url(&m, i) }))
        .collect();
    let all = json!({
        "lane": m.lane.lane.name,
        "lane_id": hex(&m.lane.lane_id()),
        "template": ctx.template_name()?,
        "env": m.env_name,
        "network": m.env.network.name(),
        "network_passphrase": m.env.network.passphrase(),
        "rpc_url": m.rpc_url(),
        "settlement": c_addr(&a.settlement),
        "settlement_pinned": a.settlement_pinned,
        "token": c_addr(&a.token),
        "token_asset": a.token_asset.as_ref().map(|(code, issuer)| format!("{code}:{}", g_addr(issuer))),
        "admin": g_addr(&keys.admin),
        "relayer": g_addr(&keys.relayer),
        "api": api_url(&m),
        "validators": validators,
    });
    match name {
        None if g.json => print_json(&all)?,
        None => {
            for (k, v) in all.as_object().expect("an object") {
                match v {
                    Value::String(s) => println!("{k} = {s}"),
                    Value::Null => println!("{k} = (none)"),
                    other => println!("{k} = {other}"),
                }
            }
        }
        Some(n) => {
            let v = all.get(n).ok_or_else(|| {
                let names: Vec<_> = all
                    .as_object()
                    .expect("an object")
                    .keys()
                    .cloned()
                    .collect();
                anyhow!("no output {n:?}; the outputs are {}", names.join(", "))
            })?;
            match (g.json, v) {
                (false, Value::String(s)) => println!("{s}"),
                (false, Value::Null) => bail!("{n} has no value in [env.{}]", m.env_name),
                _ => print_json(v)?,
            }
        }
    }
    Ok(exit::OK)
}

fn version(g: &Global) -> Result<u8> {
    let stellar = Stellar::version().ok();
    let want = caravel_deploy::versions::stellar_cli();
    let templates: Vec<Value> = template::installed()
        .into_iter()
        .map(|(path, p)| match p {
            Ok(p) => json!({
                "template": p.info.template,
                "version": p.info.version,
                "commit": p.info.commit,
                "protocol": p.info.protocol,
                "path": path.display().to_string(),
            }),
            Err(e) => json!({ "path": path.display().to_string(), "error": format!("{e:#}") }),
        })
        .collect();
    let release = caravel_deploy::release::installed_dir().map(|d| {
        let commit = std::fs::read_to_string(d.join("COMMIT")).unwrap_or_default();
        let dir = std::fs::canonicalize(&d).unwrap_or(d);
        json!({ "commit": commit.trim(), "dir": dir.display().to_string() })
    });
    let v = json!({
        "caravel": env!("CARGO_PKG_VERSION"),
        "commit": option_env!("CARAVEL_COMMIT").filter(|c| !c.is_empty()),
        "release": release,
        "plugin_protocol": caravel_node::plugin::PROTOCOL,
        "stellar_cli": { "installed": stellar, "needed": want },
        "templates": templates,
    });
    if g.json {
        print_json(&v)?;
    } else {
        println!(
            "caravel {}{}",
            env!("CARGO_PKG_VERSION"),
            option_env!("CARAVEL_COMMIT")
                .filter(|c| !c.is_empty())
                .map(|c| format!(" ({c})"))
                .unwrap_or_default()
        );
        println!(
            "stellar CLI {} (needs {want})",
            stellar.as_deref().unwrap_or("not found")
        );
        match &v["release"] {
            Value::Null => println!("release: this checkout's builds (no installed release)"),
            r => println!(
                "release: {} at {}",
                r["commit"].as_str().unwrap_or_default(),
                r["dir"].as_str().unwrap_or_default()
            ),
        }
        if templates.is_empty() {
            println!("templates: none found (caravel-<template>-node next to caravel or on PATH)");
        } else {
            println!("templates:");
            for t in &templates {
                match t["template"].as_str() {
                    Some(name) => println!(
                        "  {name:<10} {}  {}",
                        t["version"].as_str().unwrap_or_default(),
                        t["path"].as_str().unwrap_or_default()
                    ),
                    None => println!(
                        "  ?          {}: {}",
                        t["path"].as_str().unwrap_or_default(),
                        t["error"].as_str().unwrap_or_default()
                    ),
                }
            }
        }
    }
    Ok(exit::OK)
}

/// One `doctor` check.
struct Check {
    what: String,
    ok: bool,
    detail: String,
}

fn doctor(g: &Global) -> Result<u8> {
    let mut checks = Vec::new();
    let mut add = |what: &str, r: Result<String>| {
        let (ok, detail) = match r {
            Ok(d) => (true, d),
            Err(e) => (false, format!("{e:#}")),
        };
        checks.push(Check {
            what: what.into(),
            ok,
            detail,
        });
    };
    add(
        "stellar CLI",
        Stellar::check_version().map(|()| caravel_deploy::versions::stellar_cli().to_string()),
    );
    add("Node.js 22+", node_version());
    match context(g, None) {
        Err(e) => add("lane file", Err(e)),
        Ok(ctx) => {
            add("lane file", Ok(ctx.describe()));
            let m = Manifest::from_lane(ctx.lane.clone(), &ctx.env);
            let t = ctx.template();
            add(
                "template",
                t.as_ref()
                    .map(|t| {
                        format!(
                            "{} {} at {}",
                            t.info.template,
                            t.info.version,
                            t.path.display()
                        )
                    })
                    .map_err(|e| anyhow!("{e:#}")),
            );
            if let Ok(t) = &t {
                add(
                    "genesis",
                    t.genesis(&ctx.lane)
                        .map(|h| format!("config_hash {}…", &hex(&h.config_hash)[..16])),
                );
            }
            add("release", release_check(&ctx, t.as_ref().ok()));
            match m {
                Err(e) => add("deployment", Err(e)),
                Ok(m) => {
                    add(
                        "deployment",
                        Ok(format!("[env.{}] follows the rules", m.env_name)),
                    );
                    let missing: Vec<String> = identities(&m)
                        .into_iter()
                        .filter(|id| !Stellar::has_identity(id))
                        .collect();
                    add(
                        "identities",
                        if missing.is_empty() {
                            Ok(format!(
                                "{} in the Stellar CLI keystore",
                                identities(&m).len()
                            ))
                        } else {
                            Err(anyhow!(
                                "missing: {} (create them with `stellar keys generate <name>`)",
                                missing.join(", ")
                            ))
                        },
                    );
                    if m.env.network == caravel_deploy::manifest::Network::Local {
                        add("Docker (local network)", docker());
                    }
                }
            }
        }
    }
    let ok = checks.iter().all(|c| c.ok);
    if g.json {
        let v: Vec<Value> = checks
            .iter()
            .map(|c| json!({ "check": c.what, "ok": c.ok, "detail": c.detail }))
            .collect();
        print_json(&json!({ "ok": ok, "checks": v }))?;
    } else {
        for c in &checks {
            println!(
                "{} {:<24} {}",
                if c.ok { "ok " } else { "ERR" },
                c.what,
                c.detail.replace('\n', "\n                             ")
            );
        }
    }
    Ok(if ok { exit::OK } else { exit::ERROR })
}

fn node_version() -> Result<String> {
    let out = std::process::Command::new("node")
        .arg("--version")
        .output()
        .context("node is not installed")?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let major: u32 = v
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|m| m.parse().ok())
        .ok_or_else(|| anyhow!("node --version said {v:?}"))?;
    if major < 22 {
        bail!("node {v} is installed; the relayer needs 22 or later");
    }
    Ok(v)
}

fn docker() -> Result<String> {
    let out = std::process::Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .output()
        .context("docker is not installed")?;
    if !out.status.success() {
        bail!("docker is not running");
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn release_check(ctx: &Ctx, t: Option<&Plugin>) -> Result<String> {
    let template = ctx.template_name()?;
    let mut r =
        caravel_deploy::release::Release::locate(ctx.release.release_dir.as_deref(), &template)?;
    if let Some(w) = &ctx.release.wasm_dir {
        r.use_wasm_from(w)?;
    }
    let engine = t
        .map(|t| t.info.engine_file.clone())
        .unwrap_or_else(|| caravel_deploy::render::engine_file(&template));
    let have = r.wasm_hash(&engine)?;
    let want = ctx
        .lane
        .engine_wasm_hash()?
        .ok_or_else(|| anyhow!("[app] engine_wasm_sha256 is required"))?;
    if have != want {
        bail!(
            "the release's {engine} is {}…, the lane file names {}…",
            &hex(&have)[..16],
            &hex(&want)[..16]
        );
    }
    Ok(format!("{} (engine {}…)", r.commit, &hex(&want)[..16]))
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::{Cli, Cmd};

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    /// The forms the RUNBOOK and the e2e use keep working.
    #[test]
    fn older_command_lines_parse() {
        let c = Cli::try_parse_from([
            "caravel",
            "plan",
            "lanes/perps/config/lane.caravel-perps.testnet.toml",
            "--env",
            "testnet",
            "--release-dir",
            "/tmp/release",
            "--diff",
        ])
        .unwrap();
        assert_eq!(c.global.env.as_deref(), Some("testnet"));
        assert!(matches!(
            c.command,
            Cmd::Plan {
                lane: Some(_),
                diff: true,
                ..
            }
        ));
        let c = Cli::try_parse_from(["caravel", "apply", "lane.toml", "--env", "e2e", "--yes"])
            .unwrap();
        assert!(matches!(c.command, Cmd::Apply { yes: true, .. }));
        let c = Cli::try_parse_from(["caravel", "status", "lane.toml", "--env", "e2e", "--json"])
            .unwrap();
        assert!(c.global.json);
        let c = Cli::try_parse_from(["caravel", "destroy", "--yes", "--pay-out", "-f", "x.toml"])
            .unwrap();
        assert!(matches!(
            c.command,
            Cmd::Destroy {
                yes: true,
                pay_out: true,
                ..
            }
        ));
        // And the new ones.
        let c = Cli::try_parse_from(["caravel", "-e", "local", "plan", "--exit-code"]).unwrap();
        assert!(matches!(
            c.command,
            Cmd::Plan {
                lane: None,
                exit_code: true,
                ..
            }
        ));
        assert!(Cli::try_parse_from(["caravel", "output", "settlement", "--json"]).is_ok());
        assert!(Cli::try_parse_from(["caravel", "env", "list"]).is_ok());
        assert!(Cli::try_parse_from(["caravel", "plan", "--force"]).is_err());
    }
}
