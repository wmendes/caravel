//! Which lane file and which deployment a command is about, without being
//! told: the lane file is `-f`, `CARAVEL_FILE`, `./lane.toml`, the one
//! `lane*.toml` here, or the nearest `lane.toml` above (up to the
//! repository's root or the home directory). The deployment is `--env`,
//! `CARAVEL_ENV`, the one marked `default = true`, or the only one. Nothing
//! is remembered between runs, so an apply can't land on a deployment the
//! command line didn't name or the file doesn't mark.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use caravel_deploy::manifest::{envs, EnvInfo};
use caravel_node::lane_toml::LaneFile;

/// How the lane file was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileFrom {
    Flag,
    EnvVar,
    Here,
    Above,
}

/// Finds the lane file. `explicit` is `-f` (or the positional), `var` is
/// `CARAVEL_FILE`, `home` stops the walk up.
pub fn find_lane_file(
    explicit: Option<&Path>,
    var: Option<&Path>,
    cwd: &Path,
    home: Option<&Path>,
) -> Result<(PathBuf, FileFrom)> {
    let named = |p: &Path, from: FileFrom| -> Result<(PathBuf, FileFrom)> {
        let p = if p.is_relative() {
            cwd.join(p)
        } else {
            p.to_path_buf()
        };
        let p = if p.is_dir() { p.join("lane.toml") } else { p };
        if !p.is_file() {
            bail!("no lane file at {}", p.display());
        }
        Ok((p, from))
    };
    if let Some(p) = explicit {
        return named(p, FileFrom::Flag);
    }
    if let Some(p) = var {
        return named(p, FileFrom::EnvVar);
    }
    if cwd.join("lane.toml").is_file() {
        return Ok((cwd.join("lane.toml"), FileFrom::Here));
    }
    let mut here: Vec<PathBuf> = std::fs::read_dir(cwd)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                    n.starts_with("lane") && n.ends_with(".toml") && n != "lane.toml"
                })
        })
        .collect();
    here.sort();
    match here.len() {
        1 => return Ok((here.remove(0), FileFrom::Here)),
        0 => {}
        _ => {
            let names: Vec<_> = here
                .iter()
                .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
                .collect();
            bail!(
                "several lane files here ({}): pick one with -f",
                names.join(", ")
            );
        }
    }
    let mut dir = cwd.to_path_buf();
    loop {
        if dir.join(".git").exists() || Some(dir.as_path()) == home || !dir.pop() {
            break;
        }
        if dir.join("lane.toml").is_file() {
            return Ok((dir.join("lane.toml"), FileFrom::Above));
        }
    }
    bail!("no lane file here or above: run `caravel init`, or name one with -f")
}

/// How the deployment was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvFrom {
    Flag,
    EnvVar,
    Default,
    Only,
}

impl EnvFrom {
    pub fn describe(&self) -> &'static str {
        match self {
            Self::Flag => "--env",
            Self::EnvVar => "CARAVEL_ENV",
            Self::Default => "default = true",
            Self::Only => "the only deployment",
        }
    }
}

