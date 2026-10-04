//! `caravel init` and `caravel keys` (M0.6, DEC-077): a lane file from a
//! template's example, and the Stellar CLI identities a deployment names.
//! Identities stay in the Stellar CLI's keystore; no secret is written
//! anywhere else.

use std::collections::BTreeMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use caravel_deploy::manifest::{envs, Manifest};
use caravel_deploy::stellar::Cli as Stellar;
use caravel_deploy::template::{self, Plugin, Template};
use caravel_node::lane_toml::LaneFile;
use caravel_node::plugin::{example_roles, fill_example};
use serde_json::{json, Value};

/// A lane name from a directory's name: lowercase letters, digits and
/// dashes, at most 48 characters.
pub fn lane_name(raw: &str) -> Result<String> {
    let mut out = String::new();
    for c in raw.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        bail!("{raw:?} gives no lane name: pass --name");
    }
    if out.len() > 48 {
        bail!("the lane name {out:?} is longer than 48 characters: pass --name");
    }
    Ok(out)
}

/// The first port from `from` (in steps of 10) where `width` ports in a row
/// are free on 127.0.0.1: the sequencer's, then each validator's.
pub fn free_port(from: u16, width: u16) -> Result<u16> {
    let free = |p: u16| TcpListener::bind(("127.0.0.1", p)).is_ok();
    let mut p = from;
    while p.checked_add(width).is_some() && p < from.saturating_add(2000) {
        if (p..p + width).all(free) {
            return Ok(p);
        }
        p += 10;
    }
    bail!("no {width} free ports in a row from {from}: pass --port")
}

/// The roles a deployment's identities play, with their names, in a stable
/// order: admin, relayer, each validator, each feed key, each declared
/// account.
pub fn roles(m: &Manifest) -> Vec<(String, String)> {
    let mut out = vec![
        ("admin".to_string(), m.env.admin.clone()),
        ("relayer".to_string(), m.env.relayer.account.clone()),
    ];
    if let Some(k) = &m.env.sequencer.key {
        out.push(("sequencer".to_string(), k.clone()));
    }
    for v in &m.env.validators {
        let role = match v.url {
            Some(_) => format!("validator-{} (run elsewhere)", v.name),
            None => format!("validator-{}", v.name),
        };
        out.push((role, v.key.clone()));
    }
    for (var, id) in &m.env.relayer.feed_keys {
        out.push((format!("feed {var}"), id.clone()));
    }
    for (name, a) in &m.env.accounts {
        out.push((format!("account {name}"), a.identity_of(name).to_string()));
    }
    out
}

pub struct InitArgs<'a> {
    pub template: Option<&'a str>,
    pub dir: Option<&'a Path>,
    pub name: Option<&'a str>,
    pub port: Option<u16>,
    pub prefix: Option<&'a str>,
    pub force: bool,
    /// `docker` or `process`; None picks docker when the release has images.
    pub runtime: Option<&'a str>,
}

/// The example's local host with `runtime = "docker"` (D-03).
fn with_docker(text: &str) -> String {
    text.replacen("[env.local.host]\n", "[env.local.host]\nruntime = \"docker\"                         # containers from the release's images (caravel init --runtime)\n", 1)
}

/// What `init` did.
pub struct Init {
    pub lane_file: PathBuf,
    pub report: Value,
}

