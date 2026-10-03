//! What a deployment's expressions can read once its keys and addresses are
//! known (M0.6, C-10, DEC-082): relayer feeds and `[outputs]` use them.
//!
//! | Name | Fields |
//! |---|---|
//! | `lane` | `name`, `template`, `id`, `engine_wasm_hash`, `config_hash`, `genesis_state_hash` |
//! | `network` | `name`, `passphrase`, `rpc_url` |
//! | `account.admin`, `account.relayer` | `identity`, `public_key` |
//! | `token.settlement` | `address`, `asset` (`CODE:G…` or null) |
//! | `contract.settlement` | `address`, `pinned` |
//! | `node.sequencer` | `url` (where users reach it, or null), `port` |
//! | `node.validator-<name>` | `name`, `key`, `url`, `port`, `weight` |
//! | `validators` | the same, as a list in file order |
//! | `signers.settlement` | `threshold`, `count` |
//! | `release` | `commit` |
//!
//! Before they are known (while the deployment is read), each of these is a
//! deferred value, and only relayer feeds may use one.

use std::collections::{BTreeMap, BTreeSet};

use caravel_lanefile::expr::Value;

use crate::address::strkey;
use crate::deploy::{api_url, validator_url, Addresses, Keys};
use crate::manifest::Manifest;
use crate::render::validator_port;
use crate::template::GenesisHashes;

/// The roots known only after keys and addresses.
pub const LATER: [&str; 8] = [
    "account",
    "contract",
    "network",
    "node",
    "release",
    "signers",
    "token",
    "validators",
];

/// Where deferred values may be used: they are filled in before the files
/// are written.
pub const DEFERRED_OK: [&str; 1] = ["relayer.feeds"];

fn hex(k: &[u8]) -> String {
    k.iter().map(|b| format!("{b:02x}")).collect()
}

fn s(v: impl Into<String>) -> Value {
    Value::Str(v.into())
}

fn map<const N: usize>(entries: [(&str, Value); N]) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

fn later(name: &str) -> Value {
    Value::Deferred(BTreeSet::from([name.to_string()]))
}

/// The names for the first read of a deployment: `lane` as far as the lane
/// file knows it, and every other root deferred.
pub fn before(
    lane_name: &str,
    template: &str,
    lane_id: &[u8; 32],
    engine: &str,
) -> BTreeMap<String, Value> {
    let mut out: BTreeMap<String, Value> =
        LATER.iter().map(|r| (r.to_string(), later(r))).collect();
    out.insert(
        "lane".into(),
        map([
            ("name", s(lane_name)),
            ("template", s(template)),
            ("id", s(hex(lane_id))),
            ("engine_wasm_hash", s(engine)),
            ("config_hash", later("lane.config_hash")),
            ("genesis_state_hash", later("lane.genesis_state_hash")),
        ]),
    );
    out
}

/// Every name, once keys and addresses are known (and genesis and the
/// release, if given).
pub fn attributes(
    m: &Manifest,
    keys: &Keys,
    a: &Addresses,
    genesis: Option<&GenesisHashes>,
    release: Option<&str>,
) -> BTreeMap<String, Value> {
    let g = |k: &[u8; 32]| caravel_runtime::views::g_address(k);
    let template = m
        .lane
        .app
        .as_ref()
        .map(|a| a.template.clone())
        .unwrap_or_default();
    let engine = m
        .lane
        .app
        .as_ref()
        .map(|a| a.engine_wasm_sha256.clone())
        .unwrap_or_default();
    let validators: Vec<Value> = m
        .env
        .validators
        .iter()
        .zip(&keys.validators)
        .enumerate()
        .map(|(i, (v, k))| {
            map([
                ("name", s(&v.name)),
                ("key", s(g(k))),
                (
                    "url",
                    validator_url(m, i).map(Value::Str).unwrap_or(Value::Null),
                ),
                ("port", Value::Int(i64::from(validator_port(m, i)))),
                ("weight", Value::Int(i64::from(v.weight))),
            ])
        })
        .collect();
    let mut nodes = BTreeMap::from([(
        "sequencer".to_string(),
        map([
            ("url", api_url(m).map(Value::Str).unwrap_or(Value::Null)),
            ("port", Value::Int(i64::from(m.env.sequencer.port))),
        ]),
    )]);
    for (v, val) in m.env.validators.iter().zip(&validators) {
        nodes.insert(format!("validator-{}", v.name), val.clone());
    }
    let account = |identity: &str, key: &[u8; 32]| {
        map([("identity", s(identity)), ("public_key", s(g(key)))])
    };
    BTreeMap::from([
        (
            "lane".to_string(),
            map([
                ("name", s(&m.lane.lane.name)),
                ("template", s(template)),
                ("id", s(hex(&m.lane.lane_id()))),
                ("engine_wasm_hash", s(engine)),
                (
                    "config_hash",
                    genesis.map_or(later("lane.config_hash"), |h| s(hex(&h.config_hash))),
                ),
                (
                    "genesis_state_hash",
                    genesis.map_or(later("lane.genesis_state_hash"), |h| {
                        s(hex(&h.genesis_state_hash))
                    }),
                ),
            ]),
        ),
        (
            "network".to_string(),
            map([
                ("name", s(m.env.network.name())),
                ("passphrase", s(m.env.network.passphrase())),
                ("rpc_url", s(m.rpc_url())),
            ]),
        ),
        ("account".to_string(), {
            let mut accounts = BTreeMap::from([
                ("admin".to_string(), account(&m.env.admin, &keys.admin)),
                (
                    "relayer".to_string(),
                    account(&m.env.relayer.account, &keys.relayer),
                ),
            ]);
            // Declared accounts (C-18).
            for (name, spec) in &m.env.accounts {
                if let Some(k) = keys.accounts.get(name) {
                    accounts.insert(name.clone(), account(spec.identity_of(name), k));
                }
            }
            Value::Map(accounts)
        }),
        (
            "token".to_string(),
            map([(
                "settlement",
                map([
                    ("address", s(strkey(&a.token))),
                    (
                        "asset",
                        a.token_asset
                            .as_ref()
                            .map_or(Value::Null, |(code, issuer)| {
                                s(format!("{code}:{}", g(issuer)))
                            }),
                    ),
                ]),
            )]),
        ),
        (
            "contract".to_string(),
            map([(
                "settlement",
                map([
                    ("address", s(strkey(&a.settlement))),
                    ("pinned", Value::Bool(a.settlement_pinned)),
                ]),
            )]),
        ),
        ("node".to_string(), Value::Map(nodes)),
        ("validators".to_string(), Value::List(validators)),
        (
            "signers".to_string(),
            map([(
                "settlement",
                map([
                    ("threshold", Value::Int(i64::from(m.env.threshold))),
                    ("count", Value::Int(m.env.validators.len() as i64)),
                ]),
            )]),
        ),
        (
            "release".to_string(),
            map([("commit", release.map_or(later("release.commit"), s))]),
        ),
    ])
}

/// A value as JSON, for `--json`.
pub fn json(v: &Value) -> serde_json::Value {
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => (*b).into(),
        Value::Int(i) => (*i).into(),
        Value::Str(s) => s.clone().into(),
        Value::List(l) => l.iter().map(json).collect(),
        Value::Map(m) => m.iter().map(|(k, v)| (k.clone(), json(v))).collect(),
        Value::Opaque(t) => serde_json::to_value(t).unwrap_or(serde_json::Value::Null),
        Value::Deferred(r) => format!(
            "(known later: {})",
            r.iter().cloned().collect::<Vec<_>>().join(", ")
        )
        .into(),
    }
}
