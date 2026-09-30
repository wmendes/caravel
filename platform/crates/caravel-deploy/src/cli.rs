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
        /// A CI release artifact to install; defaults to this checkout's builds.
        #[arg(long)]
        release_dir: Option<PathBuf>,
    },
    /// Make Stellar and the host match the deployment. Shows the plan and
    /// asks first, unless --yes. Running it again changes nothing.
    Apply {
        lane: PathBuf,
        #[arg(long)]
        env: String,
        #[arg(long)]
        release_dir: Option<PathBuf>,
        /// Apply without asking.
        #[arg(long)]
        yes: bool,
    },
}

pub fn run<A: NodeApp>(app: A, cmd: Command) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        match cmd {
            Command::Plan {
                lane,
                env,
                release_dir,
            } => {
                let p = deploy::prepare(&app, &lane, &env, release_dir.as_deref(), false).await?;
                for n in &p.notes {
                    eprintln!("note: {n}");
                }
                print!("{}", p.plan().render(&p.desired));
                Ok(())
            }
            Command::Apply {
                lane,
                env,
                release_dir,
                yes,
            } => {
                let p = deploy::prepare(&app, &lane, &env, release_dir.as_deref(), true).await?;
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
                let again =
                    deploy::prepare(&app, &lane, &env, release_dir.as_deref(), true).await?;
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
        }
    })
}
