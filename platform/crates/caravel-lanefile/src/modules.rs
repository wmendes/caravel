//! Local modules (M0.6, C-21, DEC-093): a deployment's
//! `[env.<name>.modules.<m>]` brings in the accounts, tokens and contracts
//! a module file declares, under the module instance's name.
//!
//! ```toml
//! [env.local.modules.btc]
//! source = "modules/oracle.toml"      # relative to the lane file
//! inputs = { symbol = "BTC", decimals = 8 }
//! ```
//!
//! A module file holds only `[inputs.<n>]` (`type`, `default`,
//! `description`), `[locals]`, `[accounts.<n>]`, `[tokens.<n>]`,
//! `[contracts.<n>]` and `[outputs]`. Inside it, expressions read `input`,
//! its own `local`, `lane` and `env`, and the deployment's `account`,
//! `token`, `contract`, `network` and other names known later; its own
//! resources by their short names. It doesn't see the lane file's vars:
//! what it needs comes in as inputs.
//!
//! Its resources join the deployment as `<instance>.<n>` (a `.` in a name
//! means a module's), which the deploy tool addresses as
//! `module.<instance>.<kind>.<n>`. With `for_each`, each item is an
//! instance, `<m>-<key>`. Remote sources aren't supported.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::expr::{eval_str, has_expression, MapScope, Value};
use crate::resolve::VarType;
use crate::{did_you_mean, Diagnostic, LaneDoc};

/// The tables a module file may have.
const MODULE_KEYS: [&str; 6] = [
    "inputs",
    "locals",
    "accounts",
    "tokens",
    "contracts",
    "outputs",
];
/// The resource kinds a module declares, by their table and their address's kind.
pub(crate) const KINDS: [(&str, &str); 3] = [
    ("accounts", "account"),
    ("tokens", "token"),
    ("contracts", "contract"),
];

/// One instance of a module.
pub(crate) struct Instance {
    pub name: String,
    pub file: PathBuf,
    pub body: toml::Table,
    pub scope: MapScope,
}

fn str_of(v: &toml::Value) -> Option<&str> {
    v.as_str()
}

