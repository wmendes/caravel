use std::collections::BTreeMap;
use std::path::Path;

use super::*;

const GENESIS: &str = "[lane]\nname = \"t\"\n[node]\nblock_time_ms = 1000\n";

/// Loads `files[0]` with the others readable next to it.
fn load(files: &[(&str, &str)]) -> Result<LaneDoc, Error> {
    let map: BTreeMap<String, String> = files
        .iter()
        .map(|(p, t)| (format!("lanes/{p}"), t.to_string()))
        .collect();
    let read = move |p: &Path| {
        map.get(&p.display().to_string())
            .cloned()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"))
    };
    LaneDoc::load_with(&Path::new("lanes").join(files[0].0), &read)
}

fn err(files: &[(&str, &str)]) -> String {
    load(files).unwrap_err().to_string()
}

fn env<'a>(d: &'a LaneDoc, name: &str) -> &'a toml::Table {
    &d.envs[name]
}

#[test]
fn a_plain_lane_file_is_itself() {
    let text = format!(
        "{GENESIS}[env.local]\nnetwork = \"local\"\n[env.local.host]\nprovider = \"local\"\n"
    );
    let d = load(&[("lane.toml", &text)]).unwrap();
    let plain: toml::Table = toml::from_str(&text).unwrap();
    assert_eq!(d.to_table(), plain);
    assert!(d.abstract_envs.is_empty());
}

