//! Perps lane files (spec §10.3, DEC-054): the platform layout in
//! `lanes/perps/config/` and the M0 layout lane #1's M0 nodes read
//! (`tests/fixtures/*.m0.toml`) give the same `GenesisConfigV1` bytes, so
//! lane #1 keeps its `lane_id`, `config_hash` and `genesis_state_hash`.

use std::path::PathBuf;

use caravel_node::lane_toml::{self, LaneFile};
use caravel_node::NodeApp;
use caravel_perps_node::{lane_file, PerpsApp};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn text(path: &str) -> String {
    std::fs::read_to_string(root().join(path)).unwrap()
}

const TESTNET: &str = "lanes/perps/config/lane.caravel-perps.testnet.toml";
const LOCAL: &str = "lanes/perps/config/lane.caravel-perps.local.toml";
const TESTNET_M0: &str = "lanes/perps/node/tests/fixtures/lane.caravel-perps.testnet.m0.toml";
const LOCAL_M0: &str = "lanes/perps/node/tests/fixtures/lane.caravel-perps.local.m0.toml";

fn genesis(t: &str) -> anyhow::Result<(lane_toml::GenesisReport, Vec<u8>, Vec<u8>)> {
    lane_toml::genesis(&PerpsApp, &LaneFile::parse(t)?)
}

#[test]
fn lane_1_keeps_its_hashes() {
    let (report, config, state) = genesis(&text(TESTNET)).unwrap();
    assert_eq!(report.lane_name, "caravel-perps-testnet-0");
    assert_eq!(
        report.lane_id,
        "319b46e9804746739b1e177fae8d79df8f013ec86dde16eaff6c83b374531047"
    );
    assert_eq!(
        report.config_hash,
        "f4b9db09137583ba9d66ea0b8a3a2b658a163f7b72993e0f242f04ea3ac93997"
    );
    assert_eq!(
        report.genesis_state_hash,
        "22702d9f4c45f88306aca02169cf7a86ccaec3b45ed85103c7a9fc4277291e77"
    );
    assert_eq!(report.config_bytes, config.len());
    assert_eq!(report.state_bytes, state.len());
}

/// The local perps lane's hashes, pinned before the lane file grows an
/// expression layer (M0.6): resolving a file must never move them.
#[test]
fn the_local_lane_keeps_its_hashes() {
    let (report, _, _) = genesis(&text(LOCAL)).unwrap();
    assert_eq!(report.lane_name, "caravel-perps-local-0");
    assert_eq!(
        report.lane_id,
        "dc6edb91328cbdf76d69369788153d06cfa9efddcb1cfcb239c14c6029afe0cb"
    );
    assert_eq!(
        report.config_hash,
        "81442ee3fcc06437dba0ee342260a9654c895b510952c40ed369f92ae3e2b70b"
    );
    assert_eq!(
        report.genesis_state_hash,
        "6b079675e579addd0d831835c30ace3d5f562323662836b89cf855ff9b8a8a78"
    );
}

#[test]
fn both_layouts_give_the_same_bytes() {
    for (new, m0) in [(TESTNET, TESTNET_M0), (LOCAL, LOCAL_M0)] {
        let (_, config_new, state_new) = genesis(&text(new)).unwrap();
        let (_, config_m0, state_m0) = genesis(&text(m0)).unwrap();
        assert_eq!(config_new, config_m0, "{new}");
        assert_eq!(state_new, state_m0, "{new}");
        assert!(LaneFile::parse(&text(m0)).unwrap().app.is_none());
    }
}

/// The TOML and the §10.3 fixture in caravel-types agree on every number;
/// only the keys and the benchmarked caps differ.
#[test]
fn testnet_numbers_match_the_section_10_3_fixture() {
    let mut from_toml =
        lane_file::genesis_config(&LaneFile::parse(&text(TESTNET)).unwrap()).unwrap();
    let mut fixture = caravel_types::vectors::config();
    // The codec fixture keeps the v0.1.0 caps; the lane runs the caps the
    // T-005 benchmark chose (DEC-028).
    fixture.max_accounts = 256;
    fixture.max_orders_per_side = 128;
    fixture.max_block_bytes = 12_000;
    fixture.exec_cpu_limit = 200_000_000;
    from_toml.backstop_key = fixture.backstop_key;
    from_toml.treasury_key = fixture.treasury_key;
    from_toml.oracle_keys = fixture.oracle_keys.clone();
    assert_eq!(from_toml, fixture);
}