impl LaneDoc {
    /// The module instances of deployment `env` (its `modules` table), each
    /// with the scope its file's expressions are read in. `scope` is the
    /// deployment's.
    pub(crate) fn module_instances(
        &self,
        env: &str,
        scope: &MapScope,
        diags: &mut Vec<Diagnostic>,
    ) -> Vec<Instance> {
        let Some(mods) = self.envs[env].get("modules").and_then(|m| m.as_table()) else {
            return Vec::new();
        };
        let base = self
            .sources
            .0
            .first()
            .and_then(|s| s.path.parent().map(std::path::Path::to_path_buf))
            .unwrap_or_default();
        let mut out = Vec::new();
        for (m, spec) in mods {
            let at = format!("env.{env}.modules.{m}");
            let Some(spec) = spec.as_table() else {
                diags.push(self.diag_at(&at, format!("modules.{m} is a table: source and inputs")));
                continue;
            };
            for k in spec.keys() {
                if !matches!(k.as_str(), "source" | "inputs" | "for_each") {
                    diags.push(self.diag_at(
                        &format!("{at}.{k}"),
                        format!("modules.{m}: unknown key `{k}` (source, inputs, for_each)"),
                    ));
                }
            }
            let Some(source) = spec.get("source").and_then(str_of) else {
                diags.push(self.diag_at(
                    &at,
                    format!("modules.{m} needs a source: a .toml file relative to the lane file"),
                ));
                continue;
            };
            if source.contains("://") {
                diags.push(self.diag_at(
                    &format!("{at}.source"),
                    format!("modules.{m}.source = {source:?}: modules are local files (no remote sources)"),
                ));
                continue;
            }
            let file = base.join(source);
            let body: toml::Table = match std::fs::read_to_string(&file)
                .map_err(|e| e.to_string())
                .and_then(|t| toml::from_str(&t).map_err(|e| e.to_string()))
            {
                Ok(b) => b,
                Err(e) => {
                    diags.push(self.diag_at(
                        &format!("{at}.source"),
                        format!("modules.{m}: reading {}: {e}", file.display()),
                    ));
                    continue;
                }
            };
            for k in body.keys() {
                if !MODULE_KEYS.contains(&k.as_str()) {
                    diags.push(self.diag_at(
                        &format!("{at}.source"),
                        format!(
                            "{}: a module file has only {}, not `{k}`",
                            file.display(),
                            MODULE_KEYS.join(", ")
                        ),
                    ));
                }
            }
            // One instance, or one per for_each item (`<m>-<key>`).
            let mut items: Vec<(String, Option<(Value, Value)>)> = Vec::new();
            match spec.get("for_each") {
                None => items.push((m.clone(), None)),
                Some(fe) => {
                    let over = match fe {
                        toml::Value::String(s) if has_expression(s) => eval_str(s, scope),
                        v => Ok(Value::from_toml(v)),
                    };
                    let pairs: Vec<(Value, Value)> = match over {
                        Ok(Value::List(l)) => l
                            .into_iter()
                            .enumerate()
                            .map(|(i, v)| (Value::Int(i as i64), v))
                            .collect(),
                        Ok(Value::Map(map)) => {
                            map.into_iter().map(|(k, v)| (Value::Str(k), v)).collect()
                        }
                        Ok(v) => {
                            diags.push(self.diag_at(
                                &format!("{at}.for_each"),
                                format!("modules.{m}.for_each goes through a list or a map, known before anything is read, not {}", v.kind()),
                            ));
                            continue;
                        }
                        Err(e) => {
                            diags.push(self.diag_at(
                                &format!("{at}.for_each"),
                                format!("modules.{m}.for_each: {}", e.message),
                            ));
                            continue;
                        }
                    };
                    for (k, v) in pairs {
                        let suffix = match (&k, &v) {
                            (Value::Str(s), _) | (_, Value::Str(s)) => s.clone(),
                            (Value::Int(i), _) => i.to_string(),
                            _ => continue,
                        };
                        items.push((format!("{m}-{suffix}"), Some((k, v))));
                    }
                }
            }
            for (name, each) in items {
                let mut env_roots = scope.0.clone();
                if let Some((k, v)) = each {
                    env_roots.insert(
                        "each".into(),
                        Value::Map(BTreeMap::from([("key".into(), k), ("value".into(), v)])),
                    );
                }
                let env_scope = MapScope(env_roots);
                let input = self.module_inputs(
                    &at,
                    &name,
                    &file,
                    &body,
                    spec.get("inputs"),
                    &env_scope,
                    diags,
                );
                let scope = self.module_scope(&name, &body, input, scope, &at, diags);
                out.push(Instance {
                    name,
                    file: file.clone(),
                    body: body.clone(),
                    scope,
                });
            }
        }
        out
    }

