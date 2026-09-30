//! Golden plans (`tests/golden/plan-*.txt`): each case is a lane file's
//! deployment against one state of Stellar and the host, rendered as the user
//! reads it. `UPDATE_GOLDEN=1` rewrites them; review the diff.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use caravel_deploy::address::{asset_contract_id, contract_id, settlement_salt};
use caravel_deploy::manifest::{Network, Provider};
use caravel_deploy::plan::{
    diff, target_epoch, Chain, Desired, DesiredHost, Host, NodeReport, NodeState, OnChain, Params,
    Plan, Problem, SignerSet, Step,
};

fn key(n: u8) -> [u8; 32] {
    [n; 32]
}

fn hex_key(h: &str) -> [u8; 32] {
    let v: Vec<u8> = (0..32)
        .map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap())
        .collect();
    v.try_into().unwrap()
}

const ADMIN: u8 = 0xA0;
const RELAYER: u8 = 0xA1;

fn signers(keys: &[u8]) -> SignerSet {
    let mut s: Vec<_> = keys.iter().map(|k| (key(*k), 1)).collect();
    s.sort();
    SignerSet {
        signers: s,
        threshold: 2,
    }
}

fn files(epoch: u64, validators: &[&str]) -> BTreeMap<String, [u8; 32]> {
    let mut f = BTreeMap::new();
    f.insert("lane.toml".into(), key(0x11));
    f.insert("sequencer.toml".into(), key(0x20 + epoch as u8));
    f.insert("relayer.json".into(), key(0x31));
    for v in validators {
        f.insert(format!("validator-{v}.toml"), key(0x40 + v.as_bytes()[0]));
    }
    f
}

fn desired() -> Desired {
    let lane_id = key(1);
    let passphrase = Network::Local.passphrase();
    Desired {
        lane_name: "demo-pay-0".into(),
        env: "local".into(),
        network: Network::Local,
        lane_id,
        config_hash: key(2),
        genesis_state_hash: key(3),
        engine_wasm_hash: key(4),
        admin: key(ADMIN),
        relayer: key(RELAYER),
        usdc: asset_contract_id(passphrase, "USDC", &key(ADMIN)),
        usdc_local: true,
        settlement: contract_id(passphrase, &key(ADMIN), &settlement_salt(&lane_id)),
        settlement_pinned: false,
        settlement_wasm: hex_key(caravel_deploy::versions::settlement_wasm()),
        params: Params {
            force_inclusion_window_secs: 20,
            escape_timeout_secs: 30,
            min_rotation_delay_secs: 3600,
            signer_retention_epochs: 2,
            min_deposit: 10_000_000,
        },
        signers: signers(&[0x61, 0x62, 0x63]),
        validators: vec![
            "validator-1".into(),
            "validator-2".into(),
            "validator-3".into(),
        ],
        host: DesiredHost {
            provider: Provider::Local,
            release: "0a1b2c3d".into(),
            files: files(1, &["1", "2", "3"]),
        },
    }
}

/// The chain after a first apply of `d`.
fn deployed(d: &Desired) -> Chain {
    Chain {
        accounts: BTreeSet::from([d.admin, d.relayer]),
        usdc_exists: true,
        settlement_wasm_uploaded: true,
        settlement: Some(OnChain {
            code: d.settlement_wasm,
            admin: d.admin,
            usdc: d.usdc,
            lane_id: d.lane_id,
            engine_wasm_hash: d.engine_wasm_hash,
            genesis_state_hash: d.genesis_state_hash,
            config_hash: d.config_hash,
            params: d.params.clone(),
            epoch: 1,
            signers: d.signers.clone(),
            desired_set_epoch: Some(1),
            frozen: false,
        }),
    }
}

fn report(d: &Desired) -> NodeReport {
    NodeReport {
        lane_id: d.lane_id,
        config_hash: d.config_hash,
        settlement: d.settlement,
        engine_wasm_hash: d.engine_wasm_hash,
        key: None,
    }
}

/// The host after a first apply of `d`: every node running.
fn running(d: &Desired) -> Host {
    let mut nodes = BTreeMap::new();
    for n in ["sequencer", "relayer"]
        .into_iter()
        .map(String::from)
        .chain(d.validators.iter().cloned())
    {
        nodes.insert(
            n,
            NodeState {
                running: true,
                report: Some(report(d)),
            },
        );
    }
    Host {
        missing: vec![],
        release: Some(d.host.release.clone()),
        files: d.host.files.clone(),
        nodes,
        has_data: true,
    }
}

