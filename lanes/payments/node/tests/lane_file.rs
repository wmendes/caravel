//! Payments lane files (DEC-054, spec §20.4.4): the local file maps to the
//! `AppGenesisV1` built by hand, its hashes are pinned, and bad files are
//! refused before any node starts.

use std::path::PathBuf;

use caravel_app_sdk::{template_id, AccessMode, AppGenesisV1};
use caravel_core::preimage::lane_id_preimage;
use caravel_harness::{pk, sha256, USDC};
use caravel_node::lane_toml::{self, LaneFile};
use caravel_payments::Params;
use caravel_payments_node::{lane_file, PaymentsApp};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn local() -> String {
    std::fs::read_to_string(root().join("lanes/payments/config/lane.caravel-payments.local.toml"))
        .unwrap()
}

fn genesis(t: &str) -> anyhow::Result<(lane_toml::GenesisReport, Vec<u8>, Vec<u8>)> {
    lane_toml::genesis(&PaymentsApp, &LaneFile::parse(t)?)
}

fn refused(t: &str) -> String {
    format!("{:#}", genesis(t).expect_err("refused"))
}

#[test]
fn the_local_file_is_the_config_built_by_hand() {
    let want = AppGenesisV1 {
        lane_id: sha256(&lane_id_preimage("caravel-payments-local-0")),
        template: template_id("payments"),
        template_version: 1,
        system_keys: vec![pk(0x22)],
        access_mode: AccessMode::Open,
        allowlist: vec![],
        min_deposit: USDC,
        min_withdrawal: USDC,
        max_accounts: 256,
        max_session_keys: 4,
        max_txs_per_account_per_block: 50,
        max_entries_per_block: 256,
        max_block_bytes: 12_000,
        max_pending_withdrawals: 512,
        exec_cpu_limit: 200_000_000,
        exec_mem_limit: 41_943_040,
        app_params: Params {
            transfer_fee: 100_000,
            min_transfer: 1,
        }
        .encode(),
    };
    let file = LaneFile::parse(&local()).unwrap();
    assert_eq!(lane_file::genesis_config(&file).unwrap(), want);
    let (report, config, state) = genesis(&local()).unwrap();
    assert_eq!(config, want.encode().unwrap());
    assert_eq!(report.lane_name, "caravel-payments-local-0");
    assert_eq!(
        report.lane_id,
        "5528e7c78337a09b73ad00fd5a0f3516640b7b1e7b936c69b7cb171c1bcf61b7"
    );
    assert_eq!(
        report.config_hash,
        "8c16d400bca53cd17af142f1b8cb0517a9c320bd1b4255db4d493a17edb3d66d"
    );
    assert_eq!(
        report.genesis_state_hash,
        "589cb7a65c011ebc7cb6c8082dd7bcc5578af03f404c7a46ee84ccb7635ef0da"
    );
    assert_eq!((config.len(), state.len()), (197, 640));
}

#[test]
fn the_file_names_the_engine_of_record() {
    let versions: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root().join("versions.json")).unwrap())
            .unwrap();
    let want = versions["lanes"]["payments"]["engine_wasm_sha256"]
        .as_str()
        .unwrap();
    let file = LaneFile::parse(&local()).unwrap();
    let hash: [u8; 32] = (0..32)
        .map(|i| u8::from_str_radix(&want[2 * i..2 * i + 2], 16).unwrap())
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();
    file.check_engine(&hash).unwrap();
    assert!(file.check_engine(&[0; 32]).is_err());
}

#[test]
fn an_allowlist_lane() {
    let t = local().replace(
        "mode = \"open\"                              # \"open\" | \"allowlist\"\nallowlist = []",
        &format!(
            "mode = \"allowlist\"\nallowlist = [\"{}\", \"{}\"]",
            caravel_runtime::views::g_address(&pk(0x42)),
            caravel_runtime::views::g_address(&pk(0x41)),
        ),
    );
    assert_ne!(t, local(), "the access section changed shape");
    let cfg = lane_file::genesis_config(&LaneFile::parse(&t).unwrap()).unwrap();
    assert_eq!(cfg.access_mode, AccessMode::Allowlist);
    let mut keys = vec![pk(0x41), pk(0x42)];
    keys.sort();
    assert_eq!(cfg.allowlist, keys);
}

#[test]
fn bad_files_are_refused() {
    let cases = [
        (
            "transfer_fee = 100000 ",
            "transfer_fee = -1 ",
            "transfer_fee",
        ),
        ("min_transfer = 1 ", "min_transfer = 0 ", "min_transfer"),
        (
            "treasury_key = \"GCQJ",
            "treasury_key = \"GXQJ",
            "treasury_key",
        ),
        (
            "min_transfer = 1 ",
            "min_transfer = 1\nfee_sponsor = \"x\"\n#",
            "fee_sponsor",
        ),
        (
            "template = \"payments\"",
            "template = \"perps\"",
            "unknown section [payments]",
        ),
        ("mode = \"open\"", "mode = \"closed\"", "access.mode"),
        ("max_accounts = 256", "max_accounts = 1", "genesis rule"),
        (
            "exec_cpu_limit = 200000000",
            "exec_cpu_limit = 400000001",
            "genesis rule",
        ),
    ];
    for (from, to, why) in cases {
        let t = local().replacen(from, to, 1);
        assert_ne!(t, local(), "{from:?} not in the file");
        let err = refused(&t);
        assert!(err.contains(why), "{why}: {err}");
    }
    // No [payments] section.
    let cut = local().split("[payments]").next().unwrap().to_string();
    assert!(refused(&cut).contains("payments"), "{}", refused(&cut));
    // The treasury can't also be on the allowlist.
    let t = local().replace(
        "allowlist = []",
        &format!(
            "allowlist = [\"{}\"]",
            caravel_runtime::views::g_address(&pk(0x22))
        ),
    );
    assert!(refused(&t).contains("genesis rule"));
}

#[test]
fn a_perps_lane_file_is_refused() {
    let perps =
        std::fs::read_to_string(root().join("lanes/perps/config/lane.caravel-perps.local.toml"))
            .unwrap();
    let err = refused(&perps);
    assert!(err.contains("template \"perps\""), "{err}");
}

/// `[env]` is not consensus: the same genesis with a deployment block.
#[test]
fn env_tables_change_no_hash() {
    let block = "\n[env.local]\nnetwork = \"local\"\nadmin = \"acme-admin\"\n[[env.local.validators]]\nname = \"v1\"\nkey = \"acme-v1\"\n[env.local.host]\nprovider = \"local\"\n";
    let (_, config, state) = genesis(&local()).unwrap();
    let (report, config_env, state_env) = genesis(&format!("{}{block}", local())).unwrap();
    assert_eq!((config_env, state_env), (config, state));
    assert_eq!(
        report.config_hash,
        "8c16d400bca53cd17af142f1b8cb0517a9c320bd1b4255db4d493a17edb3d66d"
    );
}