/// Chooses the deployment. `explicit` is `--env`, `var` is `CARAVEL_ENV`.
pub fn choose_env(
    lane: &LaneFile,
    explicit: Option<&str>,
    var: Option<&str>,
) -> Result<(String, EnvFrom)> {
    let all = envs(lane);
    let list = |all: &[EnvInfo]| {
        all.iter()
            .map(|e| format!("[env.{}]", e.name))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let named = |name: &str, from: EnvFrom| -> Result<(String, EnvFrom)> {
        if all.iter().any(|e| e.name == name) {
            Ok((name.to_string(), from))
        } else if all.is_empty() {
            Err(anyhow!(
                "the lane file has no deployments; add an [env.{name}] table"
            ))
        } else {
            Err(anyhow!("no [env.{name}]; the file has {}", list(&all)))
        }
    };
    if let Some(e) = explicit {
        return named(e, EnvFrom::Flag);
    }
    if let Some(e) = var.filter(|e| !e.is_empty()) {
        return named(e, EnvFrom::EnvVar);
    }
    let defaults: Vec<_> = all.iter().filter(|e| e.default).collect();
    match (defaults.as_slice(), all.as_slice()) {
        ([d], _) => Ok((d.name.clone(), EnvFrom::Default)),
        ([_, _, ..], _) => bail!(
            "several deployments say default = true ({}); keep one",
            defaults
                .iter()
                .map(|e| format!("[env.{}]", e.name))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ([], [only]) => Ok((only.name.clone(), EnvFrom::Only)),
        ([], []) => bail!("the lane file has no deployments; add an [env.<name>] table"),
        ([], _) => bail!(
            "which deployment? pass --env (the file has {}), or mark one default = true",
            list(&all)
        ),
    }
}

/// Where a lane's local state lives (`.caravel/<lane>/<env>/`): next to its
/// lane file. A deployment started from another directory by the old
/// commands, whose state is under the current directory, is still found.
pub fn state_root(
    lane_path: &Path,
    cwd: &Path,
    lane: &str,
    env: &str,
) -> (PathBuf, Option<String>) {
    let by_file = lane_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| cwd.to_path_buf());
    let at = |root: &Path| root.join(".caravel").join(lane).join(env);
    if !at(&by_file).exists() && at(cwd).exists() && by_file != cwd {
        let note = format!(
            "using this deployment's state under {} (it was started from here); new deployments keep it next to the lane file",
            cwd.join(".caravel").display()
        );
        return (cwd.to_path_buf(), Some(note));
    }
    (by_file, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane(envs: &str) -> LaneFile {
        let text = format!(
            "[lane]\nname = \"t\"\n[node]\nblock_time_ms = 1000\ncheckpoint_every_blocks = 10\nmax_batch_bytes = 96000\n{envs}"
        );
        LaneFile::parse(&text).unwrap()
    }

    #[test]
    fn the_env_is_chosen() {
        let two = lane("[env.local]\nnetwork = \"local\"\n[env.testnet]\nnetwork = \"testnet\"\n");
        assert_eq!(
            choose_env(&two, Some("testnet"), Some("local")).unwrap(),
            ("testnet".into(), EnvFrom::Flag)
        );
        assert_eq!(
            choose_env(&two, None, Some("local")).unwrap(),
            ("local".into(), EnvFrom::EnvVar)
        );
        let e = choose_env(&two, None, None).unwrap_err().to_string();
        assert!(e.contains("[env.local], [env.testnet]"), "{e}");
        assert!(choose_env(&two, Some("prod"), None)
            .unwrap_err()
            .to_string()
            .contains("no [env.prod]"));
        let marked = lane("[env.local]\ndefault = true\n[env.testnet]\nnetwork = \"testnet\"\n");
        assert_eq!(
            choose_env(&marked, None, None).unwrap(),
            ("local".into(), EnvFrom::Default)
        );
        assert_eq!(
            choose_env(&marked, None, Some("")).unwrap(),
            ("local".into(), EnvFrom::Default)
        );
        let one = lane("[env.e2e]\nnetwork = \"local\"\n");
        assert_eq!(
            choose_env(&one, None, None).unwrap(),
            ("e2e".into(), EnvFrom::Only)
        );
        let both = lane("[env.a]\ndefault = true\n[env.b]\ndefault = true\n");
        assert!(choose_env(&both, None, None).is_err());
        assert!(choose_env(&lane(""), None, None).is_err());
    }

    #[test]
    fn the_lane_file_is_found() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
        std::fs::create_dir_all(root.join("repo/lanes/a/sub")).unwrap();
        std::fs::create_dir_all(root.join("repo/lanes/b")).unwrap();
        std::fs::write(root.join("repo/lanes/a/lane.toml"), "").unwrap();
        std::fs::write(root.join("repo/lanes/b/lane.x.toml"), "").unwrap();
        let a = root.join("repo/lanes/a");
        let b = root.join("repo/lanes/b");
        assert_eq!(
            find_lane_file(None, None, &a, None).unwrap(),
            (a.join("lane.toml"), FileFrom::Here)
        );
        assert_eq!(
            find_lane_file(None, None, &a.join("sub"), None).unwrap(),
            (a.join("lane.toml"), FileFrom::Above)
        );
        assert_eq!(
            find_lane_file(None, None, &b, None).unwrap(),
            (b.join("lane.x.toml"), FileFrom::Here)
        );
        std::fs::write(b.join("lane.y.toml"), "").unwrap();
        let e = find_lane_file(None, None, &b, None)
            .unwrap_err()
            .to_string();
        assert!(e.contains("lane.x.toml, lane.y.toml"), "{e}");
        // -f wins, a directory means its lane.toml, and CARAVEL_FILE comes next.
        assert_eq!(
            find_lane_file(
                Some(Path::new("../a")),
                Some(&b.join("lane.x.toml")),
                &b,
                None
            )
            .unwrap(),
            (b.join("../a").join("lane.toml"), FileFrom::Flag)
        );
        assert_eq!(
            find_lane_file(None, Some(&b.join("lane.x.toml")), &a, None).unwrap(),
            (b.join("lane.x.toml"), FileFrom::EnvVar)
        );
        assert!(find_lane_file(Some(Path::new("nope.toml")), None, &a, None).is_err());
        // The walk stops at the repository's root and at home.
        std::fs::write(root.join("lane.toml"), "").unwrap();
        let empty = root.join("repo/lanes");
        assert!(find_lane_file(None, None, &empty, None).is_err());
        std::fs::create_dir_all(root.join("home/me/work")).unwrap();
        std::fs::write(root.join("home/lane.toml"), "").unwrap();
        assert!(find_lane_file(
            None,
            None,
            &root.join("home/me/work"),
            Some(&root.join("home/me"))
        )
        .is_err());
    }

    #[test]
    fn local_state_is_next_to_the_lane_file() {
        let t = tempfile::tempdir().unwrap();
        let lanes = t.path().join("lanes");
        std::fs::create_dir_all(&lanes).unwrap();
        let file = lanes.join("lane.toml");
        assert_eq!(
            state_root(&file, t.path(), "pay", "local"),
            (lanes.clone(), None)
        );
        std::fs::create_dir_all(t.path().join(".caravel/pay/local")).unwrap();
        let (root, note) = state_root(&file, t.path(), "pay", "local");
        assert_eq!(root, t.path());
        assert!(note.is_some());
        std::fs::create_dir_all(lanes.join(".caravel/pay/local")).unwrap();
        assert_eq!(state_root(&file, t.path(), "pay", "local"), (lanes, None));
    }
}