#[test]
fn rejects_bad_files() {
    for path in [TESTNET, TESTNET_M0] {
        let t = text(path);
        let bad = |t: &str| genesis(t).is_err();
        assert!(
            bad(&t.replace("mode = \"open\"", "mode = \"closed\"")),
            "{path}"
        );
        assert!(
            bad(&t.replace("block_time_ms = 1000", "block_time_ms = 100")),
            "{path}"
        );
        assert!(
            bad(&t.replace("imf_bps = 2000", "imf_bps = 900")),
            "{path}: mmf 1000 must be below imf"
        );
        assert!(bad(&t.replace(
            "GAG5OK7U7GLFB2BQWIZPNSPVOTITXV3AJBQAHZJWT6RFABBONNLJES7U",
            "GBAD"
        )));
        assert!(
            bad(&format!("{t}\n[extra]\nx = 1\n")),
            "{path}: unknown sections are rejected"
        );
        assert!(bad(&t.replace("max_orders_per_side = 128\n", "")), "{path}");
    }
    // Platform layout only: the perps limits are not platform limits, and
    // the file must be for this app.
    let t = text(TESTNET);
    assert!(genesis(&t.replace(
        "max_session_keys = 4\n",
        "max_session_keys = 4\nmax_orders_per_side = 128\n"
    ))
    .is_err());
    assert!(genesis(&t.replace("template = \"perps\"", "template = \"payments\"")).is_err());
}

#[test]
fn a_node_refuses_an_engine_the_lane_file_does_not_name() {
    let lane = LaneFile::parse(&text(TESTNET)).unwrap();
    let want = lane.engine_wasm_hash().unwrap().unwrap();
    assert!(lane.check_engine(&want).is_ok());
    assert!(lane.check_engine(&[0; 32]).is_err());
    // The engine of record is the one versions.json records.
    let versions: serde_json::Value = serde_json::from_str(&text("versions.json")).unwrap();
    let record = versions["artifacts"]["engine_wasm_sha256"]
        .as_str()
        .unwrap();
    assert_eq!(caravel_runtime::sequencer::hex(&want), record);
    assert_eq!(PerpsApp::TEMPLATE, "perps");
}

/// A deployment block, as `[env.<name>]` tables look in a manifest.
const ENV_BLOCK: &str = r#"
[env.other]
network = "testnet"
admin = "caravel-admin"
usdc = "circle"
threshold = 2
[env.other.settlement_params]
force_inclusion_window_secs = 3600
[[env.other.validators]]
name = "v1"
key = "caravel-validator-1"
[env.other.host]
provider = "ssh"
address = "caravel@example.invalid"
[env.other2]
network = "local"
"#;

/// `[env]` is not consensus: every perps lane file, in both layouts, gives the
/// same genesis bytes with a deployment block as without one.
#[test]
fn env_tables_change_no_hash() {
    for path in [TESTNET, LOCAL, TESTNET_M0, LOCAL_M0] {
        let plain = text(path);
        let (report, config, state) = genesis(&plain).unwrap();
        let (report_env, config_env, state_env) =
            genesis(&format!("{plain}\n{ENV_BLOCK}")).unwrap();
        assert_eq!((config_env, state_env), (config, state), "{path}");
        assert_eq!(report_env.config_hash, report.config_hash, "{path}");
    }
}

/// Through the lane file language (M0.6, caravel_lanefile): the same lane
/// file, deployments and hashes as the plain parser gives.
#[test]
fn the_lane_file_language_changes_no_file() {
    for path in [TESTNET, LOCAL, TESTNET_M0, LOCAL_M0] {
        let plain = LaneFile::parse(&text(path)).unwrap();
        let loaded = caravel_deploy::manifest::load_lane(&root().join(path)).unwrap();
        assert_eq!(loaded.raw, plain.raw, "{path}");
        assert_eq!(loaded.env, plain.env, "{path}");
        let (a, ca, sa) = lane_toml::genesis(&PerpsApp, &loaded).unwrap();
        let (b, cb, sb) = lane_toml::genesis(&PerpsApp, &plain).unwrap();
        assert_eq!((a.config_hash, ca, sa), (b.config_hash, cb, sb), "{path}");
    }
}