    /// An instance's inputs: each one given (read in the deployment's
    /// scope), else its default; checked against its declaration.
    #[allow(clippy::too_many_arguments)]
    fn module_inputs(
        &self,
        at: &str,
        name: &str,
        file: &std::path::Path,
        body: &toml::Table,
        given: Option<&toml::Value>,
        scope: &MapScope,
        diags: &mut Vec<Diagnostic>,
    ) -> BTreeMap<String, Value> {
        let decls = body
            .get("inputs")
            .and_then(|i| i.as_table())
            .cloned()
            .unwrap_or_default();
        let given = given
            .and_then(|g| g.as_table())
            .cloned()
            .unwrap_or_default();
        let mut out = BTreeMap::new();
        for (k, v) in &given {
            if !decls.contains_key(k) {
                let mut d = self.diag_at(
                    &format!("{at}.inputs.{k}"),
                    format!("modules.{name}: {} declares no input `{k}`", file.display()),
                );
                if let Some(m) = did_you_mean(k, decls.keys().map(String::as_str)) {
                    d = d.help(format!("did you mean `{m}`?"));
                }
                diags.push(d);
                continue;
            }
            let value = match v {
                toml::Value::String(s) if has_expression(s) => match eval_str(s, scope) {
                    Ok(v) => v,
                    Err(e) => {
                        diags.push(self.diag_at(
                            &format!("{at}.inputs.{k}"),
                            format!("modules.{name}.inputs.{k}: {}", e.message),
                        ));
                        continue;
                    }
                },
                v => Value::from_toml(v),
            };
            out.insert(k.clone(), value);
        }
        for (k, d) in &decls {
            let d = d.as_table().cloned().unwrap_or_default();
            let ty = d
                .get("type")
                .and_then(str_of)
                .and_then(VarType::parse_name)
                .unwrap_or(VarType::Any);
            match out.get(k) {
                None => match d.get("default") {
                    Some(v) => {
                        out.insert(k.clone(), Value::from_toml(v));
                    }
                    None => diags.push(self.diag_at(
                        &format!("{at}.inputs"),
                        format!(
                            "modules.{name} needs input `{k}` ({} has no default)",
                            file.display()
                        ),
                    )),
                },
                Some(Value::Deferred(_)) => {}
                Some(v) => {
                    if let Ok(Some(t)) = v.to_toml() {
                        if !ty.accepts_value(&t) {
                            diags.push(self.diag_at(
                                &format!("{at}.inputs.{k}"),
                                format!("modules.{name}.inputs.{k} must be {}", ty.describe()),
                            ));
                        }
                    }
                }
            }
        }
        out
    }

