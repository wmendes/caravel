//! The ssh provider's host files for lane #1 (DEC-069, DEC-070): from the
//! `[env.testnet]` table in its lane file, the generated systemd units and
//! Caddyfile are byte for byte the ones its VM runs, kept as copies in
//! `lanes/perps/deploy/testnet/`. Every other file it renders, and the
//! deployment values that reach Stellar, are pinned too (M0.6 baseline,
//! `tests/golden/render-testnet/`; `UPDATE_GOLDEN=1`): a change to the lane
//! file's language must leave lane #1's `plan` at "No changes.".

use std::path::PathBuf;

use caravel_deploy::manifest::{Manifest, Network, SettlementParams, Token};
use caravel_deploy::render::{render, Resolved};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// Lane #1's deployment, from its lane file (P-16).
fn manifest() -> Manifest {
    Manifest::load(
        &root().join("lanes/perps/config/lane.caravel-perps.testnet.toml"),
        "testnet",
    )
    .unwrap()
}

fn files() -> std::collections::BTreeMap<String, String> {
    let m = manifest();
    let r = Resolved {
        template: "perps".into(),
        engine_wasm_hash: m.lane.engine_wasm_hash().unwrap().unwrap(),
        settlement: stellar_strkey::Contract::from_string(
            "CBIHBEUZYFZQZEQPBJH2ID6CDRDZFEDI6XHAXVOCHG6FO5XWUIGPONWO",
        )
        .unwrap()
        .0,
        validator_keys: vec![[1; 32], [2; 32], [3; 32]],
        web: true,
    };
    render(&m, &r, "/opt/caravel", 1).unwrap()
}

#[test]
fn the_units_are_lane_1s() {
    let f = files();
    for unit in [
        "caravel-sequencer.service",
        "caravel-validator@.service",
        "caravel-relayer.service",
    ] {
        let have = &f[&format!("systemd/{unit}")];
        let want =
            std::fs::read_to_string(root().join("lanes/perps/deploy/testnet/systemd").join(unit))
                .unwrap();
        assert!(*have == want, "{unit} differs:\n{have}");
    }
}

#[test]
fn the_caddyfile_is_lane_1s() {
    // Lane #1's VM runs the generated Caddyfile since P-16; the file here is a copy.
    let f = files();
    let want =
        std::fs::read_to_string(root().join("lanes/perps/deploy/testnet/Caddyfile")).unwrap();
    assert!(
        f["caddy/Caddyfile"] == want,
        "Caddyfile differs:\n{}",
        f["caddy/Caddyfile"]
    );
}

#[test]
fn every_rendered_file_is_pinned() {
    let files = files();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/render-testnet");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        for (name, text) in &files {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        return;
    }
    let mut pinned = Vec::new();
    for entry in walk(&dir) {
        pinned.push(entry.strip_prefix(&dir).unwrap().display().to_string());
    }
    pinned.sort();
    assert_eq!(
        pinned,
        files.keys().cloned().collect::<Vec<_>>(),
        "the set of rendered files changed (UPDATE_GOLDEN=1)"
    );
    for (name, text) in &files {
        let want = std::fs::read_to_string(dir.join(name)).unwrap();
        assert!(*text == want, "{name} changed:\n{text}");
    }
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// What lane #1's deployment puts on Stellar: the constructor's fields and
/// the signer set. Any change here is a plan problem on the live lane.
#[test]
fn the_values_on_stellar_are_lane_1s() {
    let m = manifest();
    let e = &m.env;
    assert_eq!(e.network, Network::Testnet);
    assert_eq!(e.admin, "caravel-admin");
    assert_eq!(e.token, Token::Named("circle-usdc".into()));
    assert_eq!(
        e.settlement.as_deref(),
        Some("CBIHBEUZYFZQZEQPBJH2ID6CDRDZFEDI6XHAXVOCHG6FO5XWUIGPONWO")
    );
    assert_eq!(
        e.settlement_wasm.as_deref(),
        Some("8a2fafbd1ad48d53333ab79d73afa1c50d5f790f39a97fafb88b5443b383d503")
    );
    assert_eq!(
        e.settlement_params,
        SettlementParams {
            force_inclusion_window_secs: 3600,
            escape_timeout_secs: 21600,
            min_rotation_delay_secs: 3600,
            signer_retention_epochs: 2,
            min_deposit: None,
        }
    );
    assert_eq!(m.min_deposit().unwrap(), 10_000_000);
    assert_eq!(e.threshold, 2);
    let validators: Vec<_> = e
        .validators
        .iter()
        .map(|v| {
            (
                v.name.as_str(),
                v.key.as_str(),
                v.weight,
                v.port(e.sequencer.port),
            )
        })
        .collect();
    assert_eq!(
        validators,
        [
            ("1", "caravel-validator-1", 1, Some(8081)),
            ("2", "caravel-validator-2", 1, Some(8082)),
            ("3", "caravel-validator-3", 1, Some(8083)),
        ]
    );
    assert_eq!(e.relayer.account, "caravel-relayer");
    assert_eq!(
        e.relayer
            .feed_keys
            .get("CARAVEL_ORACLE_SECRET")
            .map(String::as_str),
        Some("caravel-oracle")
    );
}
