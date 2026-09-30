//! The ssh provider's host files for lane #1 (DEC-069, DEC-070): from the
//! `[env.testnet]` table in its lane file, the generated systemd units and
//! Caddyfile are byte for byte the ones its VM runs, kept as copies in
//! `lanes/perps/deploy/testnet/`.

use std::path::PathBuf;

use caravel_deploy::manifest::Manifest;
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
