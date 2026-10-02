//! The lane file language (M0.6, spec §20.5, DEC-078). A lane file is TOML:
//! its genesis sections (`[lane]`, `[app]`, `[node]`, `[access]`,
//! `[limits]`, the template's) say what the lane is, and its `[env.<name>]`
//! tables say where and how it runs. On top of plain TOML, the deployments
//! can be composed:
//!
//! - `include = ["envs.toml", …]` (top level, before any table) loads more
//!   deployment tables from files next to this one. An included file holds
//!   only `[env.*]` tables and its own `include`; a name is defined once.
//! - `extends = "base"` or `["a", "b"]` in an `[env.<name>]` starts it from
//!   other deployments, later ones winning: tables merge key by key, and
//!   anything else (a value, an array, an array of tables) is replaced.
//! - `abstract = true` marks a deployment that is only there to be extended.
//!
//! [`LaneDoc::to_table`] gives the plain lane file every other part of
//! Caravel reads (the genesis sections, and each deployment fully merged),
//! and every value keeps where it came from for error messages.
//!
//! Genesis sections are consensus config: the lane file language never
//! changes them, and they are read from the lane file itself only.

pub mod diag;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub use diag::{did_you_mean, Diagnostic, Error, Source, Sources, Span};

/// Top-level keys that belong to deployments, not genesis. Lane file
/// parsers set them aside, and no template may use them as its section.
pub const DEPLOYMENT_KEYS: [&str; 5] = ["env", "include", "vars", "locals", "outputs"];

/// Reserved for the expression layer, which comes later (M0.6, C-07).
const LATER: [&str; 3] = ["vars", "locals", "outputs"];

/// Where a value of a deployment came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub span: Span,
    /// The deployment it was inherited from, if not its own.
    pub via: Option<String>,
}

/// A lane file with its includes and inheritance resolved.
#[derive(Clone, Debug)]
pub struct LaneDoc {
    pub sources: Sources,
    /// The genesis sections, as written.
    pub genesis: toml::Table,
    /// Every deployment a command can run, fully merged, by name.
    pub envs: BTreeMap<String, toml::Table>,
    /// Deployments that are only there to be extended.
    pub abstract_envs: BTreeSet<String>,
    /// `env.<name>.<path>` → where the value came from.
    origins: BTreeMap<String, Origin>,
}

impl LaneDoc {
    pub fn load(path: &Path) -> Result<Self, Error> {
        Self::load_with(path, &|p: &Path| std::fs::read_to_string(p))
    }

    /// Like [`LaneDoc::load`], reading files through `read` (tests).
    pub fn load_with(
        path: &Path,
        read: &dyn Fn(&Path) -> std::io::Result<String>,
    ) -> Result<Self, Error> {
        let mut l = Loader {
            read,
            sources: Sources::default(),
            diags: Vec::new(),
            stack: Vec::new(),
            envs: BTreeMap::new(),
            origins: BTreeMap::new(),
        };
        let genesis = l.file(path, None).unwrap_or_default();
        l.finish(genesis)
    }

    /// A lane file from text, with no includes (there is no file to find them from).
    pub fn parse(text: &str) -> Result<Self, Error> {
        let text = text.to_string();
        Self::load_with(Path::new("lane.toml"), &move |p: &Path| {
            if p == Path::new("lane.toml") {
                Ok(text.clone())
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "a lane file given as text can't include files",
                ))
            }
        })
    }

    /// The lane file as the tool and the nodes read it: the genesis sections
    /// and `[env.<name>]` for each deployment, merged.
    pub fn to_table(&self) -> toml::Table {
        let mut t = self.genesis.clone();
        if !self.envs.is_empty() {
            let envs: toml::Table = self
                .envs
                .iter()
                .map(|(k, v)| (k.clone(), toml::Value::Table(v.clone())))
                .collect();
            t.insert("env".into(), toml::Value::Table(envs));
        }
        t
    }

    /// Where `env.<name>.<path>` came from, e.g. `env.testnet.validators[1].key`.
    pub fn origin(&self, path: &str) -> Option<&Origin> {
        self.origins.get(path)
    }

    /// `file:line:col` of a value, for messages that aren't diagnostics.
    pub fn locate(&self, path: &str) -> Option<String> {
        let o = self.origin(path)?;
        let (line, col) = self.sources.line_col(o.span.file, o.span.range.start);
        let via = o
            .via
            .as_ref()
            .map(|v| format!(" (from [env.{v}])"))
            .unwrap_or_default();
        Some(format!(
            "{}:{line}:{col}{via}",
            self.sources.0[o.span.file].path.display()
        ))
    }
}

