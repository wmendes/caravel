//! Golden plans (`tests/golden/plan-*.txt`): each case is a lane file's
//! deployment against one state of Stellar and the host, rendered as the user
//! reads it. `UPDATE_GOLDEN=1` rewrites them; review the diff.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use caravel_deploy::address::{asset_contract_id, contract_id, settlement_salt};
use caravel_deploy::manifest::{Network, Provider};
use caravel_deploy::plan::{
    diff, diff_with, step_addr, target_epoch, Chain, DeclaredAccount, Desired, DesiredHost, Host,
    NodeReport, NodeState, OnChain, Options, Params, Plan, Problem, SignerSet, Step,
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
        token: asset_contract_id(passphrase, "USDC", &key(ADMIN)),
        token_asset: Some(("USDC".into(), key(ADMIN))),
        settlement_asset: Some(("USDC".into(), key(ADMIN))),
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
            platform: Some("Linux x86_64".into()),
            files: files(1, &["1", "2", "3"]),
        },
        vars: String::new(),
        accounts: vec![],
    }
}

/// The chain after a first apply of `d`.
fn deployed(d: &Desired) -> Chain {
    Chain {
        accounts: BTreeSet::from([d.admin, d.relayer]),
        trustlines: BTreeMap::new(),
        token_exists: true,
        settlement_wasm_uploaded: true,
        settlement: Some(OnChain {
            code: d.settlement_wasm,
            admin: d.admin,
            token: d.token,
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
        epoch: None,
        release: None,
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
        let started_with = Some(caravel_deploy::plan::fingerprint(&d.host.files, &n));
        nodes.insert(
            n,
            NodeState {
                running: true,
                report: Some(report(d)),
                started_with,
            },
        );
    }
    Host {
        missing: vec![],
        platform: Some("Linux x86_64".into()),
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
                who: "admin".into(),
                key: d.admin
            },
            Step::Fund {
                who: "relayer".into(),
                key: d.relayer
            },
            Step::DeployToken {
                contract: d.token,
                code: "USDC".into(),
                issuer: d.admin
            },
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
    d.token_asset = None;
    let chain = Chain {
        accounts: BTreeSet::new(),
        trustlines: BTreeMap::new(),
        token_exists: true,
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

#[test]
fn a_node_started_on_older_files_is_restarted() {
    // An apply was stopped after writing sequencer.toml, before the restart:
    // the file matches, but the sequencer runs what it started with.
    let d = desired();
    let mut host = running(&d);
    host.nodes.get_mut("sequencer").unwrap().started_with = Some(key(0x77));
    let plan = diff(&d, &deployed(&d), &host);
    assert_eq!(
        plan.steps,
        [Step::Restart {
            node: "sequencer".into()
        }]
    );
}

#[test]
fn a_sequencer_on_the_old_epoch_is_restarted() {
    // A rotation landed, but the apply stopped before restarting the sequencer.
    let d = desired();
    let mut host = running(&d);
    host.nodes.get_mut("sequencer").unwrap().started_with = None;
    host.nodes
        .get_mut("sequencer")
        .unwrap()
        .report
        .as_mut()
        .unwrap()
        .epoch = Some(0);
    let plan = diff(&d, &deployed(&d), &host);
    assert_eq!(
        plan.steps,
        [Step::Restart {
            node: "sequencer".into()
        }]
    );
    // A node built from another release than the host's.
    let mut host = running(&d);
    host.nodes
        .get_mut("validator-2")
        .unwrap()
        .report
        .as_mut()
        .unwrap()
        .release = Some("ffffffff".into());
    assert_eq!(
        diff(&d, &deployed(&d), &host).steps,
        [Step::Restart {
            node: "validator-2".into()
        }]
    );
}

/// A release built for another platform is never installed (a macOS build
/// on lane #1's Linux VM); once the host runs the release, it is no problem.
#[test]
fn a_release_for_another_platform() {
    let mut d = desired();
    d.host.platform = Some("Darwin arm64".into());
    let mut host = running(&d);
    host.release = Some("an-older-one".into());
    let plan = diff(&d, &deployed(&d), &host);
    assert_eq!(
        plan.problems,
        [Problem::WrongPlatform {
            release: "Darwin arm64".into(),
            host: "Linux x86_64".into()
        }]
    );
    check("wrong-platform", &d, &plan);
    host.release = Some(d.host.release.clone());
    assert!(diff(&d, &deployed(&d), &host).problems.is_empty());
}

/// A lane file's vars show in the plan's header; without vars, the header
/// is as it was (lane #1's plan stays byte for byte).
#[test]
fn vars_in_the_header() {
    let mut d = desired();
    let plain = diff(&d, &deployed(&d), &running(&d)).render(&d);
    assert!(!plain.contains("vars:"));
    d.vars = "validators = [\"1\", \"2\"], token = (sensitive)".into();
    let text = diff(&d, &deployed(&d), &running(&d)).render(&d);
    assert!(
        text.contains("  vars: validators = [\"1\", \"2\"], token = (sensitive)\n"),
        "{text}"
    );
}

fn lines(plan: &Plan) -> Vec<String> {
    plan.steps.iter().map(step_addr).collect()
}

fn target(t: &[&str]) -> Options {
    Options {
        target: t.iter().map(|s| s.to_string()).collect(),
        replace: vec![],
    }
}

fn replace(r: &[&str]) -> Options {
    Options {
        target: vec![],
        replace: r.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn a_target_plans_it_and_what_it_depends_on() {
    let d = desired();
    let (chain, host) = (Chain::default(), Host::default());
    // The relayer needs its account, the contract (and what that needs), the
    // release and the files it reads; not the validators or the sequencer.
    let plan = diff_with(&d, &chain, &host, &target(&["node.relayer"])).unwrap();
    assert_eq!(
        lines(&plan),
        [
            "account.admin",
            "account.relayer",
            "token.settlement",
            "wasm.settlement",
            "contract.settlement",
            "release",
            "file.lane.toml",
            "file.relayer.json",
            "node.relayer",
        ]
    );
    assert!(plan
        .render(&d)
        .contains("  target: node.relayer (and what it depends on)\n"));
    // The sequencer follows the rotation, which follows every validator.
    let plan = diff_with(&d, &chain, &host, &target(&["node.sequencer"])).unwrap();
    for n in ["node.validator-1", "node.validator-2", "node.validator-3"] {
        assert!(lines(&plan).iter().any(|l| l == n), "{n}");
    }
    assert!(!lines(&plan).iter().any(|l| l == "node.relayer"));
    // `kind.*`: every file, and what writing them needs.
    let plan = diff_with(&d, &chain, &host, &target(&["file.*"])).unwrap();
    assert_eq!(
        lines(&plan)
            .iter()
            .filter(|l| l.starts_with("file."))
            .count(),
        d.host.files.len()
    );
    assert!(!lines(&plan).iter().any(|l| l.starts_with("node.")));
}

#[test]
fn a_target_leaves_out_what_it_does_not_need() {
    // Validator 3 replaced by 4: the new validator alone, without the
    // rotation (which follows it) or the old validator's stop.
    let old = desired();
    let mut d = desired();
    d.signers = signers(&[0x61, 0x62, 0x64]);
    d.validators = vec![
        "validator-1".into(),
        "validator-2".into(),
        "validator-4".into(),
    ];
    d.host.files = files(2, &["1", "2", "4"]);
    let mut chain = deployed(&old);
    if let Some(oc) = chain.settlement.as_mut() {
        oc.desired_set_epoch = None;
    }
    let plan = diff_with(&d, &chain, &running(&old), &target(&["node.validator-4"])).unwrap();
    assert_eq!(lines(&plan), ["file.validator-4.toml", "node.validator-4"]);
}

#[test]
fn an_unknown_target_says_what_it_meant() {
    let d = desired();
    let err = diff_with(
        &d,
        &Chain::default(),
        &Host::default(),
        &target(&["node.validator-9"]),
    )
    .unwrap_err();
    assert!(err.contains("no resource \"node.validator-9\""), "{err}");
    assert!(err.contains("did you mean node.validator-"), "{err}");
    let err = diff_with(
        &d,
        &Chain::default(),
        &Host::default(),
        &target(&["nodes.*"]),
    )
    .unwrap_err();
    assert!(err.contains("caravel graph"), "{err}");
}

#[test]
fn a_replacement_is_planned_even_when_nothing_differs() {
    let d = desired();
    let (chain, host) = (deployed(&d), running(&d));
    assert!(diff(&d, &chain, &host).is_empty());
    let one = |r: &[&str]| diff_with(&d, &chain, &host, &replace(r)).unwrap();
    assert_eq!(
        one(&["node.sequencer"]).steps,
        [Step::Restart {
            node: "sequencer".into()
        }]
    );
    // A file is written again, and the node that reads it restarts.
    assert_eq!(
        one(&["file.relayer.json"]).steps,
        [
            Step::WriteFile {
                path: "relayer.json".into()
            },
            Step::Restart {
                node: "relayer".into()
            }
        ]
    );
    // The release again restarts every node.
    let plan = one(&["release"]);
    assert!(matches!(plan.steps[0], Step::InstallRelease { .. }));
    assert_eq!(plan.steps.len(), 1 + 5);
    assert!(plan.render(&d).contains("  replace: release\n"));
    // Patterns: every validator, every node.
    assert_eq!(one(&["node.validator-*"]).steps.len(), 3);
    assert_eq!(one(&["node.*"]).steps.len(), 5);
}

#[test]
fn what_cannot_be_replaced_says_why() {
    let d = desired();
    let (chain, host) = (deployed(&d), running(&d));
    for (addr, why) in [
        (
            "contract.settlement",
            "its address derives from the admin and the lane's name",
        ),
        ("account.admin", "its key is the lane file's identity"),
        ("signers.settlement", "a rotation"),
    ] {
        let err = diff_with(&d, &chain, &host, &replace(&[addr])).unwrap_err();
        assert!(err.contains(&format!("{addr} can't be replaced")), "{err}");
        assert!(err.contains(why), "{err}");
    }
}

#[test]
fn the_plan_for_scripts_names_each_resource() {
    let d = desired();
    let chain = Chain {
        token_exists: false,
        ..Chain::default()
    };
    let mut d2 = d.clone();
    d2.token_asset = None;
    let v = diff(&d2, &chain, &Host::default()).to_json(&d2);
    assert_eq!(v["format"], "caravel-plan/1");
    assert_eq!(v["steps"][0]["address"], "account.admin");
    assert_eq!(v["steps"][0]["change"], "+");
    assert_eq!(v["steps"][0]["action"], "fund");
    assert_eq!(v["problems"][0]["address"], "token.settlement");
    assert!(v["problems"][0]["message"]
        .as_str()
        .unwrap()
        .starts_with("the network has no token contract"));
    let _ = d;
}

fn account(name: &str, k: u8, trust: bool, balance: Option<i128>) -> DeclaredAccount {
    DeclaredAccount {
        name: name.into(),
        identity: format!("demo-{name}"),
        key: key(k),
        fund: true,
        trustlines: if trust {
            vec![("USDC".into(), key(ADMIN))]
        } else {
            vec![]
        },
        balance,
        depends_on: vec![],
    }
}

const USDC: i128 = 10_000_000;

#[test]
fn declared_accounts_are_funded_trusted_and_topped_up() {
    let mut d = desired();
    d.accounts = vec![
        account("alice", 0xE1, true, Some(100 * USDC)),
        account("bob", 0xE2, true, None),
    ];
    let plan = diff(&d, &Chain::default(), &Host::default());
    assert!(plan.problems.is_empty(), "{:?}", plan.problems);
    // After the token (alice is minted to, bob trusts it), before the contract.
    let addrs = lines(&plan);
    let at = |a: &str| addrs.iter().position(|x| x == a).unwrap();
    assert!(at("token.settlement") < at("account.alice"));
    assert!(at("account.alice") < at("contract.settlement"));
    check("accounts", &d, &plan);

    // Topped up by what's missing; nothing once it holds enough.
    let mut chain = deployed(&d);
    for a in &d.accounts {
        chain.accounts.insert(a.key);
    }
    chain
        .trustlines
        .insert((key(0xE1), "USDC".into(), key(ADMIN)), 40 * USDC);
    chain
        .trustlines
        .insert((key(0xE2), "USDC".into(), key(ADMIN)), 0);
    let plan = diff(&d, &chain, &running(&d));
    assert_eq!(
        plan.steps,
        [Step::Mint {
            who: "alice".into(),
            key: key(0xE1),
            code: "USDC".into(),
            amount: 60 * USDC
        }]
    );
    chain
        .trustlines
        .insert((key(0xE1), "USDC".into(), key(ADMIN)), 150 * USDC);
    assert!(diff(&d, &chain, &running(&d)).is_empty());
}

#[test]
fn what_the_admin_does_not_issue_it_cannot_mint() {
    let mut d = desired();
    // Another issuer's asset (Circle's USDC, say).
    d.settlement_asset = Some(("USDC".into(), key(0xC1)));
    let mut alice = account("alice", 0xE1, false, Some(5 * USDC));
    alice.trustlines = vec![("USDC".into(), key(0xC1))];
    let mut bob = account("bob", 0xE2, false, None);
    bob.fund = false;
    d.accounts = vec![alice, bob];
    let mut chain = deployed(&d);
    chain.accounts.insert(key(0xE1));
    chain
        .trustlines
        .insert((key(0xE1), "USDC".into(), key(0xC1)), USDC);
    let plan = diff(&d, &chain, &running(&d));
    assert!(plan.steps.is_empty(), "{:?}", plan.steps);
    assert_eq!(
        plan.problems,
        [
            Problem::CannotMint {
                who: "alice".into(),
                code: "USDC".into(),
                have: USDC,
                want: 5 * USDC
            },
            Problem::AccountMissing { who: "bob".into() },
        ]
    );
    let text = plan.render(&d);
    assert!(
        text.contains(
            "! alice holds 1 USDC, less than its balance of 5, and the admin doesn't issue USDC"
        ),
        "{text}"
    );
    let json = plan.to_json(&d);
    assert_eq!(json["problems"][1]["address"], "account.bob");
}

#[test]
fn depends_on_orders_declared_accounts() {
    let mut d = desired();
    let mut alice = account("alice", 0xE1, false, None);
    alice.depends_on = vec!["account.bob".into()];
    d.accounts = vec![alice, account("bob", 0xE2, false, None)];
    let plan = diff(&d, &Chain::default(), &Host::default());
    let addrs = lines(&plan);
    let at = |a: &str| addrs.iter().position(|x| x == a).unwrap();
    assert!(at("account.bob") < at("account.alice"));
    d.accounts[0].depends_on = vec!["account.carol".into()];
    let err = diff_with(&d, &Chain::default(), &Host::default(), &Options::default()).unwrap_err();
    assert!(
        err.contains("accounts.alice.depends_on: no resource \"account.carol\""),
        "{err}"
    );
}
