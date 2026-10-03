//! One deployment with its expressions evaluated (M0.6, C-08, DEC-080).
//!
//! ```toml
//! [vars.validators]
//! type = "list"
//! default = ["1", "2", "3"]
//! description = "the validators' names"
//!
//! [locals]
//! prefix = "${lane.name}-${env.name}"
//!
//! [env.base.validators]          # a table with for_each is a list of tables
//! for_each = "${var.validators}"
//! name = "${each.value}"
//! key = "${local.prefix}-v${each.value}"
//! ```
//!
//! - A var's value: its `default`, then `CARAVEL_VAR_<name>`, then each
//!   `--var-file`, then each `--var name=value`, the last one winning. A
//!   `--var` is read by the var's `type` (`string` as written; the others
//!   as a TOML value: `3`, `true`, `["1","2"]`, `{ a = 1 }`). A var with no
//!   value is an error, and so is a value for a var the file doesn't
//!   declare. `validation = [{ condition = "${…}", message = "…" }]` checks
//!   it. `sensitive = true` keeps its value out of messages.
//! - Locals are computed in the order they need each other; a cycle is an
//!   error.
//! - Names an expression can use: `var`, `local`, `lane` (`name`,
//!   `template`), `env` (`name`) and, in a `for_each`, `each` (`key`:
//!   the index or map key; `value`).
//! - A value that is `null` leaves its key out, so a deployment can drop
//!   what it inherits: `host = { address = "${null}" }`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::expr::{eval_str, has_expression, MapScope, Value};
use crate::{did_you_mean, Diagnostic, Error, LaneDoc};

/// Most instances one `for_each` makes.
const MAX_INSTANCES: usize = 1024;

/// What a command line gives the lane file.
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    /// `--var name=value`, in order.
    pub vars: Vec<(String, String)>,
    /// `--var-file`, in order.
    pub var_files: Vec<PathBuf>,
    /// `CARAVEL_VAR_<name>`, by name.
    pub env: BTreeMap<String, String>,
}

impl Inputs {
    /// `--var` arguments (`name=value`) and the process's `CARAVEL_VAR_*`.
    pub fn new(vars: &[String], var_files: &[PathBuf]) -> Result<Self, String> {
        let mut out = Self {
            var_files: var_files.to_vec(),
            ..Self::default()
        };
        for v in vars {
            let (k, val) = v
                .split_once('=')
                .ok_or_else(|| format!("--var {v:?}: write it as name=value"))?;
            out.vars.push((k.trim().to_string(), val.to_string()));
        }
        for (k, v) in std::env::vars() {
            if let Some(name) = k.strip_prefix("CARAVEL_VAR_") {
                out.env.insert(name.to_string(), v);
            }
        }
        Ok(out)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VarType {
    String,
    Integer,
    Boolean,
    List,
    Map,
    Any,
}

impl VarType {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "string" => Self::String,
            "integer" | "number" => Self::Integer,
            "boolean" | "bool" => Self::Boolean,
            "list" => Self::List,
            "map" => Self::Map,
            "any" => Self::Any,
            _ => return None,
        })
    }

    fn name(&self) -> &'static str {
        match self {
            Self::String => "a string",
            Self::Integer => "a number",
            Self::Boolean => "a boolean",
            Self::List => "a list",
            Self::Map => "a map",
            Self::Any => "any value",
        }
    }

    pub(crate) fn parse_name(s: &str) -> Option<Self> {
        Self::parse(s)
    }

    pub(crate) fn describe(&self) -> &'static str {
        self.name()
    }

    pub(crate) fn accepts_value(&self, v: &toml::Value) -> bool {
        self.accepts(v)
    }

    fn accepts(&self, v: &toml::Value) -> bool {
        matches!(
            (self, v),
            (Self::Any, _)
                | (Self::String, toml::Value::String(_))
                | (Self::Integer, toml::Value::Integer(_))
                | (Self::Boolean, toml::Value::Boolean(_))
                | (Self::List, toml::Value::Array(_))
                | (Self::Map, toml::Value::Table(_))
        )
    }
}

struct VarDecl {
    ty: VarType,
    default: Option<toml::Value>,
    sensitive: bool,
    validation: Vec<(String, String)>,
}

/// A deployment with its expressions evaluated.
#[derive(Clone, Debug)]
pub struct ResolvedEnv {
    pub name: String,
    /// `[env.<name>]`, literal.
    pub table: toml::Table,
    /// Each var's value.
    pub vars: BTreeMap<String, toml::Value>,
    /// The vars whose values are kept out of messages.
    pub sensitive: BTreeSet<String>,
    /// Values left as written because they need a root that isn't known yet
    /// (a `Deferred` root): their paths in the deployment, e.g.
    /// `relayer.feeds[0].options.oracle`.
    pub deferred: Vec<String>,
}

/// One output, evaluated.
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub name: String,
    pub value: Value,
    pub description: Option<String>,
    pub sensitive: bool,
}

