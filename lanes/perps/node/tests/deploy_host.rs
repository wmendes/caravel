//! The ssh provider's host files for lane #1 (DEC-069, DEC-070): from the
//! `[env.testnet]` table in its lane file, the generated systemd units and
//! Caddyfile are byte for byte the ones its VM runs, kept as copies in
//! `lanes/perps/deploy/testnet/`. Every other file it renders, and the
//! deployment values that reach Stellar, are pinned too (M0.6 baseline,
//! `tests/golden/render-testnet/`; `UPDATE_GOLDEN=1`): a change to the lane
//! file's language must leave lane #1's `plan` at "No changes.".

use std::path::PathBuf;

use std::collections::{BTreeMap, BTreeSet};

use caravel_deploy::deploy::{addresses, desired, Keys};
use caravel_deploy::manifest::{Manifest, Network, SettlementParams, Token};
use caravel_deploy::plan::{diff, fingerprint, Chain, Host, NodeReport, NodeState, OnChain};
use caravel_deploy::render::{render, render_for, Resolved};
use caravel_deploy::template::{InProcess, Template};
use caravel_perps_node::PerpsApp;

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
        sequencer_key: None,
    };
    render(&m, &r, "/opt/caravel", 1).unwrap()
}

/// Lane #1 split across two hosts (C-22): validator 3 on `b`, reached at
/// its private address and reaching the sequencer at `a`'s.
#[test]
fn files_for_two_hosts() {
    let text =
        std::fs::read_to_string(root().join("lanes/perps/config/lane.caravel-perps.testnet.toml"))
            .unwrap();
    let text = text
        .replace(
            "[env.testnet.sequencer]\n",
            "[env.testnet.sequencer]\nhost = \"a\"\nkey = \"caravel-sequencer\"\n",
        )
        .replace(
            "key = \"caravel-validator-3\"\n",
            "key = \"caravel-validator-3\"\nhost = \"b\"\n",
        )
        .replace("[env.testnet.host]\n", "[env.testnet.hosts.a]\nprivate_address = \"10.0.0.2\"\n")
        + "\n[env.testnet.hosts.b]\nprovider = \"ssh\"\naddress = \"caravel-2\"\npublic_url = \"https://b.example\"\nprivate_address = \"10.0.0.3\"\n";
    let m = Manifest::parse(&text, "testnet").unwrap();
    let r = Resolved {
        template: "perps".into(),
        engine_wasm_hash: m.lane.engine_wasm_hash().unwrap().unwrap(),
        settlement: [9; 32],
        validator_keys: vec![[1; 32], [2; 32], [3; 32]],
        web: true,
        sequencer_key: Some([5; 32]),
    };
    let a = render(&m, &r, "/opt/caravel", 1).unwrap();
    let b = render_for(&m, &r, "b", "/opt/caravel", 1).unwrap();
    // The sequencer's host runs the sequencer, the relayer and validators
    // 1 and 2; b runs validator 3 and serves its routes only.
    assert!(a.contains_key("sequencer.toml") && a.contains_key("relayer.json"));
    assert!(a.contains_key("validator-1.toml") && !a.contains_key("validator-3.toml"));
    assert!(b.contains_key("validator-3.toml") && b.contains_key("lane.toml"));
    for f in [
        "sequencer.toml",
        "relayer.json",
        "validator-1.toml",
        "systemd/caravel-sequencer.service",
    ] {
        assert!(!b.contains_key(f), "{f} on b");
    }
    assert_eq!(a["lane.toml"], b["lane.toml"]);
    // The sequencer listens where validator 3 reaches it, and calls it at b.
    let seq = &a["sequencer.toml"];
    assert!(seq.contains("10.0.0.2:8080"), "{seq}");
    assert!(seq.contains("http://10.0.0.3:8083"), "{seq}");
    assert!(seq.contains("http://127.0.0.1:8081"), "{seq}");
    let v3 = &b["validator-3.toml"];
    assert!(v3.contains("http://10.0.0.2:8080"), "{v3}");
    assert!(v3.contains("10.0.0.3:8083"), "{v3}");
    // Validator 1 listens on loopback, and follows the sequencer where it
    // listens.
    let v1 = &a["validator-1.toml"];
    assert!(v1.contains("listen = \"127.0.0.1:8081\""), "{v1}");
    assert!(
        v1.contains("sequencer_url = \"http://10.0.0.2:8080\""),
        "{v1}"
    );
    // The sequencer signs its requests, and every validator checks them
    // (DEC-095).
    let seq_g = stellar_strkey::ed25519::PublicKey([5; 32]).to_string();
    assert!(
        seq.contains("key_file = \"/opt/caravel/keys/sequencer.key\""),
        "{seq}"
    );
    for v in [v1, v3] {
        assert!(
            v.contains(&format!("sequencer_key = \"{}\"", seq_g.as_str())),
            "{v}"
        );
    }
    // b's Caddyfile has validator 3's routes, not the sequencer's.
    let caddy = &b["caddy/Caddyfile"];
    assert!(caddy.contains("10.0.0.3:8083"), "{caddy}");
    assert!(!caddy.contains(":8080"), "{caddy}");
    assert!(caddy.contains("respond /v1/sign 404"), "{caddy}");

    // Without private addresses, through the public URLs: b's proxy passes
    // validator 3's /v1/sign (signed), and nothing listens off loopback.
    let public = text
        .replace("private_address = \"10.0.0.2\"\n", "")
        .replace("private_address = \"10.0.0.3\"\n", "");
    let m = Manifest::parse(&public, "testnet").unwrap();
    let a = render(&m, &r, "/opt/caravel", 1).unwrap();
    let b = render_for(&m, &r, "b", "/opt/caravel", 1).unwrap();
    let seq = &a["sequencer.toml"];
    assert!(seq.contains("listen = \"127.0.0.1:8080\""), "{seq}");
    assert!(
        seq.contains("url = \"https://b.example/validators/3\""),
        "{seq}"
    );
    let v3 = &b["validator-3.toml"];
    assert!(
        v3.contains("sequencer_url = \"https://35-224-76-64.sslip.io\""),
        "{v3}"
    );
    assert!(v3.contains("listen = \"127.0.0.1:8083\""), "{v3}");
    let caddy = &b["caddy/Caddyfile"];
    assert!(!caddy.contains("respond /v1/sign 404"), "{caddy}");
    assert!(caddy.contains("reverse_proxy 127.0.0.1:8083"), "{caddy}");
    // a's proxy keeps its own validators' /v1/sign closed.
    assert!(a["caddy/Caddyfile"].contains("respond /v1/sign 404"));
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

fn hex32(h: &str) -> [u8; 32] {
    caravel_runtime::sequencer::unhex(h)
        .unwrap()
        .try_into()
        .unwrap()
}

/// Lane #1's deployment through the same code `prepare` runs (with keys in
/// place of the keystore), against a Stellar and a host that match it: the
/// plan is empty. The live values it derives are pinned.
#[test]
fn lane_1_plans_no_changes() {
    let m = manifest();
    let genesis = InProcess(PerpsApp).genesis(&m.lane).unwrap();
    assert_eq!(
        genesis.config_hash,
        hex32("f4b9db09137583ba9d66ea0b8a3a2b658a163f7b72993e0f242f04ea3ac93997")
    );
    assert_eq!(
        genesis.genesis_state_hash,
        hex32("22702d9f4c45f88306aca02169cf7a86ccaec3b45ed85103c7a9fc4277291e77")
    );
    let keys = Keys {
        admin: [0xA0; 32],
        relayer: [0xA1; 32],
        validators: vec![[1; 32], [2; 32], [3; 32]],
        accounts: Default::default(),
        sequencer: None,
    };
    let a = addresses(&m, &keys.admin).unwrap();
    assert!(a.settlement_pinned);
    assert_eq!(
        stellar_strkey::Contract(a.settlement).to_string().as_str(),
        "CBIHBEUZYFZQZEQPBJH2ID6CDRDZFEDI6XHAXVOCHG6FO5XWUIGPONWO"
    );
    assert_eq!(
        stellar_strkey::Contract(a.token).to_string().as_str(),
        caravel_deploy::versions::testnet_usdc()
    );
    assert_eq!(a.token_asset, None);
    let engine = m.lane.engine_wasm_hash().unwrap().unwrap();
    let code = hex32(m.env.settlement_wasm.as_deref().unwrap());
    let mut d = desired(&m, &genesis, &keys, &a, engine, code, "c0ffee").unwrap();
    let files = render(
        &m,
        &Resolved {
            template: "perps".into(),
            engine_wasm_hash: engine,
            settlement: a.settlement,
            validator_keys: keys.validators.clone(),
            web: true,
            sequencer_key: None,
        },
        "/opt/caravel",
        1,
    )
    .unwrap();
    d.host.files = files
        .iter()
        .map(|(k, v)| (k.clone(), caravel_runtime::checkpoint::sha256(v.as_bytes())))
        .collect();
    let chain = Chain {
        accounts: BTreeSet::from([d.admin, d.relayer]),
        trustlines: Default::default(),
        tokens: Default::default(),
        contracts: Default::default(),
        wasms: Default::default(),
        token_exists: true,
        settlement_wasm_uploaded: true,
        settlement: Some(OnChain {
            code,
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
    };
    let mut nodes = BTreeMap::new();
    for (n, key, epoch) in [
        ("sequencer".to_string(), None, Some(1)),
        ("relayer".to_string(), None, None),
    ]
    .into_iter()
    .chain(
        d.validators
            .iter()
            .zip(&keys.validators)
            .map(|(n, k)| (n.clone(), Some(*k), None)),
    ) {
        let report = (n != "relayer").then(|| NodeReport {
            lane_id: d.lane_id,
            config_hash: d.config_hash,
            settlement: d.settlement,
            engine_wasm_hash: d.engine_wasm_hash,
            key,
            epoch,
            release: Some("c0ffee".into()),
        });
        nodes.insert(
            n.clone(),
            NodeState {
                running: true,
                report,
                started_with: Some(fingerprint(&d.host.files, &n)),
            },
        );
    }
    let host = Host {
        missing: vec![],
        platform: None,
        release: Some("c0ffee".into()),
        files: d.host.files.clone(),
        nodes,
        has_data: true,
        others: Default::default(),
        taken: None,
        listening: Default::default(),
    };
    let plan = diff(&d, &chain, &host);
    assert!(plan.is_empty(), "{}", plan.render(&d));
    assert!(plan.render(&d).contains("No changes."));
    // Not vacuous: another release, or another signer set, is a change.
    let mut other = host.clone();
    other.release = Some("0ther".into());
    assert!(!diff(&d, &chain, &other).is_empty());
    let mut rotated = d.clone();
    rotated.signers.threshold = 3;
    assert!(!diff(&rotated, &chain, &host).is_empty());
}

/// Lane #1's lane file in a namespace (C-24): its units and Caddy site
/// are its own, so another lane can share the host.
#[test]
fn files_in_a_namespace() {
    let text =
        std::fs::read_to_string(root().join("lanes/perps/config/lane.caravel-perps.testnet.toml"))
            .unwrap()
            .replace(
                "[env.testnet.host]\n",
                "[env.testnet.host]\nnamespace = \"perps\"\n",
            );
    let m = Manifest::parse(&text, "testnet").unwrap();
    let r = Resolved {
        template: "perps".into(),
        engine_wasm_hash: m.lane.engine_wasm_hash().unwrap().unwrap(),
        settlement: [9; 32],
        validator_keys: vec![[1; 32], [2; 32], [3; 32]],
        web: true,
        sequencer_key: None,
    };
    let f = render(&m, &r, &m.env.host.root, 1).unwrap();
    let names: Vec<&str> = f
        .keys()
        .filter(|k| k.starts_with("systemd/") || k.starts_with("caddy/"))
        .map(String::as_str)
        .collect();
    assert_eq!(
        names,
        [
            "caddy/caravel.d/perps.caddy",
            "systemd/caravel-perps-relayer.service",
            "systemd/caravel-perps-sequencer.service",
            "systemd/caravel-perps-validator@.service",
        ]
    );
    let v = &f["systemd/caravel-perps-validator@.service"];
    assert!(
        v.contains("After=network-online.target caravel-perps-sequencer.service"),
        "{v}"
    );
    assert!(
        v.contains("/opt/caravel-perps/config/validator-%i.toml"),
        "{v}"
    );
    assert!(f["sequencer.toml"].contains("/opt/caravel-perps/data/sequencer.sqlite"));
    assert!(f["caddy/caravel.d/perps.caddy"].contains("root * /opt/caravel-perps/web"));
}
