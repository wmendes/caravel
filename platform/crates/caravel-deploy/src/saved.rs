//! Saved plans (M0.6, C-17, DEC-089): `caravel plan --out plan.json`, then
//! `caravel apply plan.json` applies exactly that plan, or refuses with the
//! reason when anything it was made from moved.
//!
//! A saved plan records what the plan was computed from, as hashes:
//! - every file the lane file loaded (itself and its includes) and every
//!   `--var-file`;
//! - each var's value (a sensitive one only as a hash, salted with the lane
//!   and the var's name);
//! - `--target` and `--replace`;
//! - the deployment the lane file resolved to (`desired`), and what Stellar
//!   (`chain`) and the host (`host`) had, each as a digest;
//! - the caravel version that made it (digests are only comparable within
//!   one version).
//!
//! It never holds a secret: no key, no file's content, no sensitive value.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

use crate::deploy::Prepared;
use crate::plan::{Options, Plan};

pub const FORMAT: &str = "caravel-saved-plan/1";

fn sha256_hex(bytes: &[u8]) -> String {
    caravel_runtime::checkpoint::sha256(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A digest of a value's `Debug` form: deterministic for the plan's inputs
/// (ordered maps and sets), within one caravel version.
pub fn digest<T: std::fmt::Debug>(v: &T) -> String {
    sha256_hex(format!("{v:?}").as_bytes())
}

/// What a plan was computed from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Basis {
    /// The lane file and its includes: path → sha256 of the text.
    pub files: BTreeMap<String, String>,
    /// `--var-file`s: path → sha256.
    pub var_files: BTreeMap<String, String>,
    /// Each var: its value as TOML, or `sha256:<hex>` when sensitive.
    pub vars: BTreeMap<String, String>,
    pub desired: String,
    pub chain: String,
    pub host: String,
}

/// A var's value as `--var` reads it back: a string as written, anything
/// else as a TOML value.
fn var_raw(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// How a saved plan records a var: its value as `--var` reads it, or, when
/// sensitive, a hash salted with the lane and the var's name.
fn var_entry(lane_id: &[u8; 32], name: &str, v: &toml::Value, sensitive: bool) -> String {
    let raw = var_raw(v);
    if !sensitive {
        return raw;
    }
    let salted = [lane_id.as_slice(), name.as_bytes(), b"\0", raw.as_bytes()].concat();
    format!("sha256:{}", sha256_hex(&salted))
}

impl Basis {
    pub fn of(p: &Prepared) -> Result<Self> {
        let mut files = BTreeMap::new();
        let mut vars = BTreeMap::new();
        if let Some(doc) = &p.m.doc {
            for s in &doc.sources.0 {
                files.insert(s.path.display().to_string(), sha256_hex(s.text.as_bytes()));
            }
            let (values, sensitive) = doc.resolve_vars(&p.m.inputs).map_err(|e| anyhow!("{e}"))?;
            for (name, v) in values {
                let shown = var_entry(&p.desired.lane_id, &name, &v, sensitive.contains(&name));
                vars.insert(name, shown);
            }
        }
        let mut var_files = BTreeMap::new();
        for f in &p.m.inputs.var_files {
            let text = std::fs::read(f).with_context(|| format!("reading {}", f.display()))?;
            var_files.insert(f.display().to_string(), sha256_hex(&text));
        }
        Ok(Self {
            files,
            var_files,
            vars,
            desired: digest(&p.desired),
            chain: digest(&p.chain),
            host: digest(&p.host),
        })
    }

    fn json(&self) -> Value {
        json!({
            "files": self.files,
            "var_files": self.var_files,
            "vars": self.vars,
            "desired": self.desired,
            "chain": self.chain,
            "host": self.host,
        })
    }

    fn from_json(v: &Value) -> Result<Self> {
        let map = |k: &str| -> Result<BTreeMap<String, String>> {
            serde_json::from_value(v[k].clone()).with_context(|| format!("the saved plan's {k}"))
        };
        let s = |k: &str| -> Result<String> {
            v[k].as_str()
                .map(String::from)
                .ok_or_else(|| anyhow!("the saved plan has no {k}"))
        };
        Ok(Self {
            files: map("files")?,
            var_files: map("var_files")?,
            vars: map("vars")?,
            desired: s("desired")?,
            chain: s("chain")?,
            host: s("host")?,
        })
    }
}

/// Where a saved plan's command ran: what `apply` needs to run it the same
/// way.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Origin {
    pub lane_file: PathBuf,
    pub env: String,
    pub release_dir: Option<PathBuf>,
    pub wasm_dir: Option<PathBuf>,
}

/// A plan read back from its file.
#[derive(Clone, Debug)]
pub struct Saved {
    pub version: String,
    pub context: Origin,
    pub options: Options,
    pub basis: Basis,
    /// The plan's lines, as `plan` printed them (each step's line and
    /// address).
    pub steps: Vec<(String, String)>,
    pub doc: Value,
}

/// The saved plan's document.
pub fn save(
    p: &Prepared,
    plan: &Plan,
    opts: &Options,
    ctx: &Origin,
    version: &str,
) -> Result<Value> {
    if !plan.problems.is_empty() {
        bail!("the plan has problems, so there is nothing to save: fix them first");
    }
    let basis = Basis::of(p)?;
    Ok(json!({
        "format": FORMAT,
        "caravel": version,
        "lane_file": ctx.lane_file,
        "env": ctx.env,
        "release_dir": ctx.release_dir,
        "wasm_dir": ctx.wasm_dir,
        "release": p.desired.host.release,
        "options": { "target": opts.target, "replace": opts.replace },
        "basis": basis.json(),
        "plan": plan.to_json(&p.desired),
    }))
}

/// Whether `text` is a saved plan (rather than a lane file).
pub fn is_saved_plan(text: &str) -> bool {
    serde_json::from_str::<Value>(text).is_ok_and(|v| v["format"] == FORMAT)
}

/// Reads a saved plan, checking only its format and version.
pub fn read(path: &Path, version: &str) -> Result<Saved> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let doc: Value =
        serde_json::from_str(&text).with_context(|| format!("{} is not JSON", path.display()))?;
    if doc["format"] != FORMAT {
        bail!(
            "{} is not a saved plan ({FORMAT}): make one with `caravel plan --out FILE`",
            path.display()
        );
    }
    if doc["applied"] == true {
        bail!(
            "{} was applied already: plan again (`caravel plan --out FILE`)",
            path.display()
        );
    }
    let made_by = doc["caravel"].as_str().unwrap_or("?").to_string();
    if made_by != version {
        bail!(
            "{} was made by caravel {made_by}, and this is {version}: plan again with this one",
            path.display()
        );
    }
    let list = |v: &Value| -> Vec<String> {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s.as_str().map(String::from))
            .collect()
    };
    let path_of = |k: &str| doc[k].as_str().map(PathBuf::from);
    Ok(Saved {
        version: made_by,
        context: Origin {
            lane_file: path_of("lane_file")
                .ok_or_else(|| anyhow!("the saved plan has no lane_file"))?,
            env: doc["env"].as_str().unwrap_or_default().to_string(),
            release_dir: path_of("release_dir"),
            wasm_dir: path_of("wasm_dir"),
        },
        options: Options {
            target: list(&doc["options"]["target"]),
            replace: list(&doc["options"]["replace"]),
        },
        basis: Basis::from_json(&doc["basis"])?,
        steps: doc["plan"]["steps"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| {
                (
                    s["line"].as_str().unwrap_or_default().to_string(),
                    s["address"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect(),
        doc,
    })
}

/// Marks the plan at `path` applied, so it isn't applied twice (a replacement
/// would run again).
pub fn mark_applied(path: &Path) -> Result<()> {
    let mut doc: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    doc["applied"] = json!(true);
    std::fs::write(path, serde_json::to_string_pretty(&doc)? + "\n")?;
    Ok(())
}

impl Saved {
    /// The `--var` values that give the plan's vars back: every var that
    /// isn't sensitive, by name (a sensitive one comes from where it came
    /// from before: `CARAVEL_VAR_<name>`, `--var` or a var file).
    pub fn var_args(&self) -> Vec<String> {
        self.basis
            .vars
            .iter()
            .filter(|(_, v)| !v.starts_with("sha256:"))
            .map(|(k, v)| format!("{k}={v}"))
            .collect()
    }

    /// Why this plan can't be applied any more, if it can't: each thing that
    /// moved since it was made. Empty when everything is as it was.
    pub fn moved(&self, now: &Basis, plan_now: &Plan, release_now: &str) -> Vec<String> {
        let mut why = Vec::new();
        let was = &self.basis;
        let files = |a: &BTreeMap<String, String>, b: &BTreeMap<String, String>, what: &str| {
            let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
            keys.into_iter()
                .filter_map(|k| match (a.get(k), b.get(k)) {
                    (Some(x), Some(y)) if x == y => None,
                    (Some(_), Some(_)) => Some(format!("{what} {k} changed")),
                    (Some(_), None) => Some(format!("{what} {k} is no longer read")),
                    (None, _) => Some(format!("{what} {k} is new")),
                })
                .collect::<Vec<_>>()
        };
        why.extend(files(&was.files, &now.files, "the lane file"));
        why.extend(files(&was.var_files, &now.var_files, "the var file"));
        let names: BTreeSet<&String> = was.vars.keys().chain(now.vars.keys()).collect();
        for k in names {
            match (was.vars.get(k), now.vars.get(k)) {
                (Some(a), Some(b)) if a == b => {}
                (Some(a), _) if a.starts_with("sha256:") => why.push(format!(
                    "the sensitive var {k} has another value (give it as when planning: CARAVEL_VAR_{k}, --var or a var file)"
                )),
                (Some(a), Some(b)) => why.push(format!("var {k} was {a}, now {b}")),
                (Some(_), None) => why.push(format!("var {k} is gone")),
                (None, _) => why.push(format!("var {k} is new")),
            }
        }
        if was.desired != now.desired {
            let release = self.doc["release"].as_str();
            why.push(match release {
                Some(r) if r != release_now => {
                    format!("the release is {release_now}, the plan's was {r}")
                }
                _ => "the deployment the lane file resolves to changed (a setting, a key, an address or the release)".into(),
            });
        }
        if was.chain != now.chain {
            why.push("Stellar changed since the plan (accounts, the token, or the settlement contract's state)".into());
        }
        if was.host != now.host {
            why.push("the host changed since the plan (its release, files or nodes)".into());
        }
        let lines: Vec<(String, String)> = plan_now
            .steps
            .iter()
            .map(|s| (crate::plan::step_line(s), crate::plan::step_addr(s)))
            .collect();
        if why.is_empty() && lines != self.steps {
            why.push("the plan computed now has other steps".into());
        }
        if why.is_empty() && !plan_now.problems.is_empty() {
            why.push("the plan computed now has problems: run `caravel plan`".into());
        }
        why
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Step;

    fn basis() -> Basis {
        let m = |kv: &[(&str, &str)]| {
            kv.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        Basis {
            files: m(&[("/l/lane.toml", "aa"), ("/l/envs.toml", "bb")]),
            var_files: m(&[]),
            vars: m(&[("validators", "[\"1\", \"2\"]"), ("host", "sha256:cc")]),
            desired: "d".into(),
            chain: "c".into(),
            host: "h".into(),
        }
    }

    fn saved(steps: &[Step]) -> Saved {
        Saved {
            version: "0.6.0".into(),
            context: Origin::default(),
            options: Options::default(),
            basis: basis(),
            steps: steps
                .iter()
                .map(|s| (crate::plan::step_line(s), crate::plan::step_addr(s)))
                .collect(),
            doc: json!({ "release": "0a1b2c3d" }),
        }
    }

    fn plan(steps: &[Step]) -> Plan {
        Plan {
            steps: steps.to_vec(),
            ..Plan::default()
        }
    }

    #[test]
    fn nothing_moved_means_it_applies() {
        let steps = [Step::Restart {
            node: "sequencer".into(),
        }];
        assert!(saved(&steps)
            .moved(&basis(), &plan(&steps), "0a1b2c3d")
            .is_empty());
    }

    #[test]
    fn each_thing_that_moved_is_named() {
        let steps = [Step::Restart {
            node: "sequencer".into(),
        }];
        let s = saved(&steps);
        let moved = |f: &dyn Fn(&mut Basis)| {
            let mut b = basis();
            f(&mut b);
            s.moved(&b, &plan(&steps), "0a1b2c3d")
        };
        assert_eq!(
            moved(&|b| {
                b.files.insert("/l/envs.toml".into(), "xx".into());
            }),
            ["the lane file /l/envs.toml changed"]
        );
        assert_eq!(
            moved(&|b| {
                b.vars.insert("validators".into(), "[\"1\", \"4\"]".into());
            }),
            ["var validators was [\"1\", \"2\"], now [\"1\", \"4\"]"]
        );
        assert!(moved(&|b| {
            b.vars.insert("host".into(), "sha256:dd".into());
        })[0]
            .starts_with("the sensitive var host has another value"));
        assert!(moved(&|b| b.chain = "c2".into())[0].starts_with("Stellar changed"));
        assert!(moved(&|b| b.host = "h2".into())[0].starts_with("the host changed"));
        let mut b = basis();
        b.desired = "d2".into();
        assert_eq!(
            s.moved(&b, &plan(&steps), "99887766"),
            ["the release is 99887766, the plan's was 0a1b2c3d"]
        );
        // The same inputs, other steps: still refused.
        let other = [Step::Start {
            node: "relayer".into(),
        }];
        assert_eq!(
            s.moved(&basis(), &plan(&other), "0a1b2c3d"),
            ["the plan computed now has other steps"]
        );
    }

    #[test]
    fn a_sensitive_var_is_kept_only_as_a_salted_hash() {
        let v = toml::Value::String("hunter2".into());
        let a = var_entry(&[1; 32], "host", &v, true);
        assert!(a.starts_with("sha256:") && !a.contains("hunter2"));
        assert_ne!(
            a,
            var_entry(&[2; 32], "host", &v, true),
            "salted by the lane"
        );
        assert_ne!(a, var_entry(&[1; 32], "other", &v, true), "and by the name");
        assert_eq!(var_entry(&[1; 32], "host", &v, false), "hunter2");
        let list = toml::Value::Array(vec!["1".into(), "2".into()]);
        assert_eq!(
            var_entry(&[1; 32], "validators", &list, false),
            "[\"1\", \"2\"]"
        );
        // --var gives back the ones that aren't sensitive.
        assert_eq!(saved(&[]).var_args(), ["validators=[\"1\", \"2\"]"]);
    }

    #[test]
    fn a_file_from_another_version_or_format_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plan.json");
        let doc = json!({
            "format": FORMAT, "caravel": "0.5.0", "lane_file": "/l/lane.toml", "env": "local",
            "options": { "target": [], "replace": [] },
            "basis": { "files": {}, "var_files": {}, "vars": {}, "desired": "d", "chain": "c", "host": "h" },
            "plan": { "steps": [] },
        });
        std::fs::write(&path, doc.to_string()).unwrap();
        assert!(is_saved_plan(&doc.to_string()));
        assert!(!is_saved_plan("[lane]\nname = \"x\"\n"));
        let err = read(&path, "0.6.0").unwrap_err().to_string();
        assert!(
            err.contains("made by caravel 0.5.0, and this is 0.6.0"),
            "{err}"
        );
        assert_eq!(read(&path, "0.5.0").unwrap().context.env, "local");
        mark_applied(&path).unwrap();
        assert!(read(&path, "0.5.0")
            .unwrap_err()
            .to_string()
            .contains("was applied already"));
        std::fs::write(&path, "{}").unwrap();
        assert!(read(&path, "0.5.0")
            .unwrap_err()
            .to_string()
            .contains("not a saved plan"));
    }
}