#[test]
fn extends_merges_tables_and_replaces_the_rest() {
    let text = format!(
        "{GENESIS}
[env.base]
abstract = true
admin = \"acme-admin\"
threshold = 2
[env.base.host]
provider = \"ssh\"
address = \"box-1\"
zone = \"z\"
[[env.base.validators]]
name = \"1\"
key = \"v1\"
[[env.base.validators]]
name = \"2\"
key = \"v2\"

[env.testnet]
extends = \"base\"
default = true
network = \"testnet\"
host = {{ address = \"box-2\" }}
[[env.testnet.validators]]
name = \"9\"
key = \"v9\"
"
    );
    let d = load(&[("lane.toml", &text)]).unwrap();
    assert_eq!(d.abstract_envs.iter().collect::<Vec<_>>(), ["base"]);
    assert_eq!(d.envs.keys().collect::<Vec<_>>(), ["testnet"]);
    let t = env(&d, "testnet");
    assert_eq!(t["admin"].as_str(), Some("acme-admin"));
    assert_eq!(t["network"].as_str(), Some("testnet"));
    assert!(!t.contains_key("extends") && !t.contains_key("abstract"));
    // Tables merge key by key.
    let host = t["host"].as_table().unwrap();
    assert_eq!(host["provider"].as_str(), Some("ssh"));
    assert_eq!(host["address"].as_str(), Some("box-2"));
    assert_eq!(host["zone"].as_str(), Some("z"));
    // An array of tables is replaced whole.
    let v = t["validators"].as_array().unwrap();
    assert_eq!(v.len(), 1);
    assert_eq!(v[0]["name"].as_str(), Some("9"));
    // Where each value came from.
    let o = d.origin("env.testnet.admin").unwrap();
    assert_eq!(o.via.as_deref(), Some("base"));
    assert_eq!(
        d.locate("env.testnet.admin").unwrap(),
        "lanes/lane.toml:8:9 (from [env.base])"
    );
    assert_eq!(d.origin("env.testnet.network").unwrap().via, None);
    assert_eq!(
        d.origin("env.testnet.host.zone").unwrap().via.as_deref(),
        Some("base")
    );
    assert_eq!(d.origin("env.testnet.host.address").unwrap().via, None);
    assert_eq!(d.origin("env.testnet.validators[0].key").unwrap().via, None);
    assert!(d.origin("env.testnet.validators[1].key").is_none());
}

#[test]
fn extends_chains_and_lists_in_order() {
    let text = format!(
        "{GENESIS}
[env.a]
abstract = true
x = 1
y = 1
z = 1
[env.b]
abstract = true
extends = \"a\"
y = 2
[env.c]
abstract = true
z = 3
[env.d]
extends = [\"b\", \"c\"]
"
    );
    let d = load(&[("lane.toml", &text)]).unwrap();
    let t = env(&d, "d");
    assert_eq!(
        (
            t["x"].as_integer(),
            t["y"].as_integer(),
            t["z"].as_integer()
        ),
        (Some(1), Some(2), Some(3))
    );
    assert_eq!(d.origin("env.d.x").unwrap().via.as_deref(), Some("a"));
    assert_eq!(d.origin("env.d.z").unwrap().via.as_deref(), Some("c"));
}

#[test]
fn default_is_not_inherited() {
    let text = format!(
        "{GENESIS}[env.a]\ndefault = true\nnetwork = \"local\"\n[env.b]\nextends = \"a\"\n"
    );
    let d = load(&[("lane.toml", &text)]).unwrap();
    assert_eq!(env(&d, "a")["default"].as_bool(), Some(true));
    assert!(!env(&d, "b").contains_key("default"));
}

#[test]
fn bad_inheritance_is_reported_where_it_is() {
    let e = err(&[(
        "lane.toml",
        &format!("{GENESIS}[env.base]\nx = 1\n[env.testnet]\nextends = \"bse\"\n"),
    )]);
    assert!(
        e.contains("[env.testnet] extends \"bse\", which the lane file doesn't have"),
        "{e}"
    );
    assert!(e.contains("--> lanes/lane.toml:8:11"), "{e}");
    assert!(e.contains("help: did you mean \"base\"?"), "{e}");

    let e = err(&[(
        "lane.toml",
        &format!("{GENESIS}[env.a]\nextends = \"b\"\n[env.b]\nextends = \"a\"\n"),
    )]);
    assert!(e.contains("in a circle: a → b → a"), "{e}");

    let e = err(&[(
        "lane.toml",
        &format!("{GENESIS}[env.a]\nabstract = true\ndefault = true\n"),
    )]);
    assert!(e.contains("abstract, so it can't be the default"), "{e}");

    let e = err(&[("lane.toml", &format!("{GENESIS}[env.a]\nextends = 3\n"))]);
    assert!(e.contains("name a deployment"), "{e}");
}

#[test]
fn includes_bring_deployments() {
    let main = format!("include = [\"envs/shared.toml\"]\n{GENESIS}[env.local]\nextends = \"base\"\nnetwork = \"local\"\n");
    let shared = "include = \"more.toml\"\n[env.base]\nabstract = true\nadmin = \"a\"\n";
    let more = "[env.testnet]\nextends = \"base\"\nnetwork = \"testnet\"\n";
    let d = load(&[
        ("lane.toml", &main),
        ("envs/shared.toml", shared),
        ("envs/more.toml", more),
    ])
    .unwrap();
    assert_eq!(d.envs.keys().collect::<Vec<_>>(), ["local", "testnet"]);
    assert_eq!(env(&d, "testnet")["admin"].as_str(), Some("a"));
    assert!(!d.genesis.contains_key("include"));
    assert!(d
        .locate("env.testnet.admin")
        .unwrap()
        .starts_with("lanes/envs/shared.toml:4:"));
    assert_eq!(d.sources.0.len(), 3);
}

#[test]
fn bad_includes_are_reported() {
    let e = err(&[(
        "lane.toml",
        &format!("include = [\"nope.toml\"]\n{GENESIS}"),
    )]);
    assert!(e.contains("can't read lanes/nope.toml"), "{e}");
    assert!(e.contains("--> lanes/lane.toml:1:"), "{e}");

    let e = err(&[
        ("lane.toml", &format!("include = \"a.toml\"\n{GENESIS}")),
        ("a.toml", "include = \"lane.toml\"\n"),
    ]);
    assert!(e.contains("in a circle"), "{e}");

    let e = err(&[
        (
            "lane.toml",
            &format!("include = \"a.toml\"\n{GENESIS}[env.x]\nn = 1\n"),
        ),
        ("a.toml", "[env.x]\nn = 2\n"),
    ]);
    assert!(e.contains("[env.x] is defined twice"), "{e}");
    // Includes load first: the second definition is the one reported.
    assert!(e.contains("--> lanes/lane.toml:6:1"), "{e}");
    assert!(e.contains("also defined in lanes/a.toml:1"), "{e}");

    let e = err(&[
        ("lane.toml", &format!("include = \"a.toml\"\n{GENESIS}")),
        ("a.toml", "[lane]\nname = \"other\"\n"),
    ]);
    assert!(e.contains("[lane] is a genesis section"), "{e}");
    assert!(e.contains("--> lanes/a.toml:"), "{e}");

    let e = err(&[(
        "lane.toml",
        &format!("include = \"/etc/x.toml\"\n{GENESIS}"),
    )]);
    assert!(e.contains("relative to this file"), "{e}");
}

#[test]
fn reserved_keys_and_syntax_errors() {
    let e = err(&[("lane.toml", &format!("outputs = 1\n{GENESIS}"))]);
    assert!(e.contains("`outputs` must be a table"), "{e}");
    let e = err(&[("lane.toml", "[lane\nname = 1\n")]);
    assert!(e.contains("--> lanes/lane.toml:1:"), "{e}");
    // Every problem at once.
    let e = load(&[(
        "lane.toml",
        &format!("{GENESIS}[env.a]\nextends = \"x\"\n[env.b]\nextends = \"y\"\n"),
    )])
    .unwrap_err();
    assert_eq!(e.diagnostics.len(), 2);
}

#[test]
fn text_without_includes() {
    let d = LaneDoc::parse(&format!("{GENESIS}[env.a]\nn = 1\n")).unwrap();
    assert_eq!(d.envs.len(), 1);
    assert!(LaneDoc::parse(&format!("include = \"x.toml\"\n{GENESIS}")).is_err());
}
