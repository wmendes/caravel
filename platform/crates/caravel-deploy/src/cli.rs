//! The deploy commands every app's binary has, next to the node commands
//! (`caravel_node::cli`). An app's binary flattens [`Command`] into its CLI:
//!
//! ```ignore
//! #[derive(clap::Subcommand)]
//! enum Cmd {
//!     #[command(flatten)]
//!     Node(caravel_node::cli::Command),
//!     #[command(flatten)]
//!     Deploy(caravel_deploy::cli::Command),
//! }
//! ```

use std::path::PathBuf;

use anyhow::Result;
use caravel_node::app::NodeApp;
use clap::Subcommand;

use crate::deploy;
use crate::ops::DestroyOptions;
use crate::template::InProcess;

/// Where `apply` takes the release from.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct ReleaseArgs {
    /// A CI release artifact to install; defaults to this checkout's builds.
    #[arg(long)]
    pub release_dir: Option<PathBuf>,
    /// Take the Wasm from here instead (the CI `contracts-wasm` artifact, the
    /// builds of record), with this checkout's binaries.
    #[arg(long)]
    pub wasm_dir: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Show what `apply` would change, on Stellar and on the host, for one of
    /// the lane file's deployments (`[env.<name>]`). Changes nothing.
    Plan {
        /// The lane file.
        lane: PathBuf,
        /// The deployment, `[env.<name>]`.
        #[arg(long)]
        env: String,
        #[command(flatten)]
        release: ReleaseArgs,
        /// Also show each file it would write as a diff against the host's.
        #[arg(long)]
        diff: bool,
    },
    /// Make Stellar and the host match the deployment. Shows the plan and
    /// asks first, unless --yes. Running it again changes nothing.
    Apply {
        lane: PathBuf,
        #[arg(long)]
        env: String,
        #[command(flatten)]
        release: ReleaseArgs,
        /// Apply without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Show a deployment's health: height, the last accepted checkpoint, when
    /// a freeze would be possible, the relayer's XLM, TTL horizons, and
    /// whether it matches the lane file.
    Status {
        lane: PathBuf,
        #[arg(long)]
        env: String,
        #[command(flatten)]
        release: ReleaseArgs,
        /// Print JSON, for scripts.
        #[arg(long)]
        json: bool,
    },
    /// Wind a lane down for good: drain, stop the sequencer and relayer,
    /// export every exit with its proof to exit.json, then freeze once the
    /// contract allows it. A frozen lane can't be restarted.
    Destroy {
        lane: PathBuf,
        #[arg(long)]
        env: String,
        #[command(flatten)]
        release: ReleaseArgs,
        /// Don't ask (the lane can't be restarted afterwards).
        #[arg(long)]
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
}

pub fn run<A: NodeApp>(app: A, cmd: Command) -> Result<()> {
    let app = InProcess(app);
    // These commands keep a local lane's processes under the current
    // directory's `.caravel/`, as they always have.
    let cwd = std::env::current_dir()?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        match cmd {
            Command::Plan {
                lane,
                env,
                release,
                diff,
            } => {
                let p = deploy::prepare(&app, &lane, &env, &release, false, &cwd).await?;
                for n in &p.notes {
                    eprintln!("note: {n}");
                }
                let plan = p.plan();
                print!("{}", plan.render(&p.desired));
                if diff {
                    print!("\n{}", p.file_diffs(&plan)?);
                }
                Ok(())
            }
            Command::Apply {
                lane,
                env,
                release,
                yes,
            } => {
                let p = deploy::prepare(&app, &lane, &env, &release, true, &cwd).await?;
                for n in &p.notes {
                    eprintln!("note: {n}");
                }
                let plan = p.plan();
                print!("{}", plan.render(&p.desired));
                if plan.steps.is_empty() || !plan.problems.is_empty() {
                    if !plan.problems.is_empty() {
                        anyhow::bail!("nothing applied");
                    }
                    return Ok(());
                }
                if !deploy::confirm(plan.steps.len(), yes)? {
                    println!("Nothing applied.");
                    return Ok(());
                }
                p.apply(&plan).await?;
                // Read everything back: a finished apply leaves nothing to do.
                let again = deploy::prepare(&app, &lane, &env, &release, true, &cwd).await?;
                let left = again.plan();
                if left.is_empty() {
                    println!("\nApplied. The lane matches the lane file.");
                } else {
                    print!(
                        "\nApplied, but the deployment still differs:\n{}",
                        left.render(&again.desired)
                    );
                }
                Ok(())
            }
            Command::Status {
                lane,
                env,
                release,
                json,
            } => {
                let p = deploy::prepare(&app, &lane, &env, &release, false, &cwd).await?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&p.status_json().await)?);
                } else {
                    print!("{}", p.render_status().await);
                }
                Ok(())
            }
            Command::Destroy {
                lane,
                env,
                release,
                yes,
                no_wait,
                stop_validators,
                pay_out,
                wipe,
            } => {
                let p = deploy::prepare(&app, &lane, &env, &release, true, &cwd).await?;
                print!("{}", p.render_status().await);
                if !yes && !deploy::confirm_destroy(&p.desired.lane_name)? {
                    println!("Nothing done.");
                    return Ok(());
                }
                p.destroy(&DestroyOptions {
                    no_wait,
                    stop_validators,
                    pay_out,
                    wipe,
                })
                .await
            }
        }
    })
}
