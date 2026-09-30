//! The ssh provider's host files for lane #1 (DEC-069): with lane #1's
//! deployment written as an `[env.testnet]` table, the generated systemd units
//! are byte for byte the ones its VM runs (`lanes/perps/deploy/testnet/systemd/`),
//! and the Caddyfile routes the same paths.

use std::path::PathBuf;

use caravel_deploy::manifest::Manifest;
use caravel_deploy::render::{render, Resolved};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// Lane #1's deployment, as P-16 writes it into its lane file.
pub const LANE_1_ENV: &str = r#"
[env.testnet]
network = "testnet"
admin = "caravel-admin"
usdc = "circle"
settlement = "CBIHBEUZYFZQZEQPBJH2ID6CDRDZFEDI6XHAXVOCHG6FO5XWUIGPONWO"
settlement_wasm = "8a2fafbd1ad48d53333ab79d73afa1c50d5f790f39a97fafb88b5443b383d503"
threshold = 2

[env.testnet.settlement_params]
force_inclusion_window_secs = 3600
escape_timeout_secs = 21600
min_rotation_delay_secs = 3600
signer_retention_epochs = 2

[[env.testnet.validators]]
name = "1"
key = "caravel-validator-1"

[[env.testnet.validators]]
name = "2"
key = "caravel-validator-2"

[[env.testnet.validators]]
name = "3"
key = "caravel-validator-3"

[env.testnet.sequencer]
port = 8080
production = true

[env.testnet.validator_polling]
sequencer_ms = 200
stellar_secs = 30

[env.testnet.relayer]
account = "caravel-relayer"
feed_keys = { CARAVEL_ORACLE_SECRET = "caravel-oracle" }
intervals_ms = { inbox = 3000, checkpoints = 2000 }

[env.testnet.host]
provider = "ssh"
transport = "gcloud-iap"
address = "caravel-1"
project = "caravel-testnet"
zone = "us-central1-a"
public_url = "https://35-224-76-64.sslip.io"
"#;

fn manifest() -> Manifest {
    let text =
        std::fs::read_to_string(root().join("lanes/perps/config/lane.caravel-perps.testnet.toml"))
            .unwrap();
    Manifest::parse(&format!("{text}{LANE_1_ENV}"), "testnet").unwrap()
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
fn the_caddyfile_routes_what_lane_1s_does() {
    let f = files();
    let have = &f["caddy/Caddyfile"];
    let want = std::fs::read_to_string(root().join("lanes/perps/deploy/testnet/Caddyfile"))
        .unwrap()
        .replace("{$CARAVEL_HOST}", "35-224-76-64.sslip.io");
    // The same site block, comments aside.
    let body = |t: &str| {
        t.lines()
            .filter(|l| !l.starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(body(have), body(&want));
    // A local host gets no units and no Caddyfile.
    assert!(f.keys().any(|k| k.starts_with("systemd/")));
}