impl ResolvedEnv {
    /// `name = value` for each var, masking sensitive ones, for `plan`.
    pub fn vars_line(&self) -> String {
        self.vars
            .iter()
            .map(|(k, v)| {
                if self.sensitive.contains(k) {
                    format!("{k} = (sensitive)")
                } else {
                    format!("{k} = {v}")
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Reads a `--var` or `CARAVEL_VAR_` value by the var's type.
fn read_input(ty: VarType, raw: &str) -> Result<toml::Value, String> {
    let as_toml = || -> Result<toml::Value, String> {
        let t: toml::Table =
            toml::from_str(&format!("v = {raw}")).map_err(|e| e.message().to_string())?;
        Ok(t.get("v")
            .cloned()
            .unwrap_or(toml::Value::String(raw.into())))
    };
    match ty {
        VarType::String => Ok(toml::Value::String(raw.to_string())),
        VarType::Any => Ok(as_toml().unwrap_or_else(|_| toml::Value::String(raw.to_string()))),
        _ => as_toml(),
    }
}

impl LaneDoc {
    pub(crate) fn diag_at(&self, path: &str, message: String) -> Diagnostic {
        let mut d = Diagnostic::new(message).at(self.origin(path).map(|o| o.span.clone()));
        if let Some(via) = self.origin(path).and_then(|o| o.via.clone()) {
            d = d.note(format!("inherited from [env.{via}]"));
        }
        d
    }

    fn error(&self, diagnostics: Vec<Diagnostic>) -> Error {
        Error {
            sources: self.sources.clone(),
            diagnostics,
        }
    }

    fn decls(&self, diags: &mut Vec<Diagnostic>) -> BTreeMap<String, VarDecl> {
        let mut out = BTreeMap::new();
        for (name, d) in &self.vars {
            let here = format!("vars.{name}");
            let Some(t) = d.as_table() else {
                diags.push(self.diag_at(
                    &here,
                    format!("[vars.{name}] must be a table (type, default, description)"),
                ));
                continue;
            };
            for k in t.keys() {
                if !["type", "default", "description", "sensitive", "validation"]
                    .contains(&k.as_str())
                {
                    let mut d = self.diag_at(
                        &format!("{here}.{k}"),
                        format!("[vars.{name}] has no `{k}`"),
                    );
                    if let Some(m) = did_you_mean(
                        k,
                        ["type", "default", "description", "sensitive", "validation"],
                    ) {
                        d = d.help(format!("did you mean `{m}`?"));
                    }
                    diags.push(d);
                }
            }
            let ty = match t.get("type") {
                None => VarType::Any,
                Some(toml::Value::String(s)) => match VarType::parse(s) {
                    Some(ty) => ty,
                    None => {
                        diags.push(
                            self.diag_at(
                                &format!("{here}.type"),
                                format!("[vars.{name}] type {s:?} is not a type"),
                            )
                            .help("string, integer, boolean, list, map or any"),
                        );
                        continue;
                    }
                },
                Some(_) => {
                    diags.push(self.diag_at(
                        &format!("{here}.type"),
                        format!("[vars.{name}] type is a name, like \"string\""),
                    ));
                    continue;
                }
            };
            let default = t.get("default").cloned();
            if let Some(v) = &default {
                if !ty.accepts(v) {
                    diags.push(self.diag_at(
                        &format!("{here}.default"),
                        format!("var `{name}`'s default is not {}", ty.name()),
                    ));
                }
                if has_expression_anywhere(v) {
                    diags.push(self.diag_at(&format!("{here}.default"), format!("var `{name}`'s default is literal: a var can't be computed (use a local)")));
                }
            }
            let mut validation = Vec::new();
            match t.get("validation") {
                None => {}
                Some(toml::Value::Array(rules)) => {
                    for (i, r) in rules.iter().enumerate() {
                        match (
                            r.get("condition").and_then(|v| v.as_str()),
                            r.get("message").and_then(|v| v.as_str()),
                        ) {
                            (Some(c), Some(m)) => validation.push((c.to_string(), m.to_string())),
                            _ => diags.push(self.diag_at(
                                &format!("{here}.validation[{i}]"),
                                format!(
                                    "var `{name}`: each validation has a condition and a message"
                                ),
                            )),
                        }
                    }
                }
                Some(_) => diags.push(self.diag_at(
                    &format!("{here}.validation"),
                    format!("var `{name}`: validation is a list of {{ condition, message }}"),
                )),
            }
            out.insert(
                name.clone(),
                VarDecl {
                    ty,
                    default,
                    sensitive: t.get("sensitive").and_then(|v| v.as_bool()) == Some(true),
                    validation,
                },
            );
        }
        out
    }

    /// Each var's value from its declaration and the inputs.
    pub fn resolve_vars(
        &self,
        inputs: &Inputs,
    ) -> Result<(BTreeMap<String, toml::Value>, BTreeSet<String>), Error> {
        self.resolve_vars_with(inputs, &|p: &Path| std::fs::read_to_string(p))
    }

    fn resolve_vars_with(
        &self,
        inputs: &Inputs,
        read: &dyn Fn(&Path) -> std::io::Result<String>,
    ) -> Result<(BTreeMap<String, toml::Value>, BTreeSet<String>), Error> {
        let mut diags = Vec::new();
        let decls = self.decls(&mut diags);
        let mut values: BTreeMap<String, toml::Value> = decls
            .iter()
            .filter_map(|(k, d)| d.default.clone().map(|v| (k.clone(), v)))
            .collect();
        let known = || decls.keys().map(String::as_str).collect::<Vec<_>>();
        let undeclared = |name: &str, from: &str| {
            let mut d = Diagnostic::new(format!(
                "{from}: the lane file declares no var `{name}`{}",
                if decls.is_empty() {
                    String::new()
                } else {
                    format!(" (it has {})", known().join(", "))
                }
            ));
            if let Some(m) = did_you_mean(name, known()) {
                d = d.help(format!("did you mean `{m}`?"));
            }
            d
        };
        let set = |values: &mut BTreeMap<String, toml::Value>,
                   name: &str,
                   raw: &str,
                   from: &str,
                   diags: &mut Vec<Diagnostic>| match decls.get(name) {
            None => diags.push(undeclared(name, from)),
            Some(d) => match read_input(d.ty, raw) {
                Ok(v) if d.ty.accepts(&v) => {
                    values.insert(name.to_string(), v);
                }
                Ok(_) | Err(_) => diags.push(Diagnostic::new(format!(
                    "{from}: var `{name}` is {}{}",
                    d.ty.name(),
                    if d.sensitive {
                        String::new()
                    } else {
                        format!(", not {raw:?}")
                    }
                ))),
            },
        };
        for (name, raw) in &inputs.env {
            set(
                &mut values,
                name,
                raw,
                &format!("CARAVEL_VAR_{name}"),
                &mut diags,
            );
        }
        for f in &inputs.var_files {
            let text = match read(f) {
                Ok(t) => t,
                Err(e) => {
                    diags.push(Diagnostic::new(format!("--var-file {}: {e}", f.display())));
                    continue;
                }
            };
            let t: toml::Table = match toml::from_str(&text) {
                Ok(t) => t,
                Err(e) => {
                    diags.push(Diagnostic::new(format!(
                        "--var-file {}: {}",
                        f.display(),
                        e.message()
                    )));
                    continue;
                }
            };
            for (name, v) in t {
                let from = format!("--var-file {}", f.display());
                match decls.get(&name) {
                    None => diags.push(undeclared(&name, &from)),
                    Some(d) if !d.ty.accepts(&v) => diags.push(Diagnostic::new(format!(
                        "{from}: var `{name}` is {}",
                        d.ty.name()
                    ))),
                    Some(_) if has_expression_anywhere(&v) => diags.push(Diagnostic::new(format!(
                        "{from}: var `{name}`'s value is literal"
                    ))),
                    Some(_) => {
                        values.insert(name, v);
                    }
                }
            }
        }
        for (name, raw) in &inputs.vars {
            set(&mut values, name, raw, &format!("--var {name}"), &mut diags);
        }
        for (name, d) in &decls {
            if !values.contains_key(name) {
                // Only an error for a deployment that needs it.
                continue;
            }
            for (cond, message) in &d.validation {
                let scope = MapScope(BTreeMap::from([(
                    "var".to_string(),
                    Value::Map(
                        values
                            .iter()
                            .map(|(k, v)| (k.clone(), Value::from_toml(v)))
                            .collect(),
                    ),
                )]));
                match eval_str(cond, &scope) {
                    Ok(Value::Bool(true)) => {}
                    Ok(Value::Bool(false)) => diags.push(
                        self.diag_at(&format!("vars.{name}"), format!("var `{name}`: {message}")),
                    ),
                    Ok(v) => diags.push(self.diag_at(
                        &format!("vars.{name}"),
                        format!(
                            "var `{name}`: a validation condition is {}, not a boolean",
                            v.kind()
                        ),
                    )),
                    Err(e) => diags.push(self.diag_at(
                        &format!("vars.{name}"),
                        format!("var `{name}`'s validation: {e}"),
                    )),
                }
            }
        }
        if !diags.is_empty() {
            return Err(self.error(diags));
        }
        let sensitive = decls
            .iter()
            .filter(|(_, d)| d.sensitive)
            .map(|(k, _)| k.clone())
            .collect();
        Ok((values, sensitive))
    }

    fn missing_var(&self, name: &str, env: &str) -> Diagnostic {
        self.diag_at(
            &format!("vars.{name}"),
            format!("var `{name}` has no value, and [env.{env}] needs it"),
        )
        .help(format!(
            "give it one: --var {name}=…, CARAVEL_VAR_{name}, a --var-file, or a default"
        ))
    }

    /// The deployment `env`, with its vars, locals and `for_each` resolved.
    pub fn resolve_env(&self, env: &str, inputs: &Inputs) -> Result<ResolvedEnv, Error> {
        self.resolve_env_with(env, inputs, &BTreeMap::new())
    }

    /// Like [`LaneDoc::resolve_env`], with more names for expressions
    /// (`extra`, over the built-in ones). A root that is
    /// [`Value::Deferred`] leaves the values that need it as written, listed
    /// in [`ResolvedEnv::deferred`].
    pub fn resolve_env_with(
        &self,
        env: &str,
        inputs: &Inputs,
        extra: &BTreeMap<String, Value>,
    ) -> Result<ResolvedEnv, Error> {
        let (scope, vars, sensitive) = self.scope_for(env, inputs, extra)?;
        let table = &self.envs[env];
        let mut diags = Vec::new();
        let mut deferred = Vec::new();
        let mut out = toml::Table::new();
        for (k, v) in table {
            if k == "outputs" || k == "modules" {
                continue;
            }
            let path = format!("env.{env}.{k}");
            if let Some(v) = self.eval_value(v, &scope, &path, &mut diags, &mut deferred) {
                out.insert(k.clone(), v);
            }
        }
        // A `.` in a name means a module's resource (C-21).
        for (kinds, _) in crate::modules::KINDS {
            if let Some(t) = table.get(kinds).and_then(|t| t.as_table()) {
                for n in t.keys().filter(|n| n.contains('.')) {
                    diags.push(self.diag_at(
                        &format!("env.{env}.{kinds}.{n}"),
                        format!("{kinds}.{n}: a name can't hold '.' (it would read as a module's)"),
                    ));
                }
            }
        }
        // Modules' resources join the deployment's.
        for inst in self.module_instances(env, &scope, &mut diags) {
            for (kinds, resources) in self.module_resources(env, &inst, &mut diags, &mut deferred) {
                let toml::Value::Table(t) = out
                    .entry(kinds.to_string())
                    .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                else {
                    continue;
                };
                for (n, r) in resources {
                    t.insert(n, r);
                }
            }
        }
        if !diags.is_empty() {
            return Err(self.error(diags));
        }
        let prefix = format!("env.{env}.");
        Ok(ResolvedEnv {
            name: env.to_string(),
            table: out,
            vars,
            sensitive,
            deferred: deferred
                .into_iter()
                .map(|p| p.strip_prefix(&prefix).unwrap_or(&p).to_string())
                .collect(),
        })
    }

    /// The deployment's outputs: `[outputs]`, then its own `[env.<name>.outputs]`
    /// over them. Each is an expression, or `{ value, description, sensitive }`.
    pub fn outputs(
        &self,
        env: &str,
        inputs: &Inputs,
        extra: &BTreeMap<String, Value>,
    ) -> Result<Vec<Output>, Error> {
        let (scope, _, sensitive_vars) = self.scope_for(env, inputs, extra)?;
        // `module.<instance>.<output>` (C-21).
        let mut diags = Vec::new();
        let modules: BTreeMap<String, Value> = self
            .module_instances(env, &scope, &mut diags)
            .iter()
            .map(|inst| {
                (
                    inst.name.clone(),
                    Value::Map(self.module_outputs(inst, &mut diags)),
                )
            })
            .collect();
        if !diags.is_empty() {
            return Err(self.error(diags));
        }
        let mut roots = scope.0.clone();
        roots.insert("module".into(), Value::Map(modules));
        let scope = MapScope(roots);
        let mut decls: BTreeMap<String, (toml::Value, String)> = self
            .outputs
            .iter()
            .map(|(k, v)| (k.clone(), (v.clone(), format!("outputs.{k}"))))
            .collect();
        if let Some(t) = self.envs[env].get("outputs").and_then(|o| o.as_table()) {
            for (k, v) in t {
                decls.insert(k.clone(), (v.clone(), format!("env.{env}.outputs.{k}")));
            }
        }
        let mut diags = Vec::new();
        let mut out = Vec::new();
        for (name, (decl, path)) in decls {
            let (expr, description, sensitive) = match &decl {
                toml::Value::Table(t) => (
                    t.get("value")
                        .cloned()
                        .unwrap_or(toml::Value::Boolean(false)),
                    t.get("description")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    t.get("sensitive").and_then(|v| v.as_bool()) == Some(true),
                ),
                v => (v.clone(), None, false),
            };
            let at = if decl.is_table() {
                format!("{path}.value")
            } else {
                path.clone()
            };
            let value = match &expr {
                toml::Value::String(s) if has_expression(s) => match eval_str(s, &scope) {
                    Ok(v) => v,
                    Err(e) => {
                        diags.push(self.diag_at(&at, format!("output `{name}`: {}", e.message)));
                        continue;
                    }
                },
                v => Value::from_toml(v),
            };
            if let Value::Deferred(r) = &value {
                diags.push(self.diag_at(
                    &at,
                    format!(
                        "output `{name}` needs {}, which isn't known here",
                        r.iter().cloned().collect::<Vec<_>>().join(", ")
                    ),
                ));
                continue;
            }
            // An output computed from a sensitive var is sensitive too.
            let mut used = BTreeSet::new();
            refs("var.", &expr, &mut used);
            let sensitive = sensitive || used.iter().any(|v| sensitive_vars.contains(v));
            out.push(Output {
                name,
                value,
                description,
                sensitive,
            });
        }
        if !diags.is_empty() {
            return Err(self.error(diags));
        }
        Ok(out)
    }

    /// The names a deployment's expressions can use, and the vars.
    #[allow(clippy::type_complexity)]
    fn scope_for(
        &self,
        env: &str,
        inputs: &Inputs,
        extra: &BTreeMap<String, Value>,
    ) -> Result<(MapScope, BTreeMap<String, toml::Value>, BTreeSet<String>), Error> {
        let Some(table) = self.envs.get(env) else {
            let mut d = if self.abstract_envs.contains(env) {
                Diagnostic::new(format!(
                    "[env.{env}] is abstract: it only exists to be extended"
                ))
            } else if self.envs.is_empty() {
                Diagnostic::new(format!(
                    "the lane file has no deployments; add an [env.{env}] table"
                ))
            } else {
                Diagnostic::new(format!(
                    "no [env.{env}]; the file has {}",
                    self.envs
                        .keys()
                        .map(|k| format!("[env.{k}]"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            };
            if let Some(m) = did_you_mean(env, self.envs.keys().map(String::as_str)) {
                d = d.help(format!("did you mean {m:?}?"));
            }
            return Err(self.error(vec![d]));
        };
        let (vars, sensitive) = self.resolve_vars(inputs)?;
        let mut diags = Vec::new();
        // A var with no value is an error only where it's used.
        let mut used = BTreeSet::new();
        refs("var.", &toml::Value::Table(table.clone()), &mut used);
        for v in self.locals.values() {
            refs("var.", v, &mut used);
        }
        for name in &used {
            if self.vars.contains_key(name) && !vars.contains_key(name) {
                diags.push(self.missing_var(name, env));
            }
        }
        if !diags.is_empty() {
            return Err(self.error(diags));
        }
        let lane = |k: &str| {
            self.genesis
                .get(k)
                .and_then(|t| t.as_table())
                .cloned()
                .unwrap_or_default()
        };
        let lane_root = Value::Map(BTreeMap::from([
            (
                "name".to_string(),
                Value::Str(
                    lane("lane")
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                ),
            ),
            (
                "template".to_string(),
                Value::Str(
                    lane("app")
                        .get("template")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                ),
            ),
        ]));
        let mut roots = BTreeMap::from([
            (
                "var".to_string(),
                Value::Map(
                    vars.iter()
                        .map(|(k, v)| (k.clone(), Value::from_toml(v)))
                        .collect(),
                ),
            ),
            ("lane".to_string(), lane_root),
            (
                "env".to_string(),
                Value::Map(BTreeMap::from([(
                    "name".to_string(),
                    Value::Str(env.to_string()),
                )])),
            ),
        ]);
        for (k, v) in extra {
            roots.insert(k.clone(), v.clone());
        }
        let locals = self.resolve_locals(&roots, &mut diags);
        roots.insert("local".to_string(), Value::Map(locals));
        if !diags.is_empty() {
            return Err(self.error(diags));
        }
        Ok((MapScope(roots), vars, sensitive))
    }

    /// Every local, in the order they need each other.
    fn resolve_locals(
        &self,
        roots: &BTreeMap<String, Value>,
        diags: &mut Vec<Diagnostic>,
    ) -> BTreeMap<String, Value> {
        let mut done: BTreeMap<String, Value> = BTreeMap::new();
        let mut state: BTreeMap<String, u8> = BTreeMap::new(); // 1: visiting, 2: done
        fn needs(v: &toml::Value, out: &mut BTreeSet<String>) {
            refs("local.", v, out);
        }
        fn visit(
            doc: &LaneDoc,
            name: &str,
            roots: &BTreeMap<String, Value>,
            done: &mut BTreeMap<String, Value>,
            state: &mut BTreeMap<String, u8>,
            chain: &mut Vec<String>,
            diags: &mut Vec<Diagnostic>,
        ) {
            match state.get(name) {
                Some(2) => return,
                Some(1) => {
                    chain.push(name.to_string());
                    let at = chain.iter().position(|c| c == name).unwrap_or(0);
                    diags.push(doc.diag_at(
                        &format!("locals.{name}"),
                        format!(
                            "the locals need each other in a circle: {}",
                            chain[at..].join(" → ")
                        ),
                    ));
                    chain.pop();
                    return;
                }
                _ => {}
            }
            let Some(v) = doc.locals.get(name) else {
                return;
            };
            state.insert(name.to_string(), 1);
            chain.push(name.to_string());
            let mut deps = BTreeSet::new();
            needs(v, &mut deps);
            for d in deps {
                if doc.locals.contains_key(&d) {
                    visit(doc, &d, roots, done, state, chain, diags);
                }
            }
            chain.pop();
            let mut r = roots.clone();
            r.insert("local".to_string(), Value::Map(done.clone()));
            let scope = MapScope(r);
            let value = match v {
                // A local may be known later; it stays a deferred value.
                toml::Value::String(s) if has_expression(s) => match eval_str(s, &scope) {
                    Ok(v) => v,
                    Err(e) => {
                        let mut d = doc.diag_at(
                            &format!("locals.{name}"),
                            format!("local `{name}`: {}", e.message),
                        );
                        if let Some(h) = e.help {
                            d = d.help(h);
                        }
                        diags.push(d);
                        Value::Null
                    }
                },
                v => doc
                    .eval_value(v, &scope, &format!("locals.{name}"), diags, &mut Vec::new())
                    .map(|v| Value::from_toml(&v))
                    .unwrap_or(Value::Null),
            };
            done.insert(name.to_string(), value);
            state.insert(name.to_string(), 2);
        }
        for name in self.locals.keys() {
            visit(
                self,
                name,
                roots,
                &mut done,
                &mut state,
                &mut Vec::new(),
                diags,
            );
        }
        done
    }

    /// A value with its expressions evaluated; `None` leaves the key out.
    pub(crate) fn eval_value(
        &self,
        v: &toml::Value,
        scope: &MapScope,
        path: &str,
        diags: &mut Vec<Diagnostic>,
        deferred: &mut Vec<String>,
    ) -> Option<toml::Value> {
        match v {
            toml::Value::String(s) if has_expression(s) => match eval_str(s, scope) {
                Ok(Value::Deferred(_)) => {
                    // Known later: left as written.
                    deferred.push(path.to_string());
                    Some(v.clone())
                }
                Ok(val) => match val.to_toml() {
                    Ok(t) => t,
                    Err(e) => {
                        diags.push(self.diag_at(path, format!("{path}: {e}")));
                        None
                    }
                },
                Err(e) => {
                    let mut d = self.diag_at(path, format!("{path}: {}", e.message));
                    if let Some(h) = e.help {
                        d = d.help(h);
                    }
                    diags.push(d);
                    None
                }
            },
            toml::Value::Table(t) if t.contains_key("for_each") => Some(toml::Value::Array(
                self.expand(t, scope, path, diags, deferred)?,
            )),
            toml::Value::Table(t) => {
                let mut out = toml::Table::new();
                for (k, v) in t {
                    if let Some(v) =
                        self.eval_value(v, scope, &format!("{path}.{k}"), diags, deferred)
                    {
                        out.insert(k.clone(), v);
                    }
                }
                Some(toml::Value::Table(out))
            }
            toml::Value::Array(a) => {
                let mut out = Vec::with_capacity(a.len());
                for (i, item) in a.iter().enumerate() {
                    let at = format!("{path}[{i}]");
                    match item {
                        toml::Value::Table(t) if t.contains_key("for_each") => {
                            out.extend(
                                self.expand(t, scope, &at, diags, deferred)
                                    .unwrap_or_default(),
                            );
                        }
                        item => match self.eval_value(item, scope, &at, diags, deferred) {
                            Some(v) => out.push(v),
                            None if diags.is_empty() => {
                                diags.push(
                                    self.diag_at(&at, format!("{at}: a list can't hold null")),
                                );
                            }
                            None => {}
                        },
                    }
                }
                Some(toml::Value::Array(out))
            }
            other => Some(other.clone()),
        }
    }

    /// A table with `for_each`: one table per item, `each` in scope.
    fn expand(
        &self,
        t: &toml::Table,
        scope: &MapScope,
        path: &str,
        diags: &mut Vec<Diagnostic>,
        deferred: &mut Vec<String>,
    ) -> Option<Vec<toml::Value>> {
        let at = format!("{path}.for_each");
        let over = match t.get("for_each") {
            Some(toml::Value::String(s)) => match eval_str(s, scope) {
                Ok(v) => v,
                Err(e) => {
                    diags.push(self.diag_at(&at, format!("{at}: {}", e.message)));
                    return None;
                }
            },
            Some(v) => Value::from_toml(v),
            None => return None,
        };
        let items: Vec<(Value, Value)> = match over {
            Value::List(l) => l
                .into_iter()
                .enumerate()
                .map(|(i, v)| (Value::Int(i as i64), v))
                .collect(),
            Value::Map(m) => m.into_iter().map(|(k, v)| (Value::Str(k), v)).collect(),
            Value::Deferred(_) => {
                diags.push(self.diag_at(&at, format!("{at}: for_each must be known before anything is read (from var, local, lane or env)")));
                return None;
            }
            v => {
                diags.push(self.diag_at(
                    &at,
                    format!(
                        "{at}: for_each goes through a list or a map, not {}",
                        v.kind()
                    ),
                ));
                return None;
            }
        };
        if items.len() > MAX_INSTANCES {
            diags.push(self.diag_at(&at, format!("{at}: more than {MAX_INSTANCES} instances")));
            return None;
        }
        let mut out = Vec::with_capacity(items.len());
        for (key, value) in items {
            let mut roots = scope.0.clone();
            roots.insert(
                "each".to_string(),
                Value::Map(BTreeMap::from([
                    ("key".to_string(), key),
                    ("value".to_string(), value),
                ])),
            );
            let inner = MapScope(roots);
            let mut one = toml::Table::new();
            for (k, v) in t {
                if k == "for_each" {
                    continue;
                }
                if let Some(v) = self.eval_value(v, &inner, &format!("{path}.{k}"), diags, deferred)
                {
                    one.insert(k.clone(), v);
                }
            }
            out.push(toml::Value::Table(one));
        }
        Some(out)
    }
}

/// The names after `prefix` (`var.`, `local.`) anywhere in a value's
/// strings. A rough scan: it may find more than an expression uses, never
/// less.
fn refs(prefix: &str, v: &toml::Value, out: &mut BTreeSet<String>) {
    match v {
        toml::Value::String(s) => {
            let mut rest = s.as_str();
            while let Some(i) = rest.find(prefix) {
                let after = &rest[i + prefix.len()..];
                let name: String = after
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .collect();
                out.insert(name.trim_end_matches('-').to_string());
                rest = after;
            }
        }
        toml::Value::Array(a) => a.iter().for_each(|v| refs(prefix, v, out)),
        toml::Value::Table(t) => t.values().for_each(|v| refs(prefix, v, out)),
        _ => {}
    }
}

fn has_expression_anywhere(v: &toml::Value) -> bool {
    match v {
        toml::Value::String(s) => has_expression(s),
        toml::Value::Array(a) => a.iter().any(has_expression_anywhere),
        toml::Value::Table(t) => t.values().any(has_expression_anywhere),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENESIS: &str = "[lane]\nname = \"acme\"\n[app]\ntemplate = \"payments\"\n";

    fn doc(text: &str) -> LaneDoc {
        LaneDoc::parse(&format!("{GENESIS}{text}")).unwrap_or_else(|e| panic!("{e}"))
    }

    fn resolve(text: &str, env: &str, inputs: &Inputs) -> Result<ResolvedEnv, String> {
        doc(text)
            .resolve_env(env, inputs)
            .map_err(|e| e.to_string())
    }

    fn vars(pairs: &[(&str, &str)]) -> Inputs {
        Inputs {
            vars: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Inputs::default()
        }
    }

    const FILE: &str = r#"
[vars.validators]
type = "list"
default = ["1", "2", "3"]
[vars.host]
type = "string"
[vars.n]
type = "integer"
default = 2
validation = [{ condition = "${var.n >= 1}", message = "n is at least 1" }]

[locals]
prefix = "${lane.name}-${env.name}"
key_of = "${local.prefix}-v"
ports = { sequencer = 18080 }

[env.base]
abstract = true
admin = "${local.prefix}-admin"
threshold = "${length(var.validators) * 2 / 3 + 1}"
[env.base.validators]
for_each = "${var.validators}"
name = "${each.value}"
key = "${local.key_of}${each.value}"
port = "${local.ports.sequencer + each.key + 1}"

[env.local]
extends = "base"
host = { provider = "local", address = "${null}" }

[env.testnet]
extends = "base"
host = { provider = "ssh", address = "${var.host}" }
"#;

    #[test]
    fn a_deployment_resolved() {
        let r = resolve(FILE, "local", &Inputs::default()).unwrap();
        let t = &r.table;
        assert_eq!(t["admin"].as_str(), Some("acme-local-admin"));
        assert_eq!(t["threshold"].as_integer(), Some(3));
        let v = t["validators"].as_array().unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[1]["name"].as_str(), Some("2"));
        assert_eq!(v[1]["key"].as_str(), Some("acme-local-v2"));
        assert_eq!(v[1]["port"].as_integer(), Some(18082));
        assert_eq!(t["host"].as_table().unwrap().len(), 1, "null drops the key");
        assert_eq!(r.vars["n"].as_integer(), Some(2));
    }

    #[test]
    fn inputs_and_their_precedence() {
        // A var with no value is an error only where it's used.
        let e = resolve(FILE, "testnet", &Inputs::default()).unwrap_err();
        assert!(
            e.contains("var `host` has no value, and [env.testnet] needs it"),
            "{e}"
        );
        assert!(resolve(FILE, "local", &Inputs::default()).is_ok());
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("prod.toml");
        std::fs::write(&f, "host = \"from-file\"\nvalidators = [\"7\"]\n").unwrap();
        let mut i = Inputs {
            var_files: vec![f],
            env: BTreeMap::from([("host".into(), "from-env".into()), ("n".into(), "5".into())]),
            ..Inputs::default()
        };
        let r = resolve(FILE, "testnet", &i).unwrap();
        assert_eq!(r.table["host"]["address"].as_str(), Some("from-file"));
        assert_eq!(r.vars["n"].as_integer(), Some(5));
        assert_eq!(r.table["validators"].as_array().unwrap().len(), 1);
        i.vars = vec![
            ("host".into(), "from-flag".into()),
            ("validators".into(), r#"["1","2","4","5"]"#.into()),
        ];
        let r = resolve(FILE, "testnet", &i).unwrap();
        assert_eq!(r.table["host"]["address"].as_str(), Some("from-flag"));
        assert_eq!(r.table["threshold"].as_integer(), Some(3));
        assert_eq!(
            r.table["validators"][3]["key"].as_str(),
            Some("acme-testnet-v5")
        );
    }

    #[test]
    fn bad_inputs() {
        let e = resolve(FILE, "local", &vars(&[("hots", "x")])).unwrap_err();
        assert!(
            e.contains("declares no var `hots`") && e.contains("did you mean `host`?"),
            "{e}"
        );
        let e = resolve(FILE, "local", &vars(&[("n", "three")])).unwrap_err();
        assert!(e.contains("var `n` is a number"), "{e}");
        let e = resolve(FILE, "local", &vars(&[("n", "0")])).unwrap_err();
        assert!(e.contains("n is at least 1"), "{e}");
        let e = resolve(FILE, "local", &vars(&[("validators", "1")])).unwrap_err();
        assert!(e.contains("is a list"), "{e}");
        assert!(Inputs::new(&["novalue".into()], &[]).is_err());
    }

    #[test]
    fn sensitive_values_stay_out_of_messages() {
        let text = "[vars.token]\ntype = \"integer\"\nsensitive = true\n[env.a]\nx = 1\n";
        let e = resolve(text, "a", &vars(&[("token", "hunter2")])).unwrap_err();
        assert!(!e.contains("hunter2"), "{e}");
        let r = resolve(text, "a", &vars(&[("token", "42")])).unwrap();
        assert_eq!(r.vars_line(), "token = (sensitive)");
    }

    #[test]
    fn declaration_errors() {
        let e = doc("[vars.a]\ntype = \"str\"\n[env.x]\nn = 1\n")
            .resolve_env("x", &Inputs::default())
            .unwrap_err()
            .to_string();
        assert!(e.contains("is not a type"), "{e}");
        let e = resolve(
            "[vars.a]\ntype = \"integer\"\ndefault = \"x\"\n[env.x]\nn = 1\n",
            "x",
            &Inputs::default(),
        )
        .unwrap_err();
        assert!(e.contains("default is not a number"), "{e}");
        let e = resolve(
            "[vars.a]\ndefualt = 1\n[env.x]\nn = 1\n",
            "x",
            &Inputs::default(),
        )
        .unwrap_err();
        assert!(e.contains("did you mean `default`?"), "{e}");
        let e = resolve(
            "[vars.a]\ndefault = \"${lane.name}\"\n[env.x]\nn = 1\n",
            "x",
            &Inputs::default(),
        )
        .unwrap_err();
        assert!(e.contains("can't be computed"), "{e}");
    }

    #[test]
    fn locals_in_order_and_cycles() {
        let r = resolve(
            "[locals]\nb = \"${local.a}-b\"\na = \"a\"\n[env.x]\nv = \"${local.b}\"\n",
            "x",
            &Inputs::default(),
        )
        .unwrap();
        assert_eq!(r.table["v"].as_str(), Some("a-b"));
        let e = resolve(
            "[locals]\na = \"${local.b}\"\nb = \"${local.a}\"\n[env.x]\nv = 1\n",
            "x",
            &Inputs::default(),
        )
        .unwrap_err();
        assert!(e.contains("in a circle"), "{e}");
    }

    #[test]
    fn for_each_forms_and_errors() {
        let r = resolve(
            "[env.x]\n[[env.x.list]]\nfor_each = \"${{ a = 1, b = 2 }}\"\nk = \"${each.key}\"\nv = \"${each.value}\"\n[[env.x.list]]\nk = \"fixed\"\n",
            "x",
            &Inputs::default(),
        )
        .unwrap();
        let l = r.table["list"].as_array().unwrap();
        assert_eq!(l.len(), 3);
        assert_eq!(
            (
                l[0]["k"].as_str(),
                l[1]["v"].as_integer(),
                l[2]["k"].as_str()
            ),
            (Some("a"), Some(2), Some("fixed"))
        );
        let e = resolve("[env.x.v]\nfor_each = \"${3}\"\n", "x", &Inputs::default()).unwrap_err();
        assert!(e.contains("goes through a list or a map"), "{e}");
        let e = resolve(
            "[env.x.v]\nfor_each = \"${range(2000)}\"\n",
            "x",
            &Inputs::default(),
        )
        .unwrap_err();
        assert!(e.contains("more than 1024"), "{e}");
        let e = resolve("[env.x]\na = \"${var.nope}\"\n", "x", &Inputs::default()).unwrap_err();
        assert!(e.contains("env.x.a: no `nope` here"), "{e}");
        assert!(e.contains("--> lane.toml:"), "{e}");
    }

    #[test]
    fn missing_and_abstract_deployments() {
        let e = resolve(FILE, "base", &Inputs::default()).unwrap_err();
        assert!(e.contains("abstract"), "{e}");
        let e = resolve(FILE, "lcoal", &Inputs::default()).unwrap_err();
        assert!(
            e.contains("no [env.lcoal]") && e.contains("did you mean \"local\"?"),
            "{e}"
        );
    }

    #[test]
    fn values_known_later_and_outputs() {
        let text = r#"
[locals]
oracle = "${contract.oracle.address}"

[outputs]
api = "${node.sequencer.url}"
oracle = { value = "${local.oracle}", description = "the oracle contract" }
plain = "${lane.name}"

[env.x]
admin = "a"
relayer = { feeds = [{ options = { oracle = "${local.oracle}", n = "${1 + 1}" } }] }
[env.x.outputs]
plain = "overridden"
"#;
        let d = doc(text);
        let later = BTreeMap::from([
            (
                "contract".to_string(),
                Value::Deferred(BTreeSet::from(["contract".to_string()])),
            ),
            (
                "node".to_string(),
                Value::Deferred(BTreeSet::from(["node".to_string()])),
            ),
        ]);
        let r = d.resolve_env_with("x", &Inputs::default(), &later).unwrap();
        assert_eq!(r.deferred, ["relayer.feeds[0].options.oracle"]);
        assert_eq!(
            r.table["relayer"]["feeds"][0]["options"]["oracle"].as_str(),
            Some("${local.oracle}"),
            "left as written"
        );
        assert_eq!(
            r.table["relayer"]["feeds"][0]["options"]["n"].as_integer(),
            Some(2)
        );
        assert!(!r.table.contains_key("outputs"));
        // Outputs need the real values.
        let e = d
            .outputs("x", &Inputs::default(), &later)
            .unwrap_err()
            .to_string();
        assert!(e.contains("output `api` needs node.sequencer.url"), "{e}");
        let known = BTreeMap::from([
            (
                "contract".to_string(),
                Value::Map(BTreeMap::from([(
                    "oracle".to_string(),
                    Value::Map(BTreeMap::from([(
                        "address".to_string(),
                        Value::Str("CABC".into()),
                    )])),
                )])),
            ),
            (
                "node".to_string(),
                Value::Map(BTreeMap::from([(
                    "sequencer".to_string(),
                    Value::Map(BTreeMap::from([(
                        "url".to_string(),
                        Value::Str("http://x".into()),
                    )])),
                )])),
            ),
        ]);
        let r = d.resolve_env_with("x", &Inputs::default(), &known).unwrap();
        assert!(r.deferred.is_empty());
        assert_eq!(
            r.table["relayer"]["feeds"][0]["options"]["oracle"].as_str(),
            Some("CABC")
        );
        let o = d.outputs("x", &Inputs::default(), &known).unwrap();
        let get = |n: &str| o.iter().find(|x| x.name == n).unwrap();
        assert_eq!(get("api").value, Value::Str("http://x".into()));
        assert_eq!(get("oracle").value, Value::Str("CABC".into()));
        assert_eq!(
            get("oracle").description.as_deref(),
            Some("the oracle contract")
        );
        assert_eq!(get("plain").value, Value::Str("overridden".into()));
    }

    #[test]
    fn an_output_from_a_sensitive_var_is_sensitive() {
        let text = "[vars.t]\nsensitive = true\ndefault = \"x\"\n[outputs]\nt = \"${var.t}-y\"\nopen = { value = \"z\", sensitive = false }\n[env.x]\na = 1\n";
        let o = doc(text)
            .outputs("x", &Inputs::default(), &BTreeMap::new())
            .unwrap();
        assert!(o.iter().find(|x| x.name == "t").unwrap().sensitive);
        assert!(!o.iter().find(|x| x.name == "open").unwrap().sensitive);
    }
}
