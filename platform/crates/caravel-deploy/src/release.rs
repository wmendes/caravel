//! What `apply` installs on a host: a template's node binary, the contracts,
//! the relayer and the template's feed modules. It comes from a CI release
//! artifact (`--release-dir`, the build of record, DEC-033) or from this
//! checkout's builds, for local lanes.

use std::collections::BTreeMap;
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
    /// The release's container images by role (`<template>-node`,
    /// `relayer`, `<template>-web`, `caddy`), from its `IMAGES` file
    /// (`scripts/build-images.sh`, DEC-111). Empty when it has none: the
    /// docker runtime then refuses it.
    pub images: BTreeMap<String, String>,
}

/// Parses an `IMAGES` file: one `<role> <image ref>` per line; blank lines
/// and `#` comments are skipped.
pub fn parse_images(text: &str) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let (Some(role), Some(image), None) = (parts.next(), parts.next(), parts.next()) else {
            bail!(
                "IMAGES line {}: expected \"<role> <image>\", got {line:?}",
                i + 1
            );
        };
        if out.insert(role.to_string(), image.to_string()).is_some() {
            bail!("IMAGES line {}: {role} is listed twice", i + 1);
        }
    }
    Ok(out)
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
    /// Where the release comes from: `release_dir` (a CI artifact or any
    /// assembled release), else the one installed next to this binary
    /// (`scripts/install.sh`: `<prefix>/share/caravel/current`), else this
    /// checkout's builds.
    pub fn locate(release_dir: Option<&Path>, template: &str) -> Result<Self> {
        match release_dir.map(Path::to_path_buf).or_else(installed_dir) {
            Some(d) => Self::from_dir(&d, template),
            None => Self::from_checkout(&find_repo()?, template),
        }
    }

    /// An assembled release (`scripts/assemble-release.sh`, CI's artifact):
    /// `bin/`, `contracts/`, `relayer/`, `relayer-feeds/<template>/`,
    /// `web/<template>/`, `COMMIT`.
    pub fn from_dir(dir: &Path, template: &str) -> Result<Self> {
        let commit = std::fs::read_to_string(dir.join("COMMIT"))
            .with_context(|| format!("{} is not a release (no COMMIT)", dir.display()))?
            .trim()
            .to_string();
        let feeds = dir.join("relayer-feeds").join(template);
        // Releases before M0.6 had one web app, the perps one, at `web/`.
        let web = [dir.join("web").join(template)]
            .into_iter()
            .chain((template == "perps").then(|| dir.join("web")))
            .find(|w| w.join("index.html").exists());
        let r = Self {
            node_binary: dir.join("bin").join(node_binary(template)),
            contracts: dir.join("contracts"),
            relayer: dir.join("relayer"),
            feeds: feeds.exists().then_some(feeds),
            web,
            commit: commit.chars().take(12).collect(),
            images: match std::fs::read_to_string(dir.join("IMAGES")) {
                Ok(text) => parse_images(&text)
                    .with_context(|| format!("reading {}", dir.join("IMAGES").display()))?,
                Err(_) => BTreeMap::new(),
            },
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
            images: match std::fs::read_to_string(repo.join("target/images/IMAGES")) {
                Ok(text) => parse_images(&text)?,
                Err(_) => BTreeMap::new(),
            },
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

    /// Takes the contracts from `dir` (the CI `contracts-wasm` artifact).
    /// The release's name then covers them too.
    pub fn use_wasm_from(&mut self, dir: &Path) -> Result<()> {
        if !dir.join("settlement.wasm").exists() {
            bail!("{} has no settlement.wasm", dir.display());
        }
        self.contracts = dir.to_path_buf();
        let mut all = hash_file(&self.node_binary)?.to_vec();
        for e in std::fs::read_dir(dir)?.flatten() {
            if e.path().extension().is_some_and(|x| x == "wasm") {
                all.extend(hash_file(&e.path())?);
            }
        }
        if self.commit.starts_with("local-") {
            let h = sha256(&all);
            self.commit = format!(
                "local-{}",
                h[..4]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
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

/// The release installed next to this binary: `<bin>/../share/caravel/current`.
pub fn installed_dir() -> Option<PathBuf> {
    let me = std::fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    installed_dir_for(me.parent()?)
}

/// The release installed for binaries in `bin`.
pub fn installed_dir_for(bin: &Path) -> Option<PathBuf> {
    let d = bin.parent()?.join("share/caravel/current");
    d.join("COMMIT").is_file().then_some(d)
}

/// What a binary runs on, in `uname -sm` form ("Linux x86_64", "Darwin
/// arm64"), read from its ELF or Mach-O header; `None` for anything else.
pub fn binary_platform(path: &Path) -> Result<Option<String>> {
    use std::io::Read;
    let mut head = [0u8; 20];
    let mut f = std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    if f.read(&mut head)? < head.len() {
        return Ok(None);
    }
    Ok(platform_of(&head))
}

fn platform_of(head: &[u8; 20]) -> Option<String> {
    if head[..4] == *b"\x7fELF" {
        // e_machine, little-endian on both targets we know.
        let arch = match u16::from_le_bytes([head[18], head[19]]) {
            0x3E => "x86_64",
            0xB7 => "aarch64",
            _ => return None,
        };
        return Some(format!("Linux {arch}"));
    }
    if head[..4] == [0xCF, 0xFA, 0xED, 0xFE] {
        let arch = match u32::from_le_bytes([head[4], head[5], head[6], head[7]]) {
            0x0100_0007 => "x86_64",
            0x0100_000C => "arm64",
            _ => return None,
        };
        return Some(format!("Darwin {arch}"));
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platforms() {
        let mut elf = [0u8; 20];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[18] = 0x3E;
        assert_eq!(platform_of(&elf).as_deref(), Some("Linux x86_64"));
        elf[18] = 0xB7;
        assert_eq!(platform_of(&elf).as_deref(), Some("Linux aarch64"));
        let mut macho = [0u8; 20];
        macho[..8].copy_from_slice(&[0xCF, 0xFA, 0xED, 0xFE, 0x0C, 0x00, 0x00, 0x01]);
        assert_eq!(platform_of(&macho).as_deref(), Some("Darwin arm64"));
        macho[4] = 0x07;
        assert_eq!(platform_of(&macho).as_deref(), Some("Darwin x86_64"));
        assert_eq!(platform_of(&[0; 20]), None);
        // This test binary is one of them.
        let me = binary_platform(&std::env::current_exe().unwrap()).unwrap();
        let want = format!(
            "{} {}",
            if cfg!(target_os = "macos") {
                "Darwin"
            } else {
                "Linux"
            },
            if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
                "arm64"
            } else {
                std::env::consts::ARCH
            }
        );
        assert_eq!(me.as_deref(), Some(want.as_str()));
    }

    #[test]
    fn a_release_is_found_next_to_the_binary_and_has_a_web_dir_per_template() {
        let t = tempfile::tempdir().unwrap();
        let bin = t.path().join("bin");
        let rel = t.path().join("share/caravel/v1");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(rel.join("bin")).unwrap();
        assert_eq!(installed_dir_for(&bin), None);
        std::fs::write(rel.join("COMMIT"), "0123456789abcdef\n").unwrap();
        std::os::unix::fs::symlink("v1", t.path().join("share/caravel/current")).unwrap();
        let found = installed_dir_for(&bin).unwrap();
        assert!(found.ends_with("share/caravel/current"));
        for d in [
            "contracts",
            "relayer/dist",
            "relayer/node_modules",
            "web/perps",
        ] {
            std::fs::create_dir_all(rel.join(d)).unwrap();
        }
        for f in [
            "bin/caravel-perps-node",
            "bin/caravel-payments-node",
            "relayer/dist/main.js",
            "web/perps/index.html",
        ] {
            std::fs::write(rel.join(f), "").unwrap();
        }
        let perps = Release::from_dir(&found, "perps").unwrap();
        assert_eq!(perps.commit, "0123456789ab");
        assert!(perps.web.unwrap().ends_with("web/perps"));
        // A payments lane doesn't get the perps web app.
        assert!(Release::from_dir(&found, "payments").unwrap().web.is_none());
        // A release from before M0.6 had the perps web app at web/.
        std::fs::remove_dir_all(rel.join("web/perps")).unwrap();
        std::fs::write(rel.join("web/index.html"), "").unwrap();
        assert!(Release::from_dir(&found, "perps")
            .unwrap()
            .web
            .unwrap()
            .ends_with("web"));
        assert!(Release::from_dir(&found, "payments").unwrap().web.is_none());
    }
}
