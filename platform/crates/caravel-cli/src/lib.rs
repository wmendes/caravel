//! `caravel`, the Caravel CLI (M0.6, spec §20.5): one binary for any
//! template's lanes. It finds the lane file and the deployment
//! ([`project`]), asks the template's binary only what the template knows
//! (genesis, bodies, examples: `caravel_node::plugin`), and does the rest
//! itself through the deploy library (`caravel_deploy`).
//!
//! Every command takes `--json` (one JSON document on stdout; progress and
//! notes on stderr). Exit codes are in [`exit`].

pub mod init;
pub mod project;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, bail, Context, Result};
use caravel_deploy::cli::ReleaseArgs;
use caravel_deploy::deploy::{self, addresses, api_url, validator_url, Keys};
use caravel_deploy::manifest::{envs, Inputs, Manifest, Network};
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
    /// `wait`: the time ran out.
    pub const TIMEOUT: u8 = 4;
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
    /// Set one of the lane file's vars; repeat for more. Read by the var's
    /// type (lists and maps as TOML: --var 'validators=["1","2"]'). Also
    /// CARAVEL_VAR_<name>.
    #[arg(long = "var", global = true, value_name = "NAME=VALUE")]
    pub vars: Vec<String>,
    /// A TOML file of var values (name = value); later files win, and --var
    /// over them.
    #[arg(long = "var-file", global = true, value_name = "FILE")]
    pub var_files: Vec<PathBuf>,
}

impl Global {
    pub fn inputs(&self) -> Result<Inputs> {
        Inputs::new(&self.vars, &self.var_files).map_err(|e| anyhow!(e))
    }
}

/// `--target` and `--replace`, for `plan` and `apply` (C-16).
#[derive(clap::Args, Debug, Default, Clone)]
pub struct Scope {
    /// Plan only for this resource and what it depends on: an address from
    /// the plan or `caravel graph` (node.validator-2, file.sequencer.toml); `*`
    /// matches any characters (node.*). Repeat for more.
    #[arg(long, value_name = "ADDR")]
    pub target: Vec<String>,
    /// Replace this node, file or the release even if it matches: a node
    /// restarts, a file is written again, the release is installed again.
    /// Repeat for more.
    #[arg(long, value_name = "ADDR")]
    pub replace: Vec<String>,
}