/// Writes `<dir>/lane.toml` from the template's example and creates the
/// identities it names that the keystore lacks.
pub fn init(a: &InitArgs) -> Result<Init> {
    let installed: Vec<Plugin> = template::installed()
        .into_iter()
        .filter_map(|(_, p)| p.ok())
        .collect();
    let names: Vec<String> = installed.iter().map(|p| p.info.template.clone()).collect();
    let plugin = match a.template {
        Some(t) => Plugin::locate(t).map_err(|e| {
            anyhow!(
                "{e:#} (installed: {})",
                if names.is_empty() {
                    "none".into()
                } else {
                    names.join(", ")
                }
            )
        })?,
        None => match installed.as_slice() {
            [only] => only.clone(),
            [] => bail!(
                "no template is installed (caravel-<template>-node next to caravel or on PATH)"
            ),
            _ => bail!("which template? caravel init <{}> [dir]", names.join("|")),
        },
    };
    let cwd = std::env::current_dir()?;
    let dir = match a.dir {
        Some(d) if d.is_relative() => cwd.join(d),
        Some(d) => d.to_path_buf(),
        None => cwd.clone(),
    };
    let lane_file = dir.join("lane.toml");
    if lane_file.exists() && !a.force {
        bail!("{} exists; pass --force to replace it", lane_file.display());
    }
    let name = match a.name {
        Some(n) => lane_name(n)?,
        None => {
            let base = std::path::absolute(&dir)?
                .file_name()
                .and_then(|n| n.to_str())
                .map(String::from)
                .ok_or_else(|| anyhow!("{} has no name: pass --name", dir.display()))?;
            lane_name(&base)?
        }
    };
    let prefix = a
        .prefix
        .map(lane_name)
        .transpose()?
        .unwrap_or_else(|| name.clone());
    let example = plugin.example()?;
    let roles = example_roles(&example)?;

    // The identities first: genesis may hold their keys.
    let mut ids = BTreeMap::new();
    let mut identities = Vec::new();
    for role in &roles {
        let identity = format!("{prefix}-{role}");
        let (key, created) = if Stellar::has_identity(&identity) {
            (Stellar::public_key(&identity)?, false)
        } else {
            (Stellar::generate_identity(&identity)?, true)
        };
        let g = caravel_runtime::views::g_address(&key);
        identities
            .push(json!({ "role": role, "identity": identity, "key": g, "created": created }));
        ids.insert(role.clone(), (identity, g));
    }
    let port = match a.port {
        Some(p) => p,
        None => free_port(18080, 10)?,
    };
    let text = fill_example(&example, &name, port, &ids)?;
    // Containers when the installed release ships this template's images.
    let docker = match a.runtime {
        Some(r) => r == "docker",
        None => {
            caravel_deploy::release::Release::locate(None, &plugin.info.template).is_ok_and(|r| {
                r.images
                    .contains_key(&format!("{}-node", plugin.info.template))
            })
        }
    };
    let text = if docker { with_docker(&text) } else { text };

    // What's written must be a lane file the template and the tool accept.
    let lane = LaneFile::parse(&text).context("the template's example")?;
    plugin.genesis(&lane).context("the template's example")?;
    for e in envs(&lane) {
        Manifest::from_lane(lane.clone(), &e.name).context("the template's example")?;
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(&lane_file, &text)?;
    let ignore = dir.join(".gitignore");
    let have = std::fs::read_to_string(&ignore).unwrap_or_default();
    if !have
        .lines()
        .any(|l| l.trim() == ".caravel/" || l.trim() == ".caravel")
    {
        let sep = if have.is_empty() || have.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        std::fs::write(
            &ignore,
            format!(
                "{have}{sep}# caravel's local state: logs, PIDs, keys for local nodes\n.caravel/\n"
            ),
        )?;
    }
    let report = json!({
        "runtime": if docker { "docker" } else { "process" },
        "lane_file": lane_file.display().to_string(),
        "template": plugin.info.template,
        "name": name,
        "port": port,
        "envs": envs(&lane).iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
        "identities": identities,
    });
    Ok(Init { lane_file, report })
}

/// `keys list`: each role's identity, whether the keystore has it, its key.
pub fn keys_report(m: &Manifest) -> Value {
    json!(roles(m)
        .into_iter()
        .map(|(role, identity)| {
            let key = Stellar::public_key(&identity)
                .ok()
                .map(|k| caravel_runtime::views::g_address(&k));
            json!({ "role": role, "identity": identity, "exists": key.is_some(), "key": key })
        })
        .collect::<Vec<_>>())
}

/// Creates the identities a deployment names that the keystore lacks; the
/// ones created.
pub fn ensure_identities(m: &Manifest) -> Result<Vec<String>> {
    let mut created = Vec::new();
    for (role, identity) in roles(m) {
        if !created.contains(&identity) && !Stellar::has_identity(&identity) {
            // Its operator holds the secret: never made up here.
            if role.ends_with("(run elsewhere)") {
                bail!("{role} needs identity {identity:?} from its public key: `stellar keys add {identity} --public-key G…`");
            }
            Stellar::generate_identity(&identity)?;
            created.push(identity);
        }
    }
    Ok(created)
}

/// The identities a deployment names that the keystore lacks.
pub fn missing_identities(m: &Manifest) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, identity) in roles(m) {
        if !out.contains(&identity) && !Stellar::has_identity(&identity) {
            out.push(identity);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lane_names() {
        assert_eq!(lane_name("acme-pay").unwrap(), "acme-pay");
        assert_eq!(lane_name("Acme Pay_2").unwrap(), "acme-pay-2");
        assert_eq!(lane_name("--my  lane--").unwrap(), "my-lane");
        assert!(lane_name("___").is_err());
        assert!(lane_name(&"a".repeat(49)).is_err());
    }

    #[test]
    fn free_ports() {
        let p = free_port(18500, 4).unwrap();
        let _held = TcpListener::bind(("127.0.0.1", p + 2)).unwrap();
        let q = free_port(p, 4).unwrap();
        assert!(q >= p + 10 && (q - p).is_multiple_of(10));
    }
}
