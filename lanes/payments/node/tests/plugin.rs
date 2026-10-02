//! The plugin protocol (M0.6, DEC-073), through the real binary: what
//! `caravel` reads from `plugin info`, the lane file `caravel init` starts
//! from (`plugin example`), and transaction bodies (`plugin body`) that the
//! platform signs with SEP-53 and submits.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

use caravel_core::tx::{sep53_preimage, sep53_tx_message, SigScheme, StandardBody, TxEnvelopeV1};
use caravel_deploy::manifest::Manifest;
use caravel_deploy::template::{InProcess, Plugin, Template};
use caravel_harness::{key, pk, sha256};
use caravel_node::lane_toml::{self, LaneFile};
use caravel_node::plugin::{example_roles, fill_example, Body, Info, PROTOCOL};
use caravel_node::NodeApp;
use caravel_payments::{Transfer, TRANSFER};
use caravel_payments_node::PaymentsApp;
use caravel_runtime::app::LaneApp;
use caravel_runtime::mempool::tx_signature_ok;
use caravel_runtime::sequencer::unhex;
use ed25519_dalek::Signer;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn plugin(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_caravel-payments-node"))
        .arg("plugin")
        .args(args)
        .output()
        .unwrap()
}

fn ok(args: &[&str]) -> String {
    let o = plugin(args);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap()
}

fn g(seed: u8) -> String {
    caravel_runtime::views::g_address(&pk(seed))
}

#[test]
fn info() {
    let i: Info = serde_json::from_str(&ok(&["info"])).unwrap();
    assert_eq!(i.protocol, PROTOCOL);
    assert_eq!(i.template, "payments");
    assert_eq!(i.engine_file, "payments_engine.wasm");
    assert_eq!(i.token_decimals, None);
    assert_eq!(i.version, env!("CARGO_PKG_VERSION"));
}

/// The example, filled as `caravel init` would.
fn example() -> String {
    let text = ok(&["example"]);
    let roles = example_roles(&text).unwrap();
    let want: BTreeSet<String> = ["admin", "relayer", "treasury", "v1", "v2", "v3"]
        .map(String::from)
        .into();
    assert_eq!(roles, want);
    let ids: BTreeMap<_, _> = roles
        .iter()
        .zip(1u8..)
        .map(|(r, seed)| (r.clone(), (format!("acme-{r}"), g(seed))))
        .collect();
    fill_example(&text, "acme-pay-0", 18080, &ids).unwrap()
}

#[test]
fn the_example_is_a_lane_file() {
    let text = example();
    let lane = LaneFile::parse(&text).unwrap();
    let (report, _, _) = lane_toml::genesis(&PaymentsApp, &lane).unwrap();
    assert_eq!(report.lane_name, "acme-pay-0");
    let versions: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root().join("versions.json")).unwrap())
            .unwrap();
    assert_eq!(
        lane.app.as_ref().unwrap().engine_wasm_sha256,
        versions["lanes"]["payments"]["engine_wasm_sha256"]
            .as_str()
            .unwrap(),
        "the example names the engine of record"
    );
    let m = Manifest::parse(&text, "local").unwrap();
    assert_eq!(m.env.admin, "acme-admin");
    assert_eq!(m.env.sequencer.port, 18080);
    let keys: Vec<_> = m.env.validators.iter().map(|v| v.key.as_str()).collect();
    assert_eq!(keys, ["acme-v1", "acme-v2", "acme-v3"]);
    assert_eq!(m.env.relayer.account, "acme-relayer");
}

fn body(args: &[&str]) -> (u8, Vec<u8>) {
    let b: Body = serde_json::from_str(&ok(&[&["body"], args].concat())).unwrap();
    (b.kind, unhex(&b.body).unwrap())
}