    /// The names a module instance's expressions read.
    fn module_scope(
        &self,
        name: &str,
        body: &toml::Table,
        input: BTreeMap<String, Value>,
        env_scope: &MapScope,
        at: &str,
        diags: &mut Vec<Diagnostic>,
    ) -> MapScope {
        let mut roots: BTreeMap<String, Value> = env_scope
            .0
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "var" | "local" | "each"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        // Its own resources by their short names, over the deployment's.
        for (table, kind) in KINDS {
            let own: Vec<&String> = body
                .get(table)
                .and_then(|t| t.as_table())
                .map(|t| t.keys().collect())
                .unwrap_or_default();
            if let Some(Value::Map(all)) = roots.get(kind).cloned() {
                let mut map = all.clone();
                for n in own {
                    if let Some(v) = all.get(&format!("{name}.{n}")) {
                        map.insert(n.clone(), v.clone());
                    }
                }
                roots.insert(kind.to_string(), Value::Map(map));
            }
        }
        roots.insert("input".into(), Value::Map(input));
        // Its locals, in the order they need each other (a few passes).
        let locals = body
            .get("locals")
            .and_then(|l| l.as_table())
            .cloned()
            .unwrap_or_default();
        let mut done: BTreeMap<String, Value> = BTreeMap::new();
        for _ in 0..=locals.len() {
            let mut progressed = false;
            for (k, v) in &locals {
                if done.contains_key(k) {
                    continue;
                }
                let mut r = roots.clone();
                r.insert("local".into(), Value::Map(done.clone()));
                let value = match v {
                    toml::Value::String(s) if has_expression(s) => eval_str(s, &MapScope(r)),
                    v => Ok(Value::from_toml(v)),
                };
                if let Ok(val) = value {
                    done.insert(k.clone(), val);
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }
        for k in locals.keys() {
            if !done.contains_key(k) {
                diags.push(self.diag_at(
                    at,
                    format!("modules.{name}: local `{k}` can't be computed (a name it reads, or a circle)"),
                ));
            }
        }
        roots.insert("local".into(), Value::Map(done));
        MapScope(roots)
    }

    /// An instance's resources, read in its scope, as the deployment's:
    /// `kind table → "<instance>.<n>" → table`. References to its own
    /// resources become qualified, and a contract's Wasm file is found next
    /// to the module file.
    pub(crate) fn module_resources(
        &self,
        env: &str,
        inst: &Instance,
        diags: &mut Vec<Diagnostic>,
        deferred: &mut Vec<String>,
    ) -> BTreeMap<&'static str, toml::Table> {
        let own = |table: &str| -> BTreeSet<String> {
            inst.body
                .get(table)
                .and_then(|t| t.as_table())
                .map(|t| t.keys().cloned().collect())
                .unwrap_or_default()
        };
        let (accounts, tokens) = (own("accounts"), own("tokens"));
        let q = |n: &str| format!("{}.{n}", inst.name);
        let qualify_ref = |r: &str| -> String {
            for (table, kind) in KINDS {
                if let Some(n) = r.strip_prefix(&format!("{kind}.")) {
                    if own(table).contains(n) {
                        return format!("module.{}.{kind}.{n}", inst.name);
                    }
                }
            }
            r.to_string()
        };
        let dir = inst
            .file
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default();
        let mut out: BTreeMap<&'static str, toml::Table> = BTreeMap::new();
        for (table, _) in KINDS {
            let Some(t) = inst.body.get(table).and_then(|t| t.as_table()) else {
                continue;
            };
            for (n, v) in t {
                if n.contains('.') {
                    diags.push(Diagnostic::new(format!(
                        "{}: {table}.{n}: a name can't hold '.'",
                        inst.file.display()
                    )));
                    continue;
                }
                let path = format!("env.{env}.{table}.{}.{n}", inst.name);
                let Some(toml::Value::Table(mut r)) =
                    self.eval_value(v, &inst.scope, &path, diags, deferred)
                else {
                    continue;
                };
                match table {
                    "accounts" => {
                        r.entry("identity")
                            .or_insert_with(|| toml::Value::String(format!("{}-{n}", inst.name)));
                        if let Some(toml::Value::Array(lines)) = r.get_mut("trustlines") {
                            for l in lines.iter_mut() {
                                if let Some(s) = l.as_str().filter(|s| tokens.contains(*s)) {
                                    *l = toml::Value::String(q(s));
                                }
                            }
                        }
                        if let Some(toml::Value::Table(b)) = r.get_mut("balances") {
                            *b = std::mem::take(b)
                                .into_iter()
                                .map(|(k, v)| {
                                    if tokens.contains(&k) {
                                        (q(&k), v)
                                    } else {
                                        (k, v)
                                    }
                                })
                                .collect();
                        }
                    }
                    "tokens" => {
                        if let Some(toml::Value::String(i)) = r.get_mut("issuer") {
                            if accounts.contains(i.as_str()) {
                                *i = q(i);
                            }
                        }
                    }
                    _ => {
                        if let Some(toml::Value::String(d)) = r.get_mut("deployer") {
                            if accounts.contains(d.as_str()) {
                                *d = q(d);
                            }
                        }
                        if let Some(toml::Value::String(w)) = r.get_mut("wasm") {
                            let hash = w.len() == 64 && w.chars().all(|c| c.is_ascii_hexdigit());
                            if !hash {
                                *w = dir.join(&*w).display().to_string();
                            }
                        }
                    }
                }
                if let Some(toml::Value::Array(deps)) = r.get_mut("depends_on") {
                    for dep in deps.iter_mut() {
                        if let Some(s) = dep.as_str() {
                            *dep = toml::Value::String(qualify_ref(s));
                        }
                    }
                }
                out.entry(table)
                    .or_default()
                    .insert(q(n), toml::Value::Table(r));
            }
        }
        out
    }

    /// An instance's outputs, by name.
    pub(crate) fn module_outputs(
        &self,
        inst: &Instance,
        diags: &mut Vec<Diagnostic>,
    ) -> BTreeMap<String, Value> {
        let mut out = BTreeMap::new();
        let Some(t) = inst.body.get("outputs").and_then(|o| o.as_table()) else {
            return out;
        };
        for (k, v) in t {
            let v = match v {
                toml::Value::Table(t) => t
                    .get("value")
                    .cloned()
                    .unwrap_or(toml::Value::Boolean(false)),
                v => v.clone(),
            };
            let value = match &v {
                toml::Value::String(s) if has_expression(s) => match eval_str(s, &inst.scope) {
                    Ok(v) => v,
                    Err(e) => {
                        diags.push(Diagnostic::new(format!(
                            "{}: output `{k}`: {}",
                            inst.file.display(),
                            e.message
                        )));
                        continue;
                    }
                },
                v => Value::from_toml(v),
            };
            out.insert(k.clone(), value);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::expr::Value;
    use crate::{Inputs, LaneDoc};

    const MODULE: &str = r#"
[inputs.symbol]
type = "string"

[inputs.decimals]
type = "integer"
default = 7

[locals]
code = "${upper(input.symbol)}"

[accounts.feeder]
trustlines = ["coin"]
balances = { coin = "10" }

[tokens.coin]
code = "${local.code}"
issuer = "feeder"

[contracts.oracle]
wasm = "oracle.wasm"
deployer = "feeder"
depends_on = ["token.coin", "account.admin"]
args = { feeder = "${account.feeder.public_key}", decimals = "${input.decimals}" }

[outputs]
oracle = "${contract.oracle.address}"
code = "${local.code}"
"#;

    fn lane(env: &str) -> String {
        format!("[lane]\nname = \"t\"\n\n[env.local]\nnetwork = \"local\"\n{env}\n\n[outputs]\nbtc = \"${{module.btc.code}}\"\n")
    }

    /// The first read's names: the deployment's resources are known later.
    fn later() -> BTreeMap<String, Value> {
        ["account", "token", "contract", "network"]
            .iter()
            .map(|r| {
                (
                    r.to_string(),
                    Value::Deferred(std::collections::BTreeSet::from([r.to_string()])),
                )
            })
            .collect()
    }

    fn setup(env: &str, module: &str) -> (tempfile::TempDir, LaneDoc) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("modules")).unwrap();
        std::fs::write(dir.path().join("modules/oracle.toml"), module).unwrap();
        std::fs::write(dir.path().join("lane.toml"), lane(env)).unwrap();
        let doc = LaneDoc::load(&dir.path().join("lane.toml")).unwrap();
        (dir, doc)
    }

    #[test]
    fn a_modules_resources_join_the_deployment() {
        let (dir, doc) = setup(
            "[env.local.modules.btc]\nsource = \"modules/oracle.toml\"\ninputs = { symbol = \"btc\" }\n",
            MODULE,
        );
        let r = doc
            .resolve_env_with("local", &Inputs::default(), &later())
            .unwrap();
        let t = &r.table;
        assert!(t.get("modules").is_none());
        let feeder = &t["accounts"]["btc.feeder"];
        assert_eq!(feeder["identity"].as_str(), Some("btc-feeder"));
        // Its own token, by its qualified name.
        assert_eq!(feeder["trustlines"][0].as_str(), Some("btc.coin"));
        assert!(feeder["balances"].get("btc.coin").is_some());
        let coin = &t["tokens"]["btc.coin"];
        assert_eq!(coin["code"].as_str(), Some("BTC"));
        assert_eq!(coin["issuer"].as_str(), Some("btc.feeder"));
        let oracle = &t["contracts"]["btc.oracle"];
        assert_eq!(oracle["deployer"].as_str(), Some("btc.feeder"));
        assert_eq!(
            oracle["wasm"].as_str().unwrap(),
            dir.path().join("modules/oracle.wasm").display().to_string()
        );
        assert_eq!(
            oracle["depends_on"].as_array().unwrap(),
            &vec![
                toml::Value::String("module.btc.token.coin".into()),
                toml::Value::String("account.admin".into()),
            ]
        );
        // Values known only with the keys wait, at the merged path.
        assert!(r
            .deferred
            .contains(&"contracts.btc.oracle.args.feeder".to_string()));
        assert_eq!(oracle["args"]["decimals"].as_integer(), Some(7));
    }

    #[test]
    fn with_the_keys_its_names_are_its_own_and_its_outputs_are_read() {
        let (_dir, doc) = setup(
            "[env.local.modules.btc]\nsource = \"modules/oracle.toml\"\ninputs = { symbol = \"btc\", decimals = 8 }\n",
            MODULE,
        );
        // Stage 2: the deployment's attributes, with the module's account
        // under its qualified name.
        let account = Value::Map(BTreeMap::from([(
            "btc.feeder".to_string(),
            Value::Map(BTreeMap::from([(
                "public_key".to_string(),
                Value::Str("GFEEDER".into()),
            )])),
        )]));
        let contract = Value::Map(BTreeMap::from([(
            "btc.oracle".to_string(),
            Value::Map(BTreeMap::from([(
                "address".to_string(),
                Value::Str("CORACLE".into()),
            )])),
        )]));
        let extra = BTreeMap::from([
            ("account".to_string(), account),
            ("contract".to_string(), contract),
        ]);
        let r = doc
            .resolve_env_with("local", &Inputs::default(), &extra)
            .unwrap();
        let args = &r.table["contracts"]["btc.oracle"]["args"];
        assert_eq!(args["feeder"].as_str(), Some("GFEEDER"));
        assert_eq!(args["decimals"].as_integer(), Some(8));
        let outputs = doc.outputs("local", &Inputs::default(), &extra).unwrap();
        assert_eq!(outputs[0].name, "btc");
        assert_eq!(outputs[0].value, Value::Str("BTC".into()));
    }

    #[test]
    fn for_each_makes_one_instance_per_item() {
        let (_dir, doc) = setup(
            "[env.local.modules.oracle]\nsource = \"modules/oracle.toml\"\nfor_each = \"${['btc', 'eth']}\"\ninputs = { symbol = \"${each.value}\" }\n",
            MODULE,
        );
        let r = doc
            .resolve_env_with("local", &Inputs::default(), &later())
            .unwrap();
        let tokens = r.table["tokens"].as_table().unwrap();
        assert_eq!(
            tokens.keys().collect::<Vec<_>>(),
            ["oracle-btc.coin", "oracle-eth.coin"]
        );
        assert_eq!(tokens["oracle-eth.coin"]["code"].as_str(), Some("ETH"));
    }

    #[test]
    fn what_a_module_refuses() {
        let err = |env: &str, module: &str| {
            let (_dir, doc) = setup(env, module);
            doc.resolve_env_with("local", &Inputs::default(), &later())
                .unwrap_err()
                .to_string()
        };
        let src = "[env.local.modules.btc]\nsource = \"modules/oracle.toml\"\n";
        let e = err(
            &format!("{src}inputs = {{ symbol = \"btc\", decimal = 8 }}\n"),
            MODULE,
        );
        assert!(
            e.contains("declares no input `decimal`") && e.contains("did you mean `decimals`"),
            "{e}"
        );
        assert!(err(src, MODULE).contains("needs input `symbol`"));
        let e = err(&format!("{src}inputs = {{ symbol = 1 }}\n"), MODULE);
        assert!(e.contains("inputs.symbol must be a string"), "{e}");
        assert!(err(
            &format!("{src}inputs = {{ symbol = \"x\" }}\n"),
            "[vars.x]\ntype = \"string\"\n"
        )
        .contains("a module file has only"));
        assert!(err(
            "[env.local.modules.btc]\nsource = \"https://example.com/m.toml\"\n",
            MODULE
        )
        .contains("no remote sources"));
        assert!(err(
            "[env.local.modules.btc]\nsource = \"modules/none.toml\"\n",
            MODULE
        )
        .contains("reading"));
        assert!(err("[env.local.accounts.\"a.b\"]\n", MODULE).contains("a name can't hold '.'"));
    }
}