/// One `[env.<name>]` as written, before inheritance.
struct RawEnv {
    table: toml::Table,
    file: usize,
}

struct Loader<'a> {
    read: &'a dyn Fn(&Path) -> std::io::Result<String>,
    sources: Sources,
    diags: Vec<Diagnostic>,
    /// The includes being loaded, to catch a cycle.
    stack: Vec<PathBuf>,
    envs: BTreeMap<String, RawEnv>,
    /// Per file, `path` → its span, and whether it is a table (tables
    /// merge; anything else replaces what it inherits).
    origins: BTreeMap<(usize, String), (Span, bool)>,
}

impl Loader<'_> {
    fn span(&self, file: usize, path: &str) -> Option<Span> {
        self.origins
            .get(&(file, path.to_string()))
            .map(|(s, _)| s.clone())
    }

    /// Loads one file. `from` is the including file; the main file's genesis
    /// sections are returned.
    fn file(&mut self, path: &Path, from: Option<(usize, String)>) -> Option<toml::Table> {
        let key = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(at) = self.stack.iter().position(|p| *p == key) {
            let chain: Vec<String> = self.stack[at..]
                .iter()
                .chain(std::iter::once(&key))
                .map(|p| p.display().to_string())
                .collect();
            let span = from.and_then(|(f, p)| self.span(f, &p));
            self.diags.push(
                Diagnostic::new(format!(
                    "the includes go round in a circle: {}",
                    chain.join(" → ")
                ))
                .at(span),
            );
            return None;
        }
        let text = match (self.read)(path) {
            Ok(t) => t,
            Err(e) => {
                let span = from.as_ref().and_then(|(f, p)| self.span(*f, p));
                self.diags
                    .push(Diagnostic::new(format!("can't read {}: {e}", path.display())).at(span));
                return None;
            }
        };
        let file = self.sources.add(path.to_path_buf(), text.clone());
        let mut table: toml::Table = match toml::from_str(&text) {
            Ok(t) => t,
            Err(e) => {
                self.diags.push(
                    Diagnostic::new(e.message().to_string())
                        .at(e.span().map(|range| Span { file, range })),
                );
                return None;
            }
        };
        if let Ok(spanned) = toml::de::DeTable::parse(&text) {
            let mut out = Vec::new();
            walk_table(spanned.get_ref(), "", &mut out);
            for (p, range, is_table) in out {
                self.origins
                    .insert((file, p), (Span { file, range }, is_table));
            }
        }
        let is_main = from.is_none();
        self.stack.push(key);

        for k in LATER {
            if table.contains_key(k) {
                self.diags.push(
                    Diagnostic::new(format!("`{k}` is reserved for the lane file's expressions, which this version doesn't have yet"))
                        .at(self.span(file, k)),
                );
            }
        }
        // Includes first, so a name defined twice points at both.
        if let Some(inc) = table.remove("include") {
            let list: Vec<(usize, String)> = match &inc {
                toml::Value::String(s) => vec![(0, s.clone())],
                toml::Value::Array(a) if a.iter().all(|v| v.is_str()) => a
                    .iter()
                    .enumerate()
                    .map(|(i, v)| (i, v.as_str().unwrap_or_default().to_string()))
                    .collect(),
                _ => {
                    self.diags.push(
                        Diagnostic::new(
                            "`include` is a file, or a list of files, next to this one",
                        )
                        .at(self.span(file, "include")),
                    );
                    Vec::new()
                }
            };
            let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            for (i, rel) in list {
                let at = if inc.is_str() {
                    "include".to_string()
                } else {
                    format!("include[{i}]")
                };
                if Path::new(&rel).is_absolute() {
                    self.diags.push(
                        Diagnostic::new(format!("include {rel:?}: name it relative to this file"))
                            .at(self.span(file, &at)),
                    );
                    continue;
                }
                self.file(&dir.join(&rel), Some((file, at)));
            }
        }
        match table.remove("env") {
            None => {}
            Some(toml::Value::Table(envs)) => {
                for (name, v) in envs {
                    let here = format!("env.{name}");
                    let toml::Value::Table(t) = v else {
                        self.diags.push(
                            Diagnostic::new(format!("[env.{name}] must be a table"))
                                .at(self.span(file, &here)),
                        );
                        continue;
                    };
                    if let Some(prev) = self.envs.get(&name) {
                        let (line, _) = self.sources.line_col(
                            prev.file,
                            self.span(prev.file, &here).map_or(0, |s| s.range.start),
                        );
                        let note = format!(
                            "it is also defined in {}:{line}",
                            self.sources.0[prev.file].path.display()
                        );
                        self.diags.push(
                            Diagnostic::new(format!("[env.{name}] is defined twice"))
                                .at(self.span(file, &here))
                                .note(note)
                                .help(
                                    "give one of them another name, or make one extend the other",
                                ),
                        );
                        continue;
                    }
                    self.envs.insert(name, RawEnv { table: t, file });
                }
            }
            Some(_) => self.diags.push(
                Diagnostic::new("`env` must be a table of deployments, [env.<name>]")
                    .at(self.span(file, "env")),
            ),
        }
        for k in LATER {
            table.remove(k);
        }
        self.stack.pop();
        if is_main {
            Some(table)
        } else {
            for k in table.keys() {
                self.diags.push(
                    Diagnostic::new(format!(
                        "[{k}] is a genesis section: it belongs in the lane file itself, not in an included file"
                    ))
                    .at(self.span(file, k)),
                );
            }
            None
        }
    }

    fn finish(mut self, genesis: toml::Table) -> Result<LaneDoc, Error> {
        let mut envs = BTreeMap::new();
        let mut abstract_envs = BTreeSet::new();
        let mut origins = BTreeMap::new();
        let names: Vec<String> = self.envs.keys().cloned().collect();
        for name in &names {
            if let Some((table, o)) = self.resolve(name, &mut Vec::new()) {
                let raw = &self.envs[name].table;
                let is_abstract = raw.get("abstract").and_then(|v| v.as_bool()) == Some(true);
                for (p, origin) in o {
                    origins.insert(format!("env.{name}{p}"), origin);
                }
                if is_abstract {
                    if raw.get("default").and_then(|v| v.as_bool()) == Some(true) {
                        let span = self.span(self.envs[name].file, &format!("env.{name}.default"));
                        self.diags.push(
                            Diagnostic::new(format!(
                                "[env.{name}] is abstract, so it can't be the default deployment"
                            ))
                            .at(span),
                        );
                    }
                    abstract_envs.insert(name.clone());
                } else {
                    envs.insert(name.clone(), table);
                }
            }
        }
        if !self.diags.is_empty() {
            return Err(Error {
                sources: self.sources,
                diagnostics: self.diags,
            });
        }
        Ok(LaneDoc {
            sources: self.sources,
            genesis,
            envs,
            abstract_envs,
            origins,
        })
    }

    /// A deployment with what it extends merged in, and where each of its
    /// values came from (paths relative to the deployment, e.g. `.host.provider`).
    fn resolve(
        &mut self,
        name: &str,
        chain: &mut Vec<String>,
    ) -> Option<(toml::Table, BTreeMap<String, Origin>)> {
        if chain.iter().any(|c| c == name) {
            chain.push(name.to_string());
            let file = self.envs[&chain[0]].file;
            let span = self.span(file, &format!("env.{}.extends", chain[0]));
            self.diags.push(
                Diagnostic::new(format!(
                    "the deployments extend each other in a circle: {}",
                    chain.join(" → ")
                ))
                .at(span),
            );
            return None;
        }
        let raw = &self.envs[name];
        let (file, mut own) = (raw.file, raw.table.clone());
        let here = format!("env.{name}");
        let parents: Vec<(String, String)> = match own.remove("extends") {
            None => Vec::new(),
            Some(toml::Value::String(s)) => vec![(s, format!("{here}.extends"))],
            Some(toml::Value::Array(a)) if a.iter().all(|v| v.is_str()) => a
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    (
                        v.as_str().unwrap_or_default().to_string(),
                        format!("{here}.extends[{i}]"),
                    )
                })
                .collect(),
            Some(_) => {
                self.diags.push(
                    Diagnostic::new(format!(
                        "[env.{name}] extends: name a deployment, or a list of them"
                    ))
                    .at(self.span(file, &format!("{here}.extends"))),
                );
                return None;
            }
        };
        match own.remove("abstract") {
            None | Some(toml::Value::Boolean(_)) => {}
            Some(_) => self.diags.push(
                Diagnostic::new(format!("[env.{name}] abstract is true or false"))
                    .at(self.span(file, &format!("{here}.abstract"))),
            ),
        }
        let mut merged = toml::Table::new();
        let mut origins: BTreeMap<String, Origin> = BTreeMap::new();
        chain.push(name.to_string());
        for (parent, at) in parents {
            if !self.envs.contains_key(&parent) {
                let mut d = Diagnostic::new(format!(
                    "[env.{name}] extends {parent:?}, which the lane file doesn't have"
                ))
                .at(self.span(file, &at));
                if let Some(m) = did_you_mean(&parent, self.envs.keys().map(String::as_str)) {
                    d = d.help(format!("did you mean {m:?}?"));
                }
                self.diags.push(d);
                continue;
            }
            let Some((mut t, o)) = self.resolve(&parent, chain) else {
                continue;
            };
            // Being the default isn't inherited.
            t.remove("default");
            for (p, mut origin) in o {
                if p != ".default" {
                    origin.via.get_or_insert_with(|| parent.clone());
                    origins.insert(p, origin);
                }
            }
            merge(&mut merged, t);
        }
        chain.pop();
        // Its own values, last.
        let mut own_origins = BTreeMap::new();
        for ((f, p), (span, is_table)) in &self.origins {
            if *f == file {
                if let Some(rest) = p.strip_prefix(&here) {
                    if rest.starts_with('.') || rest.starts_with('[') {
                        own_origins.insert(
                            rest.to_string(),
                            (
                                Origin {
                                    span: span.clone(),
                                    via: None,
                                },
                                *is_table,
                            ),
                        );
                    }
                }
            }
        }
        merge(&mut merged, own);
        for (p, (o, is_table)) in own_origins {
            if p.starts_with(".extends") || p.starts_with(".abstract") {
                continue;
            }
            if !is_table {
                // A replaced value drops what it inherited below it.
                origins.retain(|q, _| !(q.starts_with(&p) && q[p.len()..].starts_with(['.', '['])));
            }
            origins.insert(p, o);
        }
        Some((merged, origins))
    }
}