#[test]
fn bodies() {
    let to = g(2);
    let want = Transfer {
        to: pk(2),
        amount: 50_000_000,
        memo: 9,
    }
    .encode();
    assert_eq!(
        body(&[
            "--decimals",
            "7",
            "transfer",
            "--to",
            &to,
            "--amount",
            "5",
            "--memo",
            "9"
        ]),
        (TRANSFER, want.clone())
    );
    // Without --decimals, amounts are base units, as `tx` takes them.
    assert_eq!(
        body(&["transfer", "--to", &to, "--amount", "50000000", "--memo", "9"]),
        (TRANSFER, want)
    );
    let w = StandardBody::Withdraw { amount: 12_500_000 };
    assert_eq!(
        body(&["--decimals", "7", "withdraw", "--amount", "1.25"]),
        (w.kind(), w.encode())
    );
    for bad in [
        &["withdraw", "--amount", "1.5"][..],
        &["--decimals", "7", "withdraw", "--amount", "1.00000001"],
        &["transfer", "--to", "nobody", "--amount", "1"],
        &["mint", "--amount", "1"],
    ] {
        assert!(
            !plugin(&[&["body"], bad].concat()).status.success(),
            "{bad:?}"
        );
    }
}

/// `caravel tx`'s path: the plugin's body, the platform's envelope, a SEP-53
/// signature (what `stellar message sign` makes). The engine accepts it.
#[test]
fn an_envelope_around_a_plugin_body_verifies() {
    let text = example();
    let lane = LaneFile::parse(&text).unwrap();
    let (_, config_bytes, _) = lane_toml::genesis(&PaymentsApp, &lane).unwrap();
    let config_hash = sha256(&config_bytes);
    let (kind, body) = body(&[
        "--decimals",
        "7",
        "transfer",
        "--to",
        &g(2),
        "--amount",
        "1",
    ]);
    let mut tx = TxEnvelopeV1 {
        lane_id: lane.lane_id(),
        account: pk(1),
        signer: pk(1),
        nonce: 0,
        expiry_ms: 1,
        kind,
        sig_scheme: SigScheme::Sep53,
        body,
        signature: [0; 64],
    };
    let tx_hash = sha256(&tx.tx_hash_preimage(&config_hash));
    let msg = sha256(&sep53_preimage(&sep53_tx_message(&tx_hash)));
    tx.signature = key(1).sign(&msg).to_bytes();
    assert!(tx_signature_ok(&tx, &config_hash));
    assert!(PaymentsApp.tx_decodes(&tx));
    let bytes = tx.encode().unwrap();
    assert_eq!(TxEnvelopeV1::decode(&bytes).unwrap(), tx);
    // A raw signature over the same hash is not a SEP-53 one.
    tx.signature = key(1).sign(&tx_hash).to_bytes();
    assert!(!tx_signature_ok(&tx, &config_hash));
}

/// The CLI's genesis (the binary, over the document the hosts get) is the
/// linked app's, for every lane file.
#[test]
fn plugin_genesis_is_the_apps() {
    let plugin = Plugin::at(std::path::Path::new(env!(
        "CARGO_BIN_EXE_caravel-payments-node"
    )))
    .unwrap();
    assert_eq!(plugin.name(), PaymentsApp::TEMPLATE);
    assert_eq!(
        plugin.token_decimals(),
        InProcess(PaymentsApp).token_decimals()
    );
    for path in [
        "lanes/payments/config/lane.caravel-payments.local.toml",
        "lanes/payments/node/tests/golden/render/lane.toml",
    ] {
        let lane = LaneFile::load(&root().join(path)).unwrap();
        assert_eq!(
            plugin.genesis(&lane).unwrap(),
            InProcess(PaymentsApp).genesis(&lane).unwrap(),
            "{path}"
        );
    }
    let (kind, _) = plugin
        .body(&["withdraw".into(), "--amount".into(), "1".into()], Some(7))
        .unwrap();
    assert_eq!(kind, caravel_core::tx::kind::WITHDRAW);
}
