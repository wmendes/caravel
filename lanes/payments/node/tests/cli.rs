//! The `caravel` CLI over a payments lane file, with this crate's binary as
//! the template plugin (M0.6, DEC-075): what `validate` and `env list`
//! report, offline. The keystore's identities are not asserted (CI has
//! none); everything else is.

use std::path::{Path, PathBuf};

use caravel_cli::init::{ensure_identities, init, keys_report, InitArgs};
use caravel_cli::{env_list_json, validate_report};
use caravel_deploy::manifest::Manifest;
use caravel_node::lane_toml::LaneFile;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn use_this_plugin() {
    let bin = Path::new(env!("CARGO_BIN_EXE_caravel-payments-node"));
    std::env::set_var("CARAVEL_PLUGIN_DIR", bin.parent().unwrap());
}

fn check<'a>(report: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["check"] == name)
        .unwrap_or_else(|| panic!("no {name} check in {report}"))
}

#[test]
fn validate_checks_genesis_and_every_deployment() {
    use_this_plugin();
    let lane = root().join("lanes/payments/config/lane.caravel-payments.local.toml");
    let (_, report) = validate_report(&lane, None).unwrap();
    let genesis = check(&report, "genesis");
    assert_eq!(genesis["ok"], true);
    // The local lane's pinned hashes (tests/lane_file.rs).
    assert_eq!(
        genesis["lane_id"],
        "5528e7c78337a09b73ad00fd5a0f3516640b7b1e7b936c69b7cb171c1bcf61b7"
    );
    assert_eq!(
        genesis["config_hash"],
        "8c16d400bca53cd17af142f1b8cb0517a9c320bd1b4255db4d493a17edb3d66d"
    );
    assert_eq!(
        genesis["genesis_state_hash"],
        "589cb7a65c011ebc7cb6c8082dd7bcc5578af03f404c7a46ee84ccb7635ef0da"
    );
    assert_eq!(check(&report, "[env.local]")["rules"], true);
}

#[test]
fn validate_reports_a_broken_deployment() {
    use_this_plugin();
    let dir = tempfile::tempdir().unwrap();
    let text = std::fs::read_to_string(
        root().join("lanes/payments/config/lane.caravel-payments.local.toml"),
    )
    .unwrap()
    .replace("threshold = 2", "threshold = 9")
        + "\n[env.other]\nnetwork = \"mainnet\"\n";
    let lane = dir.path().join("lane.toml");
    std::fs::write(&lane, text).unwrap();
    let (ok, report) = validate_report(&lane, None).unwrap();
    assert!(!ok);
    assert_eq!(check(&report, "genesis")["ok"], true);
    let local = check(&report, "[env.local]");
    assert_eq!(local["rules"], false);
    assert!(
        local["error"].as_str().unwrap().contains("threshold"),
        "{local}"
    );
    let other = check(&report, "[env.other]");
    assert!(
        other["error"].as_str().unwrap().contains("mainnet"),
        "{other}"
    );
    // --env narrows it to one deployment.
    let (_, one) = validate_report(&lane, Some("other")).unwrap();
    assert_eq!(one["checks"].as_array().unwrap().len(), 2);
}

#[test]
fn a_bad_genesis_is_the_templates_error() {
    use_this_plugin();
    let dir = tempfile::tempdir().unwrap();
    let text = std::fs::read_to_string(
        root().join("lanes/payments/config/lane.caravel-payments.local.toml"),
    )
    .unwrap()
    .replace("min_transfer = 1", "min_transfer = 0");
    let lane = dir.path().join("lane.toml");
    std::fs::write(&lane, text).unwrap();
    let (ok, report) = validate_report(&lane, None).unwrap();
    assert!(!ok);
    let g = check(&report, "genesis");
    assert_eq!(g["ok"], false);
    assert!(g["error"].as_str().unwrap().contains("min_transfer"), "{g}");
}

#[test]
fn env_list_marks_the_chosen_deployment() {
    let text = std::fs::read_to_string(
        root().join("lanes/payments/config/lane.caravel-payments.local.toml"),
    )
    .unwrap()
        + "\n[env.testnet]\ndefault = true\nnetwork = \"testnet\"\nhost = { provider = \"ssh\" }\n";
    let lane = LaneFile::parse(&text).unwrap();
    let list = env_list_json(&lane, None, None);
    assert_eq!(
        list,
        serde_json::json!([
            { "name": "local", "default": false, "network": "local", "provider": "local", "selected": false },
            { "name": "testnet", "default": true, "network": "testnet", "provider": "ssh", "selected": true },
        ])
    );
    let list = env_list_json(&lane, Some("local"), None);
    assert_eq!(list[0]["selected"], true);
}

/// `caravel init payments acme-pay`, against a keystore of its own: the lane
/// file, its identities, and a lane file every check accepts. Needs the
/// Stellar CLI (skipped without it, as on CI's test runners).
#[test]
fn init_writes_a_lane_file_and_its_identities() {
    if std::process::Command::new("stellar")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipped: no stellar CLI");
        return;
    }
    use_this_plugin();
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", home.path());
    let dir = home.path().join("Acme Pay");
    let args = InitArgs {
        template: Some("payments"),
        dir: Some(&dir),
        name: None,
        port: Some(28080),
        prefix: None,
        force: false,
    };
    let done = init(&args).unwrap();
    assert_eq!(done.lane_file, dir.join("lane.toml"));
    assert_eq!(done.report["name"], "acme-pay");
    let ids = done.report["identities"].as_array().unwrap();
    assert_eq!(ids.len(), 6);
    assert!(ids.iter().all(|i| i["created"] == true));
    assert!(ids.iter().any(|i| i["identity"] == "acme-pay-treasury"));
    // The lane file passes every check, and the keystore has what it names.
    let (ok, report) = validate_report(&done.lane_file, None).unwrap();
    assert!(ok, "{report}");
    let m = Manifest::load(&done.lane_file, "local").unwrap();
    assert!(m.env.default);
    assert_eq!(m.env.sequencer.port, 28080);
    assert!(keys_report(&m)
        .as_array()
        .unwrap()
        .iter()
        .all(|k| k["exists"] == true));
    assert!(ensure_identities(&m).unwrap().is_empty());
    let ignore = std::fs::read_to_string(dir.join(".gitignore")).unwrap();
    assert!(ignore.lines().any(|l| l == ".caravel/"));
    // Again: refused without --force; with it, the same identities are reused.
    assert!(init(&args).is_err());
    let again = init(&InitArgs {
        force: true,
        ..args
    })
    .unwrap();
    assert!(again.report["identities"]
        .as_array()
        .unwrap()
        .iter()
        .all(|i| i["created"] == false));
    let ignore2 = std::fs::read_to_string(dir.join(".gitignore")).unwrap();
    assert_eq!(ignore, ignore2, "the .gitignore line is added once");
}
