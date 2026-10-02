//! The plugin protocol (M0.6, DEC-073) for perps, through the real binary:
//! `plugin info`, the example lane file for `caravel init perps`, and bodies
//! that the platform wraps in its envelope and signs with SEP-53.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

use caravel_core::tx::{sep53_preimage, sep53_tx_message, SigScheme, TxEnvelopeV1};
use caravel_deploy::manifest::Manifest;
use caravel_deploy::template::{InProcess, Plugin, Template};
use caravel_node::lane_toml::{self, LaneFile};
use caravel_node::plugin::{example_roles, fill_example, Body, Info, PROTOCOL};
use caravel_node::NodeApp;
use caravel_perps_node::PerpsApp;
use caravel_runtime::app::LaneApp;
use caravel_runtime::checkpoint::sha256;
use caravel_runtime::mempool::tx_signature_ok;
use caravel_runtime::sequencer::unhex;
use caravel_types::tx::{LaneTxV1, PlaceOrder, Side, Tif, TxBody};
use ed25519_dalek::{Signer, SigningKey};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn plugin(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_caravel-perps-node"))
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

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn pk(seed: u8) -> [u8; 32] {
    key(seed).verifying_key().to_bytes()
}

fn g(seed: u8) -> String {
    caravel_runtime::views::g_address(&pk(seed))
}

#[test]
fn info() {
    let i: Info = serde_json::from_str(&ok(&["info"])).unwrap();
    assert_eq!(i.protocol, PROTOCOL);
    assert_eq!(i.template, "perps");
    assert_eq!(i.engine_file, "perps_engine.wasm");
    assert_eq!(i.token_decimals, Some(7));
}

fn example() -> String {
    let text = ok(&["example"]);
    let roles = example_roles(&text).unwrap();
    let want: BTreeSet<String> = [
        "admin", "backstop", "oracle", "relayer", "treasury", "v1", "v2", "v3",
    ]
    .map(String::from)
    .into();
    assert_eq!(roles, want);
    let ids: BTreeMap<_, _> = roles
        .iter()
        .zip(1u8..)
        .map(|(r, seed)| (r.clone(), (format!("acme-{r}"), g(seed))))
        .collect();
    fill_example(&text, "acme-perps-0", 18080, &ids).unwrap()
}

#[test]
fn the_example_is_a_lane_file() {
    let text = example();
    let lane = LaneFile::parse(&text).unwrap();
    let (report, _, _) = lane_toml::genesis(&PerpsApp, &lane).unwrap();
    assert_eq!(report.lane_name, "acme-perps-0");
    let versions: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root().join("versions.json")).unwrap())
            .unwrap();
    assert_eq!(
        lane.app.as_ref().unwrap().engine_wasm_sha256,
        versions["artifacts"]["engine_wasm_sha256"]
            .as_str()
            .unwrap(),
        "the example names the engine of record"
    );
    let m = Manifest::parse(&text, "local").unwrap();
    // The oracle key in genesis and the relayer's feed key are one identity.
    assert_eq!(
        m.env.relayer.feed_keys["CARAVEL_ORACLE_SECRET"],
        "acme-oracle"
    );
    assert_eq!(m.env.relayer.feeds.len(), 1);
    // Apart from its keys and name, the example is the local lane's genesis.
    let local =
        std::fs::read_to_string(root().join("lanes/perps/config/lane.caravel-perps.local.toml"))
            .unwrap();
    let local = LaneFile::parse(&local).unwrap();
    for section in ["node", "access", "limits"] {
        assert_eq!(lane.raw[section], local.raw[section], "[{section}]");
    }
    let perps = |l: &LaneFile| {
        let mut t = l.raw["perps"].as_table().unwrap().clone();
        t.remove("accounts");
        t.remove("oracle");
        t
    };
    assert_eq!(perps(&lane), perps(&local));
}

fn body(args: &[&str]) -> (u8, Vec<u8>) {
    let b: Body = serde_json::from_str(&ok(&[&["body"], args].concat())).unwrap();
    (b.kind, unhex(&b.body).unwrap())
}

/// The bytes of a perps body, through the frozen codec.
fn perps_body(b: TxBody) -> (u8, Vec<u8>) {
    let tx = LaneTxV1 {
        lane_id: [0; 32],
        account: [0; 32],
        signer: [0; 32],
        nonce: 0,
        expiry_ms: 0,
        sig_scheme: caravel_types::tx::SigScheme::RawEd25519,
        body: b,
        signature: [0; 64],
    };
    let bytes = tx.encode();
    (b.kind() as u8, bytes[117..bytes.len() - 64].to_vec())
}

#[test]
fn bodies() {
    assert_eq!(
        body(&[
            "--decimals",
            "7",
            "place-order",
            "--market",
            "1",
            "--side",
            "buy",
            "--price",
            "6.5",
            "--lots",
            "10",
        ]),
        perps_body(TxBody::PlaceOrder(PlaceOrder {
            market_id: 1,
            side: Side::Buy,
            tif: Tif::Gtc,
            reduce_only: false,
            price: 65_000_000,
            lots: 10,
            client_order_id: 0,
        }))
    );
    assert_eq!(
        body(&["--decimals", "7", "withdraw", "--amount", "100"]),
        perps_body(TxBody::Withdraw {
            amount: 1_000_000_000
        })
    );
    assert_eq!(
        body(&["cancel-all", "--market", "all"]),
        perps_body(TxBody::CancelAll {
            market_id: caravel_types::tx::ALL_MARKETS
        })
    );
    assert!(!plugin(&["body", "place-order", "--market", "1"])
        .status
        .success());
}

#[test]
fn an_envelope_around_a_plugin_body_verifies() {
    let text = example();
    let lane = LaneFile::parse(&text).unwrap();
    let (_, config_bytes, _) = lane_toml::genesis(&PerpsApp, &lane).unwrap();
    let config_hash = sha256(&config_bytes);
    let (kind, body) = body(&["--decimals", "7", "withdraw", "--amount", "1"]);
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
    tx.signature = key(1)
        .sign(&sha256(&sep53_preimage(&sep53_tx_message(&tx_hash))))
        .to_bytes();
    assert!(tx_signature_ok(&tx, &config_hash));
    assert!(PerpsApp.tx_decodes(&tx));
}

/// The CLI's genesis (the binary, over the document the hosts get) is the
/// linked app's, for every lane file.
#[test]
fn plugin_genesis_is_the_apps() {
    let plugin = Plugin::at(std::path::Path::new(env!(
        "CARGO_BIN_EXE_caravel-perps-node"
    )))
    .unwrap();
    assert_eq!(plugin.name(), PerpsApp::TEMPLATE);
    assert_eq!(
        plugin.token_decimals(),
        InProcess(PerpsApp).token_decimals()
    );
    for path in [
        "lanes/perps/config/lane.caravel-perps.testnet.toml",
        "lanes/perps/config/lane.caravel-perps.local.toml",
        "lanes/perps/node/tests/fixtures/lane.caravel-perps.testnet.m0.toml",
        "lanes/perps/node/tests/fixtures/lane.caravel-perps.local.m0.toml",
    ] {
        let lane = LaneFile::load(&root().join(path)).unwrap();
        assert_eq!(
            plugin.genesis(&lane).unwrap(),
            InProcess(PerpsApp).genesis(&lane).unwrap(),
            "{path}"
        );
    }
    let (kind, _) = plugin
        .body(&["withdraw".into(), "--amount".into(), "1".into()], Some(7))
        .unwrap();
    assert_eq!(kind, caravel_core::tx::kind::WITHDRAW);
}