impl Scope {
    fn options(&self) -> caravel_deploy::plan::Options {
        caravel_deploy::plan::Options {
            target: self.target.clone(),
            replace: self.replace.clone(),
        }
    }
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
        #[command(flatten)]
        scope: Scope,
        /// Save the plan here, for `caravel apply FILE` to apply exactly it
        /// (it refuses if anything moved meanwhile). Holds no secret.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
    },
    /// Make Stellar and the host match the deployment. Shows the plan and
    /// asks first, unless --yes. Running it again changes nothing. Given a
    /// saved plan (`caravel plan --out FILE`), applies exactly that plan.
    Apply {
        /// A saved plan, or the lane file (the same as -f).
        lane: Option<PathBuf>,
        /// Apply without asking (also CARAVEL_YES=1).
        #[arg(short = 'y', long)]
        yes: bool,
        #[command(flatten)]
        scope: Scope,
    },
    /// The deployment's resources and what each depends on, as Graphviz DOT
    /// (`caravel graph | dot -Tsvg > lane.svg`), or as JSON. Dashed edges
    /// restart their target when their source changes.
    Graph {
        /// The lane file (the same as -f).
        lane: Option<PathBuf>,
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
        /// Don't ask (the lane can't be restarted afterwards; also CARAVEL_YES=1).
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
    /// The deployment as `plan` reads it: includes, inheritance, vars and
    /// expressions resolved. With --genesis, the genesis document the
    /// template hashes and the hosts get as lane.toml.
    Render {
        /// The lane file (the same as -f).
        lane: Option<PathBuf>,
        #[arg(long)]
        genesis: bool,
    },
    /// Stop nodes (all of them when none is named) and leave the lane as it
    /// is: no checkpoint is posted while the sequencer is down, and after the
    /// contract's escape timeout anyone may freeze the lane.
    Stop {
        /// sequencer, relayer, validator-<n> (or <n>).
        nodes: Vec<String>,
        /// Don't ask, even off a local network (also CARAVEL_YES=1).
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Start nodes that aren't running (all when none is named). Refuses if
    /// the deployment differs from the lane file in anything else: that's
    /// apply's.
    Start { nodes: Vec<String> },
    /// Stop and start nodes (all when none is named).
    Restart { nodes: Vec<String> },
    /// A node's log: the last lines, then with --follow what it writes.
    Logs {
        /// sequencer (the default), relayer, validator-<n> (or <n>).
        node: Option<String>,
        /// Keep printing what it writes (-f is the lane file, as everywhere).
        #[arg(long)]
        follow: bool,
        #[arg(short = 'n', long, default_value_t = 50)]
        lines: usize,
    },
    /// Replay the lane from Stellar alone (spec §16), with the deployment's
    /// contract, network and engine; optionally an account's proofs.
    Replay {
        /// An identity or G… account: print its escape proof.
        #[arg(long, value_name = "WHO")]
        prove_escape: Option<String>,
        /// An identity or G… account: print its unclaimed withdrawal proofs.
        #[arg(long, value_name = "WHO")]
        prove_withdrawals: Option<String>,
    },
    /// Wait until something holds (exit 4 if the time runs out).
    Wait {
        #[command(subcommand)]
        what: WaitCmd,
        /// Seconds.
        #[arg(long, default_value_t = 120, global = true)]
        timeout: u64,
        /// The lane's API, if the host has no public_url.
        #[arg(long, global = true)]
        api_url: Option<String>,
    },
    /// GET a path of the lane's API (or a validator's) and print the JSON.
    Api {
        /// e.g. /v1/status, /v1/accounts/G…
        path: String,
        #[arg(long)]
        validator: Option<String>,
        #[arg(long)]
        api_url: Option<String>,
    },
    /// A user's Stellar account for the lane: an identity on the network with
    /// a trustline to the settlement token, and some of it.
    Account {
        #[command(subcommand)]
        cmd: AccountCmd,
    },
    /// An account's settlement token on Stellar, and its lane account (a
    /// C… contract's on Stellar only).
    Balance {
        /// An identity, a G… account or a C… contract.
        who: String,
    },
    /// Deposit into the lane; returns once the lane has credited it.
    Deposit {
        /// The identity that pays (it signs).
        who: String,
        /// In token units, e.g. 50 or 12.5.
        amount: String,
        /// Return once the deposit is on Stellar, before the lane credits it.
        #[arg(long)]
        no_wait: bool,
        /// Seconds to wait for the credit.
        #[arg(long, default_value_t = 120)]
        timeout: u64,
    },
    /// Sign a lane transaction with SEP-53 through the Stellar CLI keystore
    /// and send it; waits until a block takes it and prints its result.
    Tx {
        /// The identity that signs: the lane account's owner.
        #[arg(long)]
        from: String,
        /// Return once the sequencer has it, before a block takes it.
        #[arg(long)]
        no_wait: bool,
        #[arg(long, default_value_t = 60)]
        expiry_secs: u64,
        #[arg(long, default_value_t = 120)]
        timeout: u64,
        /// The nonce to sign; by default the one after the account's queued
        /// transactions.
        #[arg(long)]
        nonce: Option<u64>,
        /// The body, in the template's `tx` syntax, amounts in token units;
        /// `@name` is an identity's G… account (e.g. transfer --to @bob --amount 5).
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        body: Vec<String>,
    },
    /// Withdraw from the lane: the transaction, then its leaf in an accepted
    /// checkpoint, then the claim on Stellar.
    Withdraw {
        who: String,
        /// In token units.
        amount: String,
        /// Return once the transaction is sent.
        #[arg(long)]
        no_wait: bool,
        /// Wait for the leaf but don't claim it.
        #[arg(long)]
        no_claim: bool,
        #[arg(long, default_value_t = 300)]
        timeout: u64,
    },
    /// Ask the settlement contract for a withdrawal the lane must include
    /// (when it won't take one), then claim it.
    ForceWithdraw {
        who: String,
        amount: String,
        #[arg(long)]
        no_wait: bool,
        #[arg(long)]
        no_claim: bool,
        #[arg(long, default_value_t = 300)]
        timeout: u64,
    },
    /// Claim every unclaimed withdrawal of an account on Stellar.
    Claim { who: String },
    /// After a freeze: claim an account's withdrawals and its share of the
    /// lane's last checkpoint.
    Escape {
        who: String,
        /// Leave its withdrawals unclaimed.
        #[arg(long)]
        no_withdrawals: bool,
        /// Claim even if it pays nothing (it uses the escape up).
        #[arg(long)]
        allow_zero: bool,
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
    /// Start a lane: write DIR/lane.toml from a template's example, and
    /// create the Stellar CLI identities it names (`<name>-<role>`).
    Init {
        /// The template (one of `caravel init --list`); the only one
        /// installed, if there is one.
        template: Option<String>,
        /// Where to write lane.toml (default: here); its name is the
        /// lane's unless --name.
        dir: Option<PathBuf>,
        /// The lane's name ([lane] name; it fixes the lane's id).
        #[arg(long)]
        name: Option<String>,
        /// The sequencer's port (default: the first free one from 18080;
        /// validators take the next ones).
        #[arg(long)]
        port: Option<u16>,
        /// The identities' prefix (default: the lane's name).
        #[arg(long)]
        prefix: Option<String>,
        /// Replace an existing lane.toml.
        #[arg(long)]
        force: bool,
        /// List the templates instead.
        #[arg(long)]
        list: bool,
    },
    /// The Stellar CLI identities a deployment names.
    Keys {
        #[command(subcommand)]
        cmd: KeysCmd,
    },
    /// This CLI, the templates it finds and the Stellar CLI it needs.
    Version,
    /// Check that this machine can run the deployment: the Stellar CLI,
    /// Node.js, the template, the release and the identities.
    Doctor,
}

#[derive(Subcommand, Debug)]
pub enum WaitCmd {
    /// A checkpoint accepted on Stellar: the next one, or --seq.
    Checkpoint {
        #[arg(long)]
        seq: Option<u64>,
        /// Signed by the validators is enough.
        #[arg(long)]
        signed: bool,
        /// Under this signer epoch.
        #[arg(long)]
        epoch: Option<u64>,
    },
    /// A value in the lane's API: `caravel wait api /v1/markets /0/oracle_price=65000000`.
    Api {
        path: String,
        /// A JSON pointer, and optionally `=value`.
        until: String,
    },
    /// Every node running and answering.
    Healthy,
    /// The settlement contract frozen.
    Frozen,
}

#[derive(Subcommand, Debug)]
pub enum AccountCmd {
    /// Create an identity (if the keystore lacks it) and fund it.
    Create {
        name: String,
        /// This much of the settlement token too, in token units.
        #[arg(long, alias = "fund")]
        amount: Option<String>,
        /// At most this much XLM to buy it with (Circle's testnet USDC).
        #[arg(long, default_value = "9000")]
        max_xlm: String,
    },
    /// Fund an existing identity: XLM by friendbot, a trustline, and tokens.
    Fund {
        name: String,
        #[arg(long)]
        amount: Option<String>,
        #[arg(long, default_value = "9000")]
        max_xlm: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum KeysCmd {
    /// Each role's identity, and whether the keystore has it.
    List,
    /// Create the identities the keystore lacks (they stay in the Stellar
    /// CLI's keystore).
    Ensure,
    /// An identity's public key: an identity name, or a role (admin,
    /// relayer, validator-<name>).
    Show { who: String },
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
    pub inputs: Inputs,
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
    let lane = caravel_deploy::manifest::load_lane(&lane_path)?;
    let (env, env_from) =
        project::choose_env(&lane, g.env.as_deref(), env_var("CARAVEL_ENV").as_deref())?;
    let cwd = std::env::current_dir()?;
    let (state_root, note) = project::state_root(&lane_path, &cwd, &lane.lane.name, &env);
    if let Some(n) = note {
        eprintln!("note: {n}");
    }
    Ok(Ctx {
        inputs: g.inputs()?,
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

    fn manifest(&self) -> Result<Manifest> {
        Manifest::load_with(&self.lane_path, &self.env, &self.inputs)
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
            &self.inputs,
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
            scope,
            out,
        } => {
            let ctx = context(g, lane.as_deref())?;
            let m = ctx.manifest()?;
            let missing = init::missing_identities(&m);
            if !missing.is_empty() {
                if m.env.network != Network::Local {
                    bail!(
                        "the keystore lacks {}: create them with `caravel keys ensure`",
                        missing.join(", ")
                    );
                }
                // Addresses derive from the admin's key: the rest of the plan
                // needs these first.
                if g.json {
                    print_json(
                        &json!({ "create_identities": missing, "steps": [], "problems": [] }),
                    )?;
                } else {
                    eprintln!("{}", ctx.describe());
                    for id in &missing {
                        println!("+ identity  {id} (Stellar CLI keystore)");
                    }
                    println!("\napply creates these identities first, then plans the rest with their keys.");
                }
                return Ok(if exit_code { exit::CHANGES } else { exit::OK });
            }
            runtime()?.block_on(async {
                let p = ctx.prepare(false).await?;
                let plan = p.plan_with(&scope.options())?;
                if let Some(out) = &out {
                    let origin = caravel_deploy::saved::Origin {
                        lane_file: std::fs::canonicalize(&ctx.lane_path)
                            .unwrap_or_else(|_| ctx.lane_path.clone()),
                        env: ctx.env.clone(),
                        release_dir: ctx.release.release_dir.clone(),
                        wasm_dir: ctx.release.wasm_dir.clone(),
                    };
                    let doc = caravel_deploy::saved::save(
                        &p,
                        &plan,
                        &scope.options(),
                        &origin,
                        &caravel_version(),
                    )?;
                    std::fs::write(out, serde_json::to_string_pretty(&doc)? + "\n")
                        .with_context(|| format!("writing {}", out.display()))?;
                    eprintln!(
                        "Saved to {}: `caravel apply {}` applies exactly this plan.",
                        out.display(),
                        out.display()
                    );
                }
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
        Cmd::Apply { lane, yes, scope } => {
            let yes = yes || assume_yes();
            if let Some(path) = lane.as_deref().filter(|p| {
                std::fs::read_to_string(p).is_ok_and(|t| caravel_deploy::saved::is_saved_plan(&t))
            }) {
                if !scope.target.is_empty() || !scope.replace.is_empty() {
                    bail!("a saved plan keeps the --target and --replace it was made with");
                }
                return apply_saved(g, path, yes);
            }
            let ctx = context(g, lane.as_deref())?;
            let m = ctx.manifest()?;
            let missing = init::missing_identities(&m);
            if !missing.is_empty() {
                if m.env.network != Network::Local {
                    bail!(
                        "the keystore lacks {}: create them with `caravel keys ensure` (and fund them), then apply",
                        missing.join(", ")
                    );
                }
                // A local network's identities are throwaway keys: apply
                // makes the ones its lane file names.
                for id in init::ensure_identities(&m)? {
                    eprintln!("+ identity  {id} (created in the Stellar CLI keystore)");
                }
            }
            runtime()?.block_on(async {
                let p = ctx.prepare(true).await?;
                let plan = p.plan_with(&scope.options())?;
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
                // (The same targets; a replacement is done once.)
                let again = ctx.prepare(true).await?;
                let left = again.plan_with(&caravel_deploy::plan::Options {
                    target: scope.target.clone(),
                    replace: vec![],
                })?;
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
            let yes = yes || assume_yes();
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
        Cmd::Graph { lane } => {
            let ctx = context(g, lane.as_deref())?;
            runtime()?.block_on(async {
                let p = ctx.prepare(false).await?;
                let graph = p.graph()?;
                if g.json {
                    let kind = |k: caravel_deploy::graph::Kind| format!("{k:?}").to_lowercase();
                    print_json(&json!({
                        "resources": graph.resources.iter().map(|r| json!({ "address": r.addr, "kind": kind(r.kind) })).collect::<Vec<_>>(),
                        "edges": graph.edges.iter().map(|e| json!({
                            "from": graph.resources[e.from].addr,
                            "to": graph.resources[e.to].addr,
                            "kind": format!("{:?}", e.kind).to_lowercase(),
                        })).collect::<Vec<_>>(),
                    }))?;
                } else {
                    print!("{}", graph.dot());
                }
                Ok(exit::OK)
            })
        }
        Cmd::Stop { nodes, yes } => {
            let yes = yes || assume_yes();
            let ctx = context(g, None)?;
            runtime()?.block_on(async {
                let p = ctx.prepare(false).await?;
                let names = p.nodes(&nodes)?;
                // What stops checkpoints: the sequencer, the relayer (it
                // posts them), or validators below the signing threshold.
                let stopped_weight: u32 = p
                    .m
                    .env
                    .validators
                    .iter()
                    .filter(|v| names.contains(&caravel_deploy::render::validator_node(&v.name)))
                    .map(|v| v.weight)
                    .sum();
                let total: u32 = p.m.env.validators.iter().map(|v| v.weight).sum();
                let halts = names.iter().any(|n| n == "sequencer" || n == "relayer")
                    || total.saturating_sub(stopped_weight) < p.m.env.threshold;
                if halts {
                    eprintln!(
                        "Stopping {} stops checkpoints. {}",
                        names.join(", "),
                        match p.freeze_possible_at() {
                            Some(t) => format!(
                                "Unless they start again, anyone may freeze the lane from {t} (in {} s).",
                                t.saturating_sub(now())
                            ),
                            None => "Anyone may freeze the lane after the contract's escape timeout without one.".into(),
                        }
                    );
                    if p.m.env.network != Network::Local && !yes {
                        if g.json || !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                            bail!("this stops a testnet lane's checkpoints: pass --yes");
                        }
                        eprint!("Stop {}? [y/N] ", names.join(", "));
                        let mut line = String::new();
                        std::io::stdin().read_line(&mut line)?;
                        if !matches!(line.trim(), "y" | "Y" | "yes") {
                            println!("Nothing stopped.");
                            return Ok(exit::OK);
                        }
                    }
                }
                p.stop_nodes_named(&names)?;
                if g.json {
                    print_json(&json!({ "stopped": names }))?;
                } else {
                    println!("Stopped {}. `caravel start` brings them back.", names.join(", "));
                }
                Ok(exit::OK)
            })
        }
        Cmd::Start { nodes } => {
            let ctx = context(g, None)?;
            runtime()?.block_on(async {
                let p = ctx.prepare(true).await?;
                let names = if nodes.is_empty() {
                    Vec::new()
                } else {
                    p.nodes(&nodes)?
                };
                let plan = p.start_plan(&names)?;
                let started: Vec<String> = plan
                    .steps
                    .iter()
                    .filter_map(|s| match s {
                        caravel_deploy::plan::Step::Start { node }
                        | caravel_deploy::plan::Step::Restart { node } => Some(node.clone()),
                        _ => None,
                    })
                    .collect();
                if plan.steps.is_empty() {
                    if g.json {
                        print_json(&json!({ "started": [] }))?;
                    } else {
                        println!("Nothing to start: the nodes are running.");
                    }
                    return Ok(exit::OK);
                }
                p.apply(&plan).await?;
                if g.json {
                    print_json(&json!({ "started": started }))?;
                } else {
                    println!("Started {}.", started.join(", "));
                }
                Ok(exit::OK)
            })
        }
        Cmd::Restart { nodes } => {
            let ctx = context(g, None)?;
            runtime()?.block_on(async {
                let p = ctx.prepare(true).await?;
                let mut names = p.nodes(&nodes)?;
                // Start order: validators, the sequencer, the relayer.
                names.reverse();
                let mut plan = p.plan();
                if !plan.problems.is_empty() {
                    bail!("the plan has problems: run `caravel plan`");
                }
                plan.steps = names
                    .iter()
                    .map(|n| caravel_deploy::plan::Step::Restart { node: n.clone() })
                    .collect();
                p.apply(&plan).await?;
                if g.json {
                    print_json(&json!({ "restarted": names }))?;
                } else {
                    println!("Restarted {}.", names.join(", "));
                }
                Ok(exit::OK)
            })
        }
        Cmd::Logs {
            node,
            follow,
            lines,
        } => {
            let ctx = context(g, None)?;
            let m = ctx.manifest()?;
            let mut all = vec!["sequencer".to_string(), "relayer".to_string()];
            all.extend(
                m.env
                    .validators
                    .iter()
                    .map(|v| caravel_deploy::render::validator_node(&v.name)),
            );
            let n =
                caravel_deploy::lifecycle::node_name(node.as_deref().unwrap_or("sequencer"), &all)?;
            let host = caravel_deploy::deploy::node_host_provider(
                &m,
                &ctx.template_name()?,
                &ctx.state_root,
                &n,
            )?;
            host.logs(&n, lines, follow)?;
            Ok(exit::OK)
        }
        Cmd::Account { cmd } => {
            let (name, amount, max_xlm, create) = match cmd {
                AccountCmd::Create {
                    name,
                    amount,
                    max_xlm,
                } => (name, amount, max_xlm, true),
                AccountCmd::Fund {
                    name,
                    amount,
                    max_xlm,
                } => (name, amount, max_xlm, false),
            };
            let ctx = context(g, None)?;
            let flows = flows_for(&ctx)?;
            let created = create && !Stellar::has_identity(&name);
            if created {
                Stellar::generate_identity(&name)?;
            } else if !Stellar::has_identity(&name) {
                bail!("no identity {name:?}: `caravel account create {name}`");
            }
            let d = flows.decimals()?;
            let amount = amount
                .map(|a| caravel_deploy::flows::parse_units(&a, d))
                .transpose()?;
            let max_xlm = caravel_deploy::flows::parse_units(&max_xlm, 7)?;
            runtime()?.block_on(async {
                let mut r = flows.fund(&name, amount, max_xlm).await?;
                r["created"] = json!(created);
                if g.json {
                    print_json(&r)?;
                } else {
                    println!(
                        "{name} ({}){}",
                        r["account"].as_str().unwrap_or_default(),
                        if created { ", a new identity" } else { "" }
                    );
                    for step in r["did"].as_array().into_iter().flatten() {
                        println!("  {}", step.as_str().unwrap_or_default());
                    }
                    if let Some(t) = r["token"].as_str() {
                        println!("  holds {t} of the settlement token");
                    }
                }
                Ok(exit::OK)
            })
        }
        Cmd::Balance { who } => {
            let ctx = context(g, None)?;
            let flows = flows_for(&ctx)?;
            // A contract (the settlement's vault, say): Stellar only.
            if let Some(c) = caravel_deploy::address::parse_contract(&who) {
                let b = flows.contract_balance(&c)?;
                if g.json {
                    print_json(&b)?;
                } else {
                    println!(
                        "{who}\n  on Stellar  {}",
                        b["stellar"].as_str().unwrap_or_default()
                    );
                }
                return Ok(exit::OK);
            }
            let key = Stellar::public_key(&who).or_else(|_| {
                stellar_strkey(&who)
                    .ok_or_else(|| anyhow!("{who:?} is neither an identity nor a G… account"))
            })?;
            runtime()?.block_on(async {
                let b = flows.balance(&who, &key).await?;
                if g.json {
                    print_json(&b)?;
                } else {
                    println!("{who} ({})", b["account"].as_str().unwrap_or_default());
                    match (b["stellar"].as_str(), b["stellar_error"].as_str()) {
                        (Some(v), _) => println!("  on Stellar  {v}"),
                        (None, Some(e)) => println!("  on Stellar  (couldn't read it: {e})"),
                        (None, None) => println!("  on Stellar  (no trustline)"),
                    }
                    match (&b["lane"], b["lane_error"].as_str()) {
                        (Value::Null, Some(e)) => {
                            println!("  on the lane (the lane's API didn't answer: {e})")
                        }
                        (Value::Null, None) => {
                            println!("  on the lane (no lane account yet: deposit first)")
                        }
                        (lane, _) => println!("  on the lane {lane}"),
                    }
                }
                Ok(exit::OK)
            })
        }
        Cmd::Deposit {
            who,
            amount,
            no_wait,
            timeout,
        } => {
            let ctx = context(g, None)?;
            let flows = flows_for(&ctx)?;
            let amount = caravel_deploy::flows::parse_units(&amount, flows.decimals()?)?;
            runtime()?.block_on(async {
                let wait = (!no_wait).then(|| std::time::Duration::from_secs(timeout));
                let r = flows.deposit(&who, amount, wait).await?;
                let amt = r["amount"].as_str().unwrap_or_default().to_string();
                let code = if r["bounced"] == true {
                    exit::ERROR
                } else if r["timed_out"] == true {
                    exit::TIMEOUT
                } else {
                    exit::OK
                };
                if g.json {
                    print_json(&r)?;
                } else if r["credited"] == true {
                    println!("Deposited {amt} for {who}; the lane credited it (inbox message {}).", r["inbox_index"]);
                } else if r["bounced"] == true {
                    eprintln!("The lane refused the deposit of {amt} (inbox message {}): it comes back as a withdrawal, `caravel claim {who}` once a checkpoint is accepted.", r["inbox_index"]);
                } else if r["timed_out"] == true {
                    eprintln!("Deposited {amt} for {who} (inbox message {}), but the lane hasn't processed it after {timeout} s: is the relayer running?", r["inbox_index"]);
                } else {
                    println!("Deposited {amt} for {who} (inbox message {}); the lane credits it at its next block.", r["inbox_index"]);
                }
                Ok(code)
            })
        }
        Cmd::Tx {
            from,
            no_wait,
            expiry_secs,
            timeout,
            nonce,
            body,
        } => {
            let ctx = context(g, None)?;
            let flows = flows_for(&ctx)?;
            let template = ctx.template()?;
            // `@name` is that identity's account; `@@` a literal `@`.
            let args: Vec<String> = body
                .iter()
                .map(|a| match a.strip_prefix('@') {
                    Some(rest) if rest.starts_with('@') => Ok(rest.to_string()),
                    Some(id) => {
                        Stellar::public_key(id).map(|k| caravel_runtime::views::g_address(&k))
                    }
                    None => Ok(a.clone()),
                })
                .collect::<Result<_>>()?;
            let (kind, bytes) = template.body(&args, Some(flows.decimals()?))?;
            runtime()?.block_on(async {
                let sent = flows.send(&from, kind, bytes, expiry_secs, nonce).await?;
                let mut r = json!({ "tx_hash": sent.tx_hash, "nonce": sent.nonce.to_string(), "kind": kind });
                if !no_wait {
                    // Sent: whatever happens next, it may still land.
                    eprintln!("Sent {} (nonce {}); waiting for a block.", sent.tx_hash, sent.nonce);
                }
                if no_wait {
                    if g.json {
                        print_json(&r)?;
                    } else {
                        println!("Sent {} (nonce {}).", sent.tx_hash, sent.nonce);
                    }
                    return Ok(exit::OK);
                }
                use caravel_deploy::flows::{receipt_name, Outcome};
                let code = match flows.included(&sent, std::time::Duration::from_secs(timeout)).await? {
                    Outcome::Included { height, code, events } => {
                        r["height"] = json!(height);
                        r["code"] = json!(code);
                        r["result"] = json!(receipt_name(code));
                        r["events"] = json!(events);
                        if !g.json {
                            if code == 0 {
                                println!("Included at height {height}: OK.");
                            } else {
                                eprintln!("Included at height {height}, but the lane refused it: {}.", receipt_name(code));
                            }
                            for e in &events {
                                println!("  {e}");
                            }
                        }
                        if code == 0 {
                            exit::OK
                        } else {
                            exit::ERROR
                        }
                    }
                    Outcome::Dropped => {
                        r["dropped"] = json!(true);
                        if !g.json {
                            eprintln!("Not included: {}.", caravel_deploy::flows::DROPPED);
                        }
                        exit::ERROR
                    }
                    Outcome::Pending => {
                        r["timed_out"] = json!(true);
                        if !g.json {
                            eprintln!("Not in a block after {timeout} s.");
                        }
                        exit::TIMEOUT
                    }
                };
                if g.json {
                    print_json(&r)?;
                }
                Ok(code)
            })
        }
        Cmd::Withdraw {
            who,
            amount,
            no_wait,
            no_claim,
            timeout,
        } => {
            let ctx = context(g, None)?;
            let flows = flows_for(&ctx)?;
            let amount = caravel_deploy::flows::parse_units(&amount, flows.decimals()?)?;
            runtime()?.block_on(async {
                let wait = (!no_wait).then(|| std::time::Duration::from_secs(timeout));
                let r = flows.withdraw(&who, amount, wait, !no_claim).await?;
                user_result(g, &r, "withdrawal")
            })
        }
        Cmd::ForceWithdraw {
            who,
            amount,
            no_wait,
            no_claim,
            timeout,
        } => {
            let ctx = context(g, None)?;
            let flows = flows_for(&ctx)?;
            let amount = caravel_deploy::flows::parse_units(&amount, flows.decimals()?)?;
            runtime()?.block_on(async {
                let wait = (!no_wait).then(|| std::time::Duration::from_secs(timeout));
                let r = flows.force_withdraw(&who, amount, wait, !no_claim).await?;
                user_result(g, &r, "forced withdrawal")
            })
        }
        Cmd::Claim { who } => {
            let ctx = context(g, None)?;
            let flows = flows_for(&ctx)?;
            let key = Stellar::public_key(&who)?;
            let d = flows.decimals()?;
            runtime()?.block_on(async {
                use caravel_deploy::flows::format_units;
                let open = flows.unclaimed(&key).await?;
                let c = flows.claim(&who, &open);
                if g.json {
                    print_json(&c.json(d))?;
                } else if open.is_empty() {
                    println!("Nothing to claim for {who}.");
                } else {
                    if !c.paid.is_empty() {
                        let total: i128 = c.paid.iter().map(|l| l.amount).sum();
                        println!(
                            "Claimed {} for {who} ({} withdrawal(s)).",
                            format_units(total, d),
                            c.paid.len()
                        );
                    }
                    if !c.already.is_empty() {
                        println!(
                            "{} withdrawal(s) were claimed meanwhile (a claim always pays {who}).",
                            c.already.len()
                        );
                    }
                    for (l, e) in &c.failed {
                        eprintln!(
                            "Claiming {} from checkpoint {} (leaf {}) failed: {e}",
                            format_units(l.amount, d),
                            l.seq,
                            l.index
                        );
                    }
                }
                Ok(if c.failed.is_empty() {
                    exit::OK
                } else {
                    exit::ERROR
                })
            })
        }
        Cmd::Escape {
            who,
            no_withdrawals,
            allow_zero,
        } => {
            let ctx = context(g, None)?;
            let flows = flows_for(&ctx)?;
            runtime()?.block_on(async {
                let r = flows.escape(&who, !no_withdrawals, allow_zero).await?;
                let w = &r["withdrawals"];
                // Withdrawals left unclaimed, or a failed escape, are errors.
                let left = r["withdrawals_error"].is_string()
                    || w["failed"].as_array().is_some_and(|f| !f.is_empty());
                let failed = r["escape_error"].as_str();
                if g.json {
                    print_json(&r)?;
                } else {
                    // Amounts a failed balance read left out are unknown, not 0.
                    let s = |k: &str| r[k].as_str().unwrap_or("(unknown)").to_string();
                    match r["escape"].as_str() {
                        Some("claimed") => println!(
                            "{who} escaped: its share of the last checkpoint paid {} (equity {}, expected {}).",
                            s("escape_paid"),
                            s("equity"),
                            s("expected")
                        ),
                        Some("skipped") => println!(
                            "{who}'s escape is left unclaimed: {}.",
                            r["note"].as_str().unwrap_or_default()
                        ),
                        Some("none") => println!("{who} has nothing to escape with: no balance in the lane's last checkpoint."),
                        Some("failed") => eprintln!(
                            "{who}'s escape failed: {}",
                            failed.unwrap_or_default()
                        ),
                        _ => println!("{who}'s escape was already claimed."),
                    }
                    for l in w["paid"].as_array().into_iter().flatten() {
                        println!(
                            "  claimed withdrawal {} from checkpoint {}",
                            l["amount"].as_str().unwrap_or_default(),
                            l["seq"]
                        );
                    }
                    if !no_withdrawals {
                        println!("  withdrawals paid {}; {} in all", s("withdrawals_paid"), s("paid"));
                    }
                    if left {
                        eprintln!("Some withdrawals are still unclaimed: `caravel claim {who}`.");
                    }
                    if let Some(e) = r["balance_error"].as_str() {
                        eprintln!("warning: the balance after couldn't be read ({e}): `caravel balance {who}`");
                    }
                }
                Ok(if left || failed.is_some() {
                    exit::ERROR
                } else {
                    exit::OK
                })
            })
        }
        Cmd::Replay {
            prove_escape,
            prove_withdrawals,
        } => replay(g, prove_escape.as_deref(), prove_withdrawals.as_deref()),
        Cmd::Wait {
            what,
            timeout,
            api_url,
        } => {
            use caravel_deploy::lifecycle::Wait;
            let w = match what {
                WaitCmd::Checkpoint { seq, signed, epoch } => {
                    Wait::Checkpoint { seq, signed, epoch }
                }
                WaitCmd::Api { path, until } => {
                    let (pointer, value) = match until.split_once('=') {
                        Some((p, v)) => (p.to_string(), Some(v.to_string())),
                        None => (until.clone(), None),
                    };
                    Wait::Api {
                        path,
                        pointer,
                        value,
                    }
                }
                WaitCmd::Healthy => Wait::Healthy,
                WaitCmd::Frozen => Wait::Frozen,
            };
            let ctx = context(g, None)?;
            runtime()?.block_on(async {
                let p = ctx.prepare(false).await?;
                let api = api_url.or_else(|| p.api_base(None).ok());
                let start = std::time::Instant::now();
                let ok = p
                    .wait(
                        &w,
                        std::time::Duration::from_secs(timeout),
                        std::time::Duration::from_secs(1),
                        api.as_deref(),
                    )
                    .await?;
                let secs = start.elapsed().as_secs();
                if g.json {
                    print_json(&json!({ "ok": ok, "seconds": secs }))?;
                } else if ok {
                    eprintln!("done in {secs} s");
                } else {
                    eprintln!("still waiting after {timeout} s");
                }
                Ok(if ok { exit::OK } else { exit::TIMEOUT })
            })
        }
        Cmd::Api {
            path,
            validator,
            api_url,
        } => {
            let ctx = context(g, None)?;
            runtime()?.block_on(async {
                let base = match api_url {
                    Some(u) => u,
                    None => {
                        let m = ctx.manifest()?;
                        match validator.as_deref() {
                            None => caravel_deploy::deploy::api_url(&m).ok_or_else(|| {
                                anyhow!("the host has no public_url: pass --api-url")
                            })?,
                            Some(v) => {
                                let i = m
                                    .env
                                    .validators
                                    .iter()
                                    .position(|x| {
                                        x.name == v || format!("validator-{}", x.name) == v
                                    })
                                    .ok_or_else(|| anyhow!("no validator {v:?}"))?;
                                caravel_deploy::deploy::validator_url(&m, i).ok_or_else(|| {
                                    anyhow!("the host has no public_url: pass --api-url")
                                })?
                            }
                        }
                    }
                };
                let path = if path.starts_with('/') {
                    path
                } else {
                    format!("/{path}")
                };
                print_json(&caravel_deploy::lifecycle::get(&base, &path).await?)?;
                Ok(exit::OK)
            })
        }
        Cmd::Render { lane, genesis } => {
            let ctx = context(g, lane.as_deref())?;
            let m = ctx.manifest()?;
            let doc = if genesis {
                toml::to_string(&m.lane.raw)?
            } else {
                let env = m
                    .lane
                    .env
                    .get(&ctx.env)
                    .cloned()
                    .ok_or_else(|| anyhow!("no [env.{}]", ctx.env))?;
                let mut t = toml::Table::new();
                t.insert(
                    "env".into(),
                    toml::Value::Table(toml::Table::from_iter([(ctx.env.clone(), env)])),
                );
                toml::to_string(&t)?
            };
            if g.json {
                let v: Value = serde_json::to_value(toml::from_str::<toml::Table>(&doc)?)?;
                print_json(
                    &json!({ "env": ctx.env, "vars": m.vars, "genesis": genesis, "document": v }),
                )?;
            } else {
                eprintln!("{}", ctx.describe());
                if !genesis && !m.vars.is_empty() {
                    println!("# vars: {}", m.vars);
                }
                print!("{doc}");
            }
            Ok(exit::OK)
        }
        Cmd::Env { cmd: EnvCmd::List } => env_list(g),
        Cmd::Output { name } => output(g, name.as_deref()),
        Cmd::Init {
            template,
            dir,
            name,
            port,
            prefix,
            force,
            list,
        } => {
            if list {
                return version(g);
            }
            let done = init::init(&init::InitArgs {
                template: template.as_deref(),
                dir: dir.as_deref(),
                name: name.as_deref(),
                port,
                prefix: prefix.as_deref(),
                force,
            })?;
            if g.json {
                print_json(&done.report)?;
            } else {
                let r = &done.report;
                println!(
                    "Wrote {} ({} lane {}, sequencer on port {}).",
                    r["lane_file"].as_str().unwrap_or_default(),
                    r["template"].as_str().unwrap_or_default(),
                    r["name"].as_str().unwrap_or_default(),
                    r["port"]
                );
                println!("Identities, in the Stellar CLI keystore:");
                for i in r["identities"].as_array().into_iter().flatten() {
                    println!(
                        "  {:<10} {:<28} {}{}",
                        i["role"].as_str().unwrap_or_default(),
                        i["identity"].as_str().unwrap_or_default(),
                        i["key"].as_str().unwrap_or_default(),
                        if i["created"] == true { "  (new)" } else { "" }
                    );
                }
                let cwd = std::env::current_dir()?;
                let dir = done.lane_file.parent().unwrap_or(&cwd);
                println!("\nNext:");
                if dir != cwd {
                    let shown = dir.strip_prefix(&cwd).unwrap_or(dir);
                    println!("  cd {}", shown.display());
                }
                println!("  caravel plan     # what apply would do");
                println!("  caravel apply    # a local Stellar network, the contracts, the nodes");
            }
            Ok(exit::OK)
        }
        Cmd::Keys { cmd } => {
            let ctx = context(g, None)?;
            let m = ctx.manifest()?;
            match cmd {
                KeysCmd::List => {
                    let r = init::keys_report(&m);
                    if g.json {
                        print_json(&r)?;
                    } else {
                        for k in r.as_array().into_iter().flatten() {
                            println!(
                                "{} {:<20} {:<28} {}",
                                if k["exists"] == true { "ok " } else { "-- " },
                                k["role"].as_str().unwrap_or_default(),
                                k["identity"].as_str().unwrap_or_default(),
                                k["key"].as_str().unwrap_or("(not in the keystore)")
                            );
                        }
                    }
                    Ok(exit::OK)
                }
                KeysCmd::Ensure => {
                    let created = init::ensure_identities(&m)?;
                    if g.json {
                        print_json(&json!({ "created": created }))?;
                    } else if created.is_empty() {
                        println!(
                            "The keystore has every identity [env.{}] names.",
                            m.env_name
                        );
                    } else {
                        for id in &created {
                            println!("+ identity  {id}");
                        }
                        if m.env.network != Network::Local {
                            println!("\napply funds the admin and the relayer with friendbot on testnet.");
                        }
                    }
                    Ok(exit::OK)
                }
                KeysCmd::Show { who } => {
                    let identity = init::roles(&m)
                        .into_iter()
                        .find(|(role, _)| *role == who)
                        .map(|(_, id)| id)
                        .unwrap_or(who);
                    let key = caravel_runtime::views::g_address(&Stellar::public_key(&identity)?);
                    if g.json {
                        print_json(&json!({ "identity": identity, "key": key }))?;
                    } else {
                        println!("{key}");
                    }
                    Ok(exit::OK)
                }
            }
        }
        Cmd::Version => version(g),
        Cmd::Doctor => doctor(g),
    }
}

/// `validate`: every deployment (or the one --env names), offline.
fn validate(g: &Global, positional: Option<&Path>) -> Result<u8> {
    let (lane_path, _) = lane_file(g, positional)?;
    let (ok, report) = validate_report(&lane_path, g.env.as_deref(), &g.inputs()?)?;
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
                    "identities missing from the Stellar CLI keystore: {} (create them with `caravel keys ensure`)",
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
pub fn validate_report(
    lane_path: &Path,
    env: Option<&str>,
    inputs: &Inputs,
) -> Result<(bool, Value)> {
    let lane = caravel_deploy::manifest::load_lane(lane_path)?;
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
        match Manifest::load_with(lane_path, name, inputs) {
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
    let lane = caravel_deploy::manifest::load_lane(&lane_path)?;
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
    let m = ctx.manifest()?;
    let keys = Keys::from_keystore(&m)?;
    let a = addresses(&m, &keys.admin)?;
    let genesis = ctx.template().and_then(|t| t.genesis(&m.lane)).ok();
    let attrs = caravel_deploy::attrs::attributes(&m, &keys, &a, genesis.as_ref(), None);
    let declared = m.outputs(&attrs)?;
    let validators: Vec<Value> = m
        .env
        .validators
        .iter()
        .zip(&keys.validators)
        .enumerate()
        .map(|(i, (v, k))| json!({ "name": v.name, "key": g_addr(k), "url": validator_url(&m, i) }))
        .collect();
    let mut all = json!({
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
    // The lane file's own [outputs], over the built-in ones.
    let mut sensitive = std::collections::BTreeSet::new();
    for o in &declared {
        all[&o.name] = caravel_deploy::attrs::json(&o.value);
        if o.sensitive {
            sensitive.insert(o.name.clone());
        }
    }
    let shown = |k: &str, v: &Value| -> Value {
        if sensitive.contains(k) {
            json!("(sensitive)")
        } else {
            v.clone()
        }
    };
    match name {
        None if g.json => {
            let masked: serde_json::Map<String, Value> = all
                .as_object()
                .expect("an object")
                .iter()
                .map(|(k, v)| (k.clone(), shown(k, v)))
                .collect();
            print_json(&Value::Object(masked))?
        }
        None => {
            for (k, v) in all.as_object().expect("an object") {
                match &shown(k, v) {
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
            let m = ctx.manifest();
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
                                "missing: {} (create them with `caravel keys ensure`)",
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

/// This caravel's version, as saved plans record it.
fn caravel_version() -> String {
    match option_env!("CARAVEL_COMMIT").filter(|c| !c.is_empty()) {
        Some(c) => format!("{} ({c})", env!("CARGO_PKG_VERSION")),
        None => env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// `apply plan.json`: the saved plan's lane file, deployment, release and
/// vars (unless given here), everything it was made from checked again, then
/// exactly its steps.
fn apply_saved(g: &Global, path: &Path, yes: bool) -> Result<u8> {
    use caravel_deploy::saved;
    let s = saved::read(path, &caravel_version())?;
    let mut g2 = g.clone();
    if g2.file.is_none() {
        g2.file = Some(s.context.lane_file.clone());
    }
    match &g2.env {
        Some(e) if *e != s.context.env => bail!(
            "{} is a plan for [env.{}], not [env.{e}]",
            path.display(),
            s.context.env
        ),
        _ => g2.env = Some(s.context.env.clone()),
    }
    if g2.release_dir.is_none() {
        g2.release_dir = s.context.release_dir.clone();
    }
    if g2.wasm_dir.is_none() {
        g2.wasm_dir = s.context.wasm_dir.clone();
    }
    // The plan's vars first; any given here come after, and the check below
    // says if they change one.
    let mut vars = s.var_args();
    vars.extend(g.vars.iter().cloned());
    g2.vars = vars;
    let ctx = context(&g2, None)?;
    let m = ctx.manifest()?;
    let missing = init::missing_identities(&m);
    if !missing.is_empty() {
        bail!(
            "the keystore lacks {}, which the plan's keys came from",
            missing.join(", ")
        );
    }
    runtime()?.block_on(async {
        let p = ctx.prepare(true).await?;
        let now = saved::Basis::of(&p)?;
        let plan = p.plan_with(&s.options)?;
        let why = s.moved(&now, &plan, &p.desired.host.release);
        if !why.is_empty() {
            let msg = format!(
                "{} no longer applies:\n  - {}\nPlan again: `caravel plan --out {}`",
                path.display(),
                why.join("\n  - "),
                path.display()
            );
            if g.json {
                // One document on stdout, the reason on stderr.
                print_json(&json!({ "ok": false, "applied": 0, "moved": why }))?;
                eprintln!("error: {msg}");
                return Ok(exit::ERROR);
            }
            bail!(msg);
        }
        if !g.json {
            print!("{}", plan.render(&p.desired));
        }
        if plan.steps.is_empty() {
            if g.json {
                print_json(&json!({ "ok": true, "applied": 0, "plan": plan.to_json(&p.desired) }))?;
            }
            return Ok(exit::OK);
        }
        if g.json && !yes {
            bail!("apply --json runs without a prompt: pass --yes");
        }
        if !deploy::confirm(plan.steps.len(), yes)? {
            println!("Nothing applied.");
            return Ok(exit::OK);
        }
        p.apply(&plan).await?;
        saved::mark_applied(path)?;
        let again = ctx.prepare(true).await?;
        let left = again.plan_with(&caravel_deploy::plan::Options {
            target: s.options.target.clone(),
            replace: vec![],
        })?;
        if g.json {
            print_json(&json!({
                "ok": left.is_empty(),
                "applied": plan.steps.len(),
                "plan": plan.to_json(&p.desired),
                "remaining": left.to_json(&again.desired),
            }))?;
        } else if left.is_empty() {
            println!("\nApplied the saved plan. The lane matches the lane file.");
        } else {
            print!(
                "\nApplied the saved plan, but the deployment still differs:\n{}",
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

/// `CARAVEL_YES=1` (or `true`): every command that asks first runs as with
/// `--yes`, for scripts and CI.
fn assume_yes() -> bool {
    std::env::var("CARAVEL_YES").is_ok_and(|v| v == "1" || v == "true")
}

/// The user flows of the context's deployment, with its exit file.
fn flows_for(ctx: &Ctx) -> Result<caravel_deploy::flows::Flows> {
    let m = ctx.manifest()?;
    let exit_file = ctx
        .state_root
        .join(".caravel")
        .join(&m.lane.lane.name)
        .join(&m.env_name)
        .join("exit.json");
    let mut f = caravel_deploy::flows::Flows::new(m)?;
    f.exit_file = Some(exit_file);
    Ok(f)
}

/// A withdrawal's result: JSON, or a line; exit 4 if it timed out.
fn user_result(g: &Global, r: &Value, what: &str) -> Result<u8> {
    let timed_out = r["timed_out"] == true;
    let claim_error = r["claim_error"].as_str();
    if g.json {
        print_json(r)?;
    } else {
        let leaf = &r["leaf"];
        if r["already_claimed"] == true {
            println!(
                "The {what} of {} in checkpoint {} (leaf {}) was already claimed on Stellar (a claim always pays the account's owner).",
                leaf["amount"].as_str().unwrap_or_default(),
                leaf["seq"],
                leaf["index"]
            );
        } else if let Some(e) = claim_error {
            eprintln!(
                "The {what} of {} is in checkpoint {} (leaf {}), but claiming it failed: {e}. `caravel claim` tries again.",
                leaf["amount"].as_str().unwrap_or_default(),
                leaf["seq"],
                leaf["index"]
            );
        } else if r["claimed"] == true {
            println!(
                "The {what} of {} is claimed on Stellar (checkpoint {}, leaf {}).",
                leaf["amount"].as_str().unwrap_or_default(),
                leaf["seq"],
                leaf["index"]
            );
        } else if !leaf.is_null() {
            println!(
                "The {what} of {} is in checkpoint {} (leaf {}): `caravel claim` pays it.",
                leaf["amount"].as_str().unwrap_or_default(),
                leaf["seq"],
                leaf["index"]
            );
        } else if timed_out {
            eprintln!(
                "The {what} is sent, but its leaf isn't in an accepted checkpoint yet{}; `caravel claim` pays it later.",
                r["note"].as_str().map(|n| format!(" ({n})")).unwrap_or_default()
            );
        } else if let Some(n) = r["note"].as_str() {
            println!("The {what} is done: {n}.");
        } else {
            println!("The {what} is sent.");
        }
    }
    Ok(if claim_error.is_some() {
        exit::ERROR
    } else if timed_out {
        exit::TIMEOUT
    } else {
        exit::OK
    })
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// An identity or a G… account, as a G… account.
fn account_of(who: &str) -> Result<String> {
    if who.len() == 56 && who.starts_with('G') {
        return Ok(who.to_string());
    }
    Ok(caravel_runtime::views::g_address(&Stellar::public_key(
        who,
    )?))
}

/// `replay`: the template's replay with the deployment's contract, network,
/// genesis and engine filled in.
fn replay(g: &Global, escape: Option<&str>, withdrawals: Option<&str>) -> Result<u8> {
    let ctx = context(g, None)?;
    let m = ctx.manifest()?;
    // Only the admin's public key: the settlement address derives from it.
    let admin = Stellar::admin_public_key(&m.env.admin)?;
    let a = addresses(&m, &admin)?;
    let template = ctx.template()?;
    let mut release = caravel_deploy::release::Release::locate(
        ctx.release.release_dir.as_deref(),
        &template.info.template,
    )?;
    if let Some(w) = &ctx.release.wasm_dir {
        release.use_wasm_from(w)?;
    }
    let engine = release.wasm_path(&template.info.engine_file);
    // The genesis document the nodes hash, next to the deployment's state.
    let dir = ctx
        .state_root
        .join(".caravel")
        .join(&m.lane.lane.name)
        .join(&m.env_name);
    std::fs::create_dir_all(&dir)?;
    let genesis = dir.join("genesis.toml");
    std::fs::write(&genesis, toml::to_string(&m.lane.raw)?)?;
    let mut cmd = std::process::Command::new(&template.path);
    cmd.arg("replay")
        .args(["--rpc", m.rpc_url()])
        .args(["--network-passphrase", m.env.network.passphrase()])
        .args(["--settlement", &c_addr(&a.settlement)])
        .arg("--genesis-config")
        .arg(&genesis)
        .arg("--engine-wasm")
        .arg(&engine);
    if let Some(w) = escape {
        cmd.args(["--prove-escape", &account_of(w)?]);
    }
    if let Some(w) = withdrawals {
        cmd.args(["--prove-withdrawals", &account_of(w)?]);
    }
    eprintln!("{}", ctx.describe());
    if !g.json {
        let status = cmd
            .status()
            .with_context(|| format!("running {}", template.path.display()))?;
        return Ok(if status.success() {
            exit::OK
        } else {
            exit::ERROR
        });
    }
    // --json: the replay's documents (report, then any proofs) as one.
    let out = cmd
        .stderr(std::process::Stdio::inherit())
        .output()
        .with_context(|| format!("running {}", template.path.display()))?;
    let docs: Vec<Value> = serde_json::Deserializer::from_slice(&out.stdout)
        .into_iter::<Value>()
        .collect::<std::result::Result<_, _>>()
        .context("the replay's output is not JSON")?;
    let mut doc = json!({ "report": docs.first().cloned().unwrap_or(Value::Null) });
    let mut rest = docs.into_iter().skip(1);
    if escape.is_some() {
        doc["escape"] = rest.next().unwrap_or(Value::Null);
    }
    if withdrawals.is_some() {
        doc["withdrawals"] = rest.next().unwrap_or(Value::Null);
    }
    print_json(&doc)?;
    Ok(if out.status.success() {
        exit::OK
    } else {
        exit::ERROR
    })
}

/// A G… account's key.
fn stellar_strkey(g: &str) -> Option<[u8; 32]> {
    caravel_node::lane_toml::parse_account(g).ok()
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
