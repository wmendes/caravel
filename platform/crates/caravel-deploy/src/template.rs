//! What the deploy tool needs from a lane's template: its name, the token
//! decimals it requires, and the genesis hashes of a lane file. Either the
//! app linked in ([`InProcess`], the template binaries' own deploy commands),
//! or the template's binary driven through its plugin protocol ([`Plugin`],
//! `caravel_node::plugin`), which is how the `caravel` CLI runs any template.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};
use caravel_node::app::NodeApp;
use caravel_node::lane_toml::{self, LaneFile};
use caravel_node::plugin::{Body, Info, PROTOCOL};
use caravel_runtime::checkpoint::sha256;

use crate::plan::Key;

/// A lane file's genesis, as Stellar and the nodes check it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenesisHashes {
    pub lane_id: Key,
    pub config_hash: Key,
    pub genesis_state_hash: Key,
}

pub trait Template {
    fn name(&self) -> &str;
    /// The decimals the template needs of the settlement token, if any.
    fn token_decimals(&self) -> Option<u32>;
    fn genesis(&self, lane: &LaneFile) -> Result<GenesisHashes>;
}

/// The app, linked into this binary.
pub struct InProcess<A: NodeApp>(pub A);

impl<A: NodeApp> Template for InProcess<A> {
    fn name(&self) -> &str {
        A::TEMPLATE
    }

    fn token_decimals(&self) -> Option<u32> {
        self.0.token_decimals()
    }

    fn genesis(&self, lane: &LaneFile) -> Result<GenesisHashes> {
        let (_, config, state) = lane_toml::genesis(&self.0, lane)?;
        Ok(GenesisHashes {
            lane_id: lane.lane_id(),
            config_hash: sha256(&config),
            genesis_state_hash: sha256(&state),
        })
    }
}

/// A template's binary, `caravel-<template>-node`, through `plugin …`.
#[derive(Clone, Debug)]
pub struct Plugin {
    pub path: PathBuf,
    pub info: Info,
}

/// `caravel-<template>-node`.
pub fn binary_name(template: &str) -> String {
    format!("caravel-{template}-node")
}

impl Plugin {
    /// Finds the template's binary: in `CARAVEL_PLUGIN_DIR`, next to this
    /// binary (a `stellar-caravel` link resolved), then on PATH.
    pub fn locate(template: &str) -> Result<Self> {
        let name = binary_name(template);
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(d) = std::env::var_os("CARAVEL_PLUGIN_DIR") {
            dirs.push(d.into());
        }
        if let Some(d) = std::env::current_exe()
            .ok()
            .and_then(|me| std::fs::canonicalize(me).ok())
            .and_then(|me| me.parent().map(Path::to_path_buf))
        {
            dirs.push(d);
        }
        if let Some(path) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&path));
        }
        let found = dirs
            .iter()
            .map(|d| d.join(&name))
            .find(|p| p.is_file())
            .ok_or_else(|| {
                anyhow!("no {name} next to caravel or on PATH: install the {template} template")
            })?;
        let p = Self::at(&found)?;
        if p.info.template != template {
            bail!(
                "{} is the {} template, not {template}",
                found.display(),
                p.info.template
            );
        }
        Ok(p)
    }

    /// The template binary at `path`, after checking it speaks this protocol.
    pub fn at(path: &Path) -> Result<Self> {
        let out = run(path, &["plugin", "info"], None)?;
        let info: Info = serde_json::from_slice(&out)
            .with_context(|| format!("{} plugin info", path.display()))?;
        if info.protocol != PROTOCOL {
            bail!(
                "{} speaks plugin protocol {}, and this caravel {PROTOCOL}: install matching versions",
                path.display(),
                info.protocol
            );
        }
        Ok(Self {
            path: path.to_path_buf(),
            info,
        })
    }

    /// The lane file `caravel init` starts from.
    pub fn example(&self) -> Result<String> {
        String::from_utf8(run(&self.path, &["plugin", "example"], None)?)
            .context("plugin example is not UTF-8")
    }

    /// A transaction body's kind and bytes, from the template's `tx` syntax.
    pub fn body(&self, args: &[String], decimals: Option<u32>) -> Result<(u8, Vec<u8>)> {
        let mut a: Vec<String> = vec!["plugin".into(), "body".into()];
        if let Some(d) = decimals {
            a.extend(["--decimals".into(), d.to_string()]);
        }
        a.extend(args.iter().cloned());
        let a: Vec<&str> = a.iter().map(String::as_str).collect();
        let b: Body = serde_json::from_slice(&run(&self.path, &a, None)?)?;
        let bytes = caravel_runtime::sequencer::unhex(&b.body)
            .ok_or_else(|| anyhow!("plugin body: not hex"))?;
        Ok((b.kind, bytes))
    }
}

impl Template for Plugin {
    fn name(&self) -> &str {
        &self.info.template
    }

    fn token_decimals(&self) -> Option<u32> {
        self.info.token_decimals
    }

    /// The template's `genesis`, over the genesis document the hosts get
    /// (`lane.toml`), on stdin.
    fn genesis(&self, lane: &LaneFile) -> Result<GenesisHashes> {
        let doc = toml::to_string(&lane.raw)?;
        let out = run(&self.path, &["genesis", "--config", "-"], Some(&doc))?;
        let r: lane_toml::GenesisReport = serde_json::from_slice(&out).context("genesis report")?;
        let key = |h: &str, what: &str| -> Result<Key> {
            caravel_runtime::sequencer::unhex(h)
                .and_then(|v| v.try_into().ok())
                .ok_or_else(|| anyhow!("genesis {what} {h:?}"))
        };
        let hashes = GenesisHashes {
            lane_id: key(&r.lane_id, "lane_id")?,
            config_hash: key(&r.config_hash, "config_hash")?,
            genesis_state_hash: key(&r.genesis_state_hash, "genesis_state_hash")?,
        };
        if hashes.lane_id != lane.lane_id() {
            bail!("{} reports another lane id", self.path.display());
        }
        Ok(hashes)
    }
}

/// Runs the binary; its stdout, or its stderr as the error.
fn run(bin: &Path, args: &[&str], stdin: Option<&str>) -> Result<Vec<u8>> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("running {}", bin.display()))?;
    if let Some(text) = stdin {
        child
            .stdin
            .take()
            .expect("piped")
            .write_all(text.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "{} {}: {}",
            bin.display(),
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}