/// Merges `top` over `base`: tables key by key, anything else replaced.
fn merge(base: &mut toml::Table, top: toml::Table) {
    for (k, v) in top {
        match (base.get_mut(&k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(t)) => merge(b, t),
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
}

/// Every value's path and byte range in a parsed document.
type Walked = Vec<(String, std::ops::Range<usize>, bool)>;

fn walk_table(t: &toml::de::DeTable<'_>, prefix: &str, out: &mut Walked) {
    for (k, v) in t {
        let p = if prefix.is_empty() {
            k.get_ref().to_string()
        } else {
            format!("{prefix}.{}", k.get_ref())
        };
        let is_table = matches!(v.get_ref(), toml::de::DeValue::Table(_));
        out.push((p.clone(), v.span(), is_table));
        walk_value(v.get_ref(), &p, out);
    }
}

fn walk_value(v: &toml::de::DeValue<'_>, prefix: &str, out: &mut Walked) {
    match v {
        toml::de::DeValue::Table(t) => walk_table(t, prefix, out),
        toml::de::DeValue::Array(a) => {
            for (i, item) in a.iter().enumerate() {
                let p = format!("{prefix}[{i}]");
                let is_table = matches!(item.get_ref(), toml::de::DeValue::Table(_));
                out.push((p.clone(), item.span(), is_table));
                walk_value(item.get_ref(), &p, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
