//! The deploy tool's node configs for a real lane file (DEC-067): the local
//! payments lane's `[env.local]`, rendered for a host root. The nodes' own
//! loaders read every file, the rendered lane file gives the same genesis, and
//! the files are pinned (`tests/golden/render/`; `UPDATE_GOLDEN=1`).

use std::path::PathBuf;

use caravel_deploy::manifest::Manifest;
use caravel_deploy::render::{render, Resolved};
use caravel_node::lane_toml::{self, LaneFile};
use caravel_payments_node::PaymentsApp;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn manifest() -> Manifest {
    Manifest::load(
        &root().join("lanes/payments/config/lane.caravel-payments.local.toml"),
        "local",
    )
    .unwrap()
}

fn resolved(m: &Manifest) -> Resolved {
    Resolved {
        template: "payments".into(),
        engine_wasm_hash: m.lane.engine_wasm_hash().unwrap().unwrap(),
        settlement: [7; 32],
        validator_keys: vec![[0x61; 32], [0x62; 32], [0x63; 32]],
        web: false,
        sequencer_key: None,
        images: Default::default(),
    }
}

#[test]
fn the_rendered_lane_file_has_the_same_genesis() {
    let m = manifest();
    let files = render(&m, &resolved(&m), "/opt/caravel", 1).unwrap();
    let (_, config, state) = lane_toml::genesis(&PaymentsApp, &m.lane).unwrap();
    let rendered = LaneFile::parse(&files["lane.toml"]).unwrap();
    assert!(
        rendered.env.is_empty(),
        "the deployment tables stay on the operator's machine"
    );
    let (_, config2, state2) = lane_toml::genesis(&PaymentsApp, &rendered).unwrap();
    assert_eq!((config2, state2), (config, state));
}

#[test]
fn the_nodes_read_every_rendered_file() {
    let m = manifest();
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path().display().to_string();
    let files = render(&m, &resolved(&m), &r, 3).unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::create_dir_all(dir.path().join("keys")).unwrap();
    for (name, text) in &files {
        std::fs::write(dir.path().join("config").join(name), text).unwrap();
    }
    for v in ["1", "2", "3"] {
        let secret = stellar_strkey::ed25519::PrivateKey([v.as_bytes()[0]; 32]).to_string();
        std::fs::write(
            dir.path().join(format!("keys/validator-{v}.key")),
            secret.as_str(),
        )
        .unwrap();
    }
    let seq = caravel_node::node_config::SequencerConfig::load_offline(
        &dir.path().join("config/sequencer.toml"),
    )
    .unwrap();
    let signers = seq.signers.expect("a [signers] section");
    assert_eq!(
        (signers.epoch, signers.threshold, signers.validators.len()),
        (3, 2, 3)
    );
    assert_eq!(seq.settlement_contract, [7; 32]);
    for v in ["1", "2", "3"] {
        let cfg = caravel_node::validator::ValidatorConfig::load(
            &dir.path().join(format!("config/validator-{v}.toml")),
        )
        .unwrap();
        assert_eq!(
            cfg.listen,
            format!("127.0.0.1:{}", 18180 + v.parse::<u16>().unwrap())
        );
        assert_eq!(cfg.stellar_poll_secs, 2);
    }
    let relayer: serde_json::Value = serde_json::from_str(&files["relayer.json"]).unwrap();
    assert_eq!(relayer["sequencerUrl"], "http://127.0.0.1:18180");
    assert_eq!(relayer["intervalsMs"]["inbox"], 1000);
    assert_eq!(relayer["feeds"], serde_json::json!([]));
    // No secret in any file.
    for (name, text) in &files {
        assert!(
            !text
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|w| w.len() == 56 && w.starts_with('S')),
            "{name}"
        );
    }
}

#[test]
fn rendered_files_are_pinned() {
    let m = manifest();
    let files = render(&m, &resolved(&m), "/opt/caravel", 1).unwrap();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/render");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in &files {
            std::fs::write(dir.join(name), text).unwrap();
        }
        return;
    }
    for (name, text) in &files {
        let want = std::fs::read_to_string(dir.join(name))
            .unwrap_or_else(|_| panic!("{name} (UPDATE_GOLDEN=1)"));
        assert!(*text == want, "{name} changed:\n{text}");
    }
}

/// A deployment's own [node] (M0.6) reaches the nodes' lane.toml and
/// leaves the genesis as it was: [node] is not consensus.
#[test]
fn a_deployments_node_settings_reach_the_hosts_not_genesis() {
    let text = std::fs::read_to_string(
        root().join("lanes/payments/config/lane.caravel-payments.local.toml"),
    )
    .unwrap()
        + "\n[env.local.node]\ncheckpoint_every_blocks = 5\n";
    let m = Manifest::parse(&text, "local").unwrap();
    let files = render(&m, &resolved(&m), "/opt/caravel", 1).unwrap();
    let rendered = LaneFile::parse(&files["lane.toml"]).unwrap();
    assert_eq!(rendered.node.checkpoint_every_blocks, 5);
    assert_eq!(rendered.node.block_time_ms, 1000);
    let base = manifest();
    let (_, c1, s1) = lane_toml::genesis(&PaymentsApp, &rendered).unwrap();
    let (_, c2, s2) = lane_toml::genesis(&PaymentsApp, &base.lane).unwrap();
    assert_eq!((c1, s1), (c2, s2));
}
