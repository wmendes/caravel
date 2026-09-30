//! What `apply` installs on a host: a template's node binary, the contracts,
//! the relayer and the template's feed modules. It comes from a CI release
//! artifact (`--release-dir`, the build of record, DEC-033) or from this
//! checkout's builds, for local lanes.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use caravel_runtime::checkpoint::sha256;

use crate::plan::Key;

pub struct Release {
    pub node_binary: PathBuf,
    pub contracts: PathBuf,
    /// The relayer: `dist/`, `package.json` and `node_modules/`.
    pub relayer: PathBuf,
    /// The template's relayer feed modules, if it has any.
    pub feeds: Option<PathBuf>,
    /// The template's web app, served at the host's public URL, if it has one.
    pub web: Option<PathBuf>,
    pub commit: String,
}

/// `caravel-<template>-node`.
pub fn node_binary(template: &str) -> String {
    format!("caravel-{template}-node")
}

fn hash_file(p: &Path) -> Result<Key> {
    Ok(sha256(
        &std::fs::read(p).with_context(|| format!("reading {}", p.display()))?,
    ))
}

impl Release {
    /// A CI release artifact (`bin/`, `contracts/`, `relayer/`, `relayer-feeds/<template>/`, `COMMIT`).
    pub fn from_dir(dir: &Path, template: &str) -> Result<Self> {
        let commit = std::fs::read_to_string(dir.join("COMMIT"))
            .with_context(|| format!("{} is not a release (no COMMIT)", dir.display()))?
            .trim()
            .to_string();
        let feeds = dir.join("relayer-feeds").join(template);
        let web = dir.join("web");
        let r = Self {
            node_binary: dir.join("bin").join(node_binary(template)),
            contracts: dir.join("contracts"),
            relayer: dir.join("relayer"),
            feeds: feeds.exists().then_some(feeds),
            web: web.join("index.html").exists().then_some(web),
            commit: commit.chars().take(12).collect(),
        };
        r.check()?;
        Ok(r)
    }

    /// This checkout's builds. The commit names the exact bytes:
    /// `local-` and the start of H(node binary ‖ Wasm hashes).
    pub fn from_checkout(repo: &Path, template: &str) -> Result<Self> {
        let feeds = repo.join("lanes").join(template).join("relayer-feeds");
        let web = repo.join("lanes").join(template).join("web/dist");
        let mut r = Self {
            node_binary: repo.join("target/release").join(node_binary(template)),
            contracts: repo.join("target/contracts"),
            relayer: repo.join("platform/relayer"),
            feeds: feeds.join("dist").exists().then_some(feeds),
            web: web.join("index.html").exists().then_some(web),
            commit: String::new(),
        };
        r.check().context("build first: ./scripts/build-contracts.sh, cargo build --release, and npm ci + npm run build in platform/relayer (and the template's relayer-feeds)")?;
        let mut all = hash_file(&r.node_binary)?.to_vec();
        for e in std::fs::read_dir(&r.contracts)?.flatten() {
            if e.path().extension().is_some_and(|x| x == "wasm") {
                all.extend(hash_file(&e.path())?);
            }
        }
        let h = sha256(&all);
        r.commit = format!(
            "local-{}",
            h[..4]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        Ok(r)
    }

    fn check(&self) -> Result<()> {
        for (what, p) in [
            ("node binary", self.node_binary.clone()),
            ("contracts", self.contracts.clone()),
            ("relayer build", self.relayer.join("dist/main.js")),
            ("relayer dependencies", self.relayer.join("node_modules")),
        ] {
            if !p.exists() {
                bail!("the release has no {what} ({})", p.display());
            }
        }
        Ok(())
    }

    /// The hash of a contract in the release.
    pub fn wasm_hash(&self, file: &str) -> Result<Key> {
        hash_file(&self.contracts.join(file))
    }

    pub fn wasm_path(&self, file: &str) -> PathBuf {
        self.contracts.join(file)
    }
}

/// The checkout: the first directory, up from the current one or else up
/// from this binary's, that has `versions.json` and `platform/`.
pub fn find_repo() -> Result<PathBuf> {
    let is_repo = |d: &Path| d.join("versions.json").exists() && d.join("platform").is_dir();
    let exe = std::env::current_exe()
        .ok()
        .and_then(|e| std::fs::canonicalize(e).ok());
    for start in [std::env::current_dir().ok(), exe].into_iter().flatten() {
        let mut dir = start;
        loop {
            if is_repo(&dir) {
                return Ok(dir);
            }
            if !dir.pop() {
                break;
            }
        }
    }
    bail!("no Caravel checkout here or around this binary; pass --release-dir <CI release>")
}