fn check(name: &str, d: &Desired, plan: &Plan) {
    let text = plan.render(d);
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("plan-{name}.txt"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let want =
        std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{name} (UPDATE_GOLDEN=1)"));
    assert!(text == want, "plan-{name}.txt changed:\n{text}");
}

#[test]
fn a_fresh_lane() {
    let d = desired();
    let plan = diff(&d, &Chain::default(), &Host::default());
    assert_eq!(plan.target_epoch, 1);
    assert!(plan.problems.is_empty());
    assert_eq!(
        plan.steps[..5],
        [
            Step::Fund {
                who: "admin",
                key: d.admin
            },
            Step::Fund {
                who: "relayer",
                key: d.relayer
            },
            Step::DeployUsdc { contract: d.usdc },
            Step::UploadWasm {
                hash: d.settlement_wasm
            },
            Step::DeploySettlement {
                contract: d.settlement
            },
        ]
    );
    // Validators start before the sequencer, and the relayer last.
    let starts: Vec<_> = plan
        .steps
        .iter()
        .filter_map(|s| match s {
            Step::Start { node } => Some(node.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        starts,
        [
            "validator-1",
            "validator-2",
            "validator-3",
            "sequencer",
            "relayer"
        ]
    );
    check("fresh", &d, &plan);
}

#[test]
fn applying_again_changes_nothing() {
    let d = desired();
    let plan = diff(&d, &deployed(&d), &running(&d));
    assert!(plan.is_empty(), "{plan:?}");
    check("no-op", &d, &plan);
}

#[test]
fn a_node_setting_restarts_the_nodes_only() {
    let d = desired();
    let mut host = running(&d);
    host.files.insert("lane.toml".into(), key(0x12)); // e.g. [node] block_time_ms
    let plan = diff(&d, &deployed(&d), &host);
    assert!(plan.problems.is_empty());
    assert!(plan
        .steps
        .iter()
        .all(|s| matches!(s, Step::WriteFile { .. } | Step::Restart { .. })));
    check("node-setting", &d, &plan);
}

#[test]
fn a_new_release_restarts_every_node() {
    let d = desired();
    let mut host = running(&d);
    host.release = Some("99887766".into());
    let plan = diff(&d, &deployed(&d), &host);
    assert_eq!(
        plan.steps[0],
        Step::InstallRelease {
            from: Some("99887766".into()),
            to: "0a1b2c3d".into()
        }
    );
    assert_eq!(plan.steps.len(), 1 + 5);
}

#[test]
fn a_validator_change_is_a_rotation_in_order() {
    // Validator 3 (key 0x63) is replaced by validator 4 (key 0x64).
    let old = desired();
    let mut d = desired();
    d.signers = signers(&[0x61, 0x62, 0x64]);
    d.validators = vec![
        "validator-1".into(),
        "validator-2".into(),
        "validator-4".into(),
    ];
    let chain = deployed(&old);
    let chain = Chain {
        settlement: chain.settlement.map(|mut oc| {
            oc.desired_set_epoch = None;
            oc
        }),
        ..chain
    };
    assert_eq!(target_epoch(&d, &chain), 2);
    d.host.files = files(2, &["1", "2", "4"]);
    let plan = diff(&d, &chain, &running(&old));
    assert!(plan.problems.is_empty(), "{:?}", plan.problems);
    let pos = |s: &Step| {
        plan.steps
            .iter()
            .position(|x| x == s)
            .unwrap_or_else(|| panic!("{s:?}"))
    };
    let start4 = pos(&Step::Start {
        node: "validator-4".into(),
    });
    let rotate = pos(&Step::RotateSigners {
        from_epoch: 1,
        to_epoch: 2,
    });
    let restart_seq = pos(&Step::Restart {
        node: "sequencer".into(),
    });
    let stop3 = pos(&Step::Stop {
        node: "validator-3".into(),
    });
    assert!(start4 < rotate && rotate < restart_seq && restart_seq < stop3);
    check("validator-change", &d, &plan);
}

#[test]
fn what_the_constructor_fixed_cannot_change() {
    let d = desired();
    let mut changed = d.clone();
    changed.config_hash = key(0x22); // e.g. a [limits] change
    changed.genesis_state_hash = key(0x23);
    changed.params.escape_timeout_secs = 60;
    let plan = diff(&changed, &deployed(&d), &running(&d));
    let fields: Vec<_> = plan
        .problems
        .iter()
        .filter_map(|p| match p {
            Problem::Immutable { field, .. } => Some(*field),
            _ => None,
        })
        .collect();
    assert_eq!(
        fields,
        [
            "config_hash",
            "genesis_state_hash",
            "settlement_params.escape_timeout_secs"
        ]
    );
    assert_eq!(plan.problems.len(), 3, "no node noise: {:?}", plan.problems);
    check("immutable", &changed, &plan);
}

#[test]
fn a_frozen_lane() {
    let d = desired();
    let mut chain = deployed(&d);
    chain.settlement.as_mut().unwrap().frozen = true;
    let plan = diff(&d, &chain, &running(&d));
    assert_eq!(plan.problems, [Problem::Frozen]);
    check("frozen", &d, &plan);
}

#[test]
fn after_a_testnet_reset() {
    // The contract is gone; the address is the same, so the old stores go.
    let mut d = desired();
    d.network = Network::Testnet;
    d.usdc_local = false;
    let chain = Chain {
        accounts: BTreeSet::new(),
        usdc_exists: true,
        settlement_wasm_uploaded: false,
        settlement: None,
    };
    let plan = diff(&d, &chain, &running(&d));
    assert!(plan.problems.is_empty());
    let wipe = plan
        .steps
        .iter()
        .position(|s| *s == Step::WipeHostData)
        .unwrap();
    let deploy = plan
        .steps
        .iter()
        .position(|s| matches!(s, Step::DeploySettlement { .. }))
        .unwrap();
    assert!(deploy < wipe);
    check("reset", &d, &plan);
    // A lane file that names its contract can't recreate it.
    d.settlement_pinned = true;
    let plan = diff(&d, &chain, &running(&d));
    assert_eq!(
        plan.problems,
        [Problem::SettlementMissing {
            contract: d.settlement
        }]
    );
}

#[test]
fn drift_the_tool_did_not_make() {
    let d = desired();
    let mut chain = deployed(&d);
    // Upgraded to lane #1's build, outside the tool.
    chain.settlement.as_mut().unwrap().code =
        hex_key(caravel_deploy::versions::known_settlement_builds()[1]);
    let mut host = running(&d);
    // A validator runs another config than its files give.
    host.nodes
        .get_mut("validator-2")
        .unwrap()
        .report
        .as_mut()
        .unwrap()
        .config_hash = key(0x99);
    let plan = diff(&d, &chain, &host);
    assert!(matches!(plan.problems[0], Problem::CodeDrift { .. }));
    assert_eq!(
        plan.problems[1],
        Problem::NodeMismatch {
            node: "validator-2".into(),
            field: "config_hash"
        }
    );
    check("drift", &d, &plan);
    chain.settlement.as_mut().unwrap().code = key(0xEE);
    assert!(matches!(
        diff(&d, &chain, &running(&d)).problems[0],
        Problem::UnknownCode { .. }
    ));
}

#[test]
fn a_signer_set_is_installed_once() {
    // Epoch 2 rotated away from the lane file's set; going back is refused.
    let d = desired();
    let mut chain = deployed(&d);
    let oc = chain.settlement.as_mut().unwrap();
    oc.epoch = 2;
    oc.signers = signers(&[0x61, 0x62, 0x64]);
    oc.desired_set_epoch = Some(1);
    let plan = diff(&d, &chain, &running(&d));
    assert_eq!(plan.problems, [Problem::SignersReused { epoch: 1 }]);
    check("signers-reused", &d, &plan);
}

#[test]
fn a_host_that_is_not_ready() {
    let d = desired();
    let host = Host {
        missing: vec!["Node 22".into(), "caddy".into()],
        ..Host::default()
    };
    let plan = diff(&d, &Chain::default(), &host);
    assert_eq!(
        plan.problems,
        [Problem::HostNotReady {
            missing: vec!["Node 22".into(), "caddy".into()]
        }]
    );
}
