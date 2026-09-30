//! `[env.<name>]` tables: what parses, and every rule a table can break.

use caravel_deploy::manifest::{Manifest, Network, Provider, Token, Transport};

const LANE: &str = r#"
[lane]
name = "demo-0"

[app]
template = "demo"
engine_wasm_sha256 = "0101010101010101010101010101010101010101010101010101010101010101"

[node]
block_time_ms = 1000
checkpoint_every_blocks = 10
max_batch_bytes = 96000

[access]
mode = "open"
allowlist = []

[limits]
min_deposit = 10000000
min_withdrawal = 10000000
max_accounts = 256
max_session_keys = 4
max_txs_per_account_per_block = 50
max_entries_per_block = 256
max_block_bytes = 12000
max_pending_withdrawals = 512
exec_cpu_limit = 200000000
exec_mem_limit = 41943040

[demo]
greeting = "hi"
"#;

const ENV: &str = r#"
[env.testnet]
network = "testnet"
admin = "demo-admin"
token = "circle-usdc"
threshold = 2

[env.testnet.settlement_params]
force_inclusion_window_secs = 600
escape_timeout_secs = 1800
min_rotation_delay_secs = 3600
signer_retention_epochs = 2

[[env.testnet.validators]]
name = "1"
key = "demo-v1"

[[env.testnet.validators]]
name = "2"
key = "demo-v2"

[[env.testnet.validators]]
name = "3"
key = "demo-v3"

[env.testnet.relayer]
account = "demo-relayer"

[env.testnet.host]
provider = "ssh"
address = "ops@lane.example"
public_url = "https://lane.example"
"#;

fn file(env: &str) -> String {
    format!("{LANE}{env}")
}

fn err(env: &str) -> String {
    format!(
        "{:#}",
        Manifest::parse(&file(env), "testnet").expect_err("refused")
    )
}

fn secret() -> String {
    stellar_strkey::ed25519::PrivateKey([7; 32])
        .to_string()
        .as_str()
        .to_string()
}

#[test]
fn a_deployment_parses_with_its_defaults() {
    let m = Manifest::parse(&file(ENV), "testnet").unwrap();
    assert_eq!(m.env_name, "testnet");
    let e = &m.env;
    assert_eq!(e.network, Network::Testnet);
    assert_eq!(e.token, Token::Named("circle-usdc".into()));
    assert_eq!(e.validators.len(), 3);
    assert!(e.validators.iter().all(|v| v.weight == 1));
    assert_eq!(e.sequencer.port, 8080);
    // A validator named n listens on the sequencer's port + n.
    let ports: Vec<_> = e.validators.iter().map(|v| v.port(8080).unwrap()).collect();
    assert_eq!(ports, [8081, 8082, 8083]);
    assert_eq!(
        (e.host.provider, e.host.transport),
        (Provider::Ssh, Transport::Ssh)
    );
    assert_eq!(e.host.root, "/opt/caravel");
    // min_deposit comes from [limits] unless the table sets it.
    assert_eq!(m.min_deposit().unwrap(), 10_000_000);
    assert_eq!(m.rpc_url(), caravel_deploy::versions::testnet_rpc());
    let own = ENV.replace(
        "signer_retention_epochs = 2",
        "signer_retention_epochs = 2\nmin_deposit = 5",
    );
    assert_eq!(
        Manifest::parse(&file(&own), "testnet")
            .unwrap()
            .min_deposit()
            .unwrap(),
        5
    );
    // The lane's genesis is untouched by the table (DEC-065).
    assert!(m.lane.raw.get("env").is_none());
}

#[test]
fn a_missing_deployment_names_the_ones_there_are() {
    let e = format!("{:#}", Manifest::parse(&file(ENV), "local").unwrap_err());
    assert!(
        e.contains("no [env.local]") && e.contains("[env.testnet]"),
        "{e}"
    );
    let e = format!("{:#}", Manifest::parse(LANE, "testnet").unwrap_err());
    assert!(e.contains("no deployments"), "{e}");
}

#[test]
fn secrets_are_refused_wherever_they_are() {
    let s = secret();
    for (from, to) in [
        ("admin = \"demo-admin\"", format!("admin = \"{s}\"")),
        ("key = \"demo-v2\"", format!("key = \"{s}\"")),
        ("account = \"demo-relayer\"", format!("account = \"{s}\"")),
        (
            "account = \"demo-relayer\"",
            format!("account = \"demo-relayer\"\n[[env.testnet.relayer.feeds]]\nmodule = \"x.js\"\noptions = {{ key = \"{s}\" }}"),
        ),
        (
            "admin = \"demo-admin\"",
            "admin = \"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about\"".to_string(),
        ),
    ] {
        let env = ENV.replacen(from, &to, 1);
        assert_ne!(env, ENV);
        let e = err(&env);
        assert!(e.contains("holds a secret"), "{e}");
        assert!(!e.contains(&s), "the error must not echo the secret: {e}");
    }
}

#[test]
fn key_fields_name_identities_not_keys() {
    let g = "GCQJVJPUPJTVTABP7FK7RXBNFIKKLSM5EO7JP6DECJ77SOBUKWSPB64N";
    let hex = "ab".repeat(32);
    for (from, to) in [
        ("admin = \"demo-admin\"", format!("admin = \"{g}\"")),
        ("key = \"demo-v1\"", format!("key = \"{hex}\"")),
        (
            "account = \"demo-relayer\"",
            "account = \"-relayer\"".to_string(),
        ),
    ] {
        let e = err(&ENV.replacen(from, &to, 1));
        assert!(e.contains("name a Stellar CLI identity"), "{e}");
    }
    // A hash elsewhere is no secret: the lane's engine hash, or a feed option.
    let env = ENV.replace(
        "account = \"demo-relayer\"",
        &format!("account = \"demo-relayer\"\n[[env.testnet.relayer.feeds]]\nmodule = \"x.js\"\noptions = {{ pin = \"{hex}\" }}"),
    );
    Manifest::parse(&file(&env), "testnet").unwrap();
}

#[test]
fn every_rule_is_checked() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "network = \"testnet\"",
            "network = \"mainnet\"",
            "testnet only",
        ),
        ("token = \"circle-usdc\"", "token = { local = \"USDC\" }", "token.local is for local networks"),
        ("token = \"circle-usdc\"", "token = \"usdc\"", "a known token is"),
        ("token = \"circle-usdc\"", "token = { asset = \"USDC\" }", "CODE:ISSUER"),
        ("token = \"circle-usdc\"", "token = { asset = \"THISCODEISTOOLONG:GCQJVJPUPJTVTABP7FK7RXBNFIKKLSM5EO7JP6DECJ77SOBUKWSPB64N\" }", "CODE:ISSUER"),
        ("token = \"circle-usdc\"", "token = { contract = \"CXYZ\" }", "not a C… contract"),
        (
            "threshold = 2",
            "threshold = 4",
            "between 1 and the validators' total weight 3",
        ),
        ("threshold = 2", "threshold = 0", "between 1 and"),
        ("key = \"demo-v2\"", "key = \"demo-v1\"", "appears twice"),
        ("name = \"2\"", "name = \"1\"", "appears twice"),
        (
            "key = \"demo-v1\"",
            "key = \"demo-v1\"\nweight = 0",
            "weight must be at least 1",
        ),
        (
            "escape_timeout_secs = 1800",
            "escape_timeout_secs = 0",
            "must be positive",
        ),
        ("address = \"ops@lane.example\"\n", "", "needs user@host"),
        (
            "address = \"ops@lane.example\"",
            "address = \"caravel-1\"\ntransport = \"gcloud-iap\"",
            "needs project and zone",
        ),
        (
            "provider = \"ssh\"",
            "provider = \"local\"",
            "runs on this machine",
        ),
        ("public_url", "root = \"opt\"\npublic_url", "absolute path"),
        (
            "public_url",
            "root = \"/opt/caravel /\"\npublic_url",
            "absolute path",
        ),
        ("public_url", "root = \"/\"\npublic_url", "absolute path"),
        (
            "public_url",
            "root = \"/opt/../etc\"\npublic_url",
            "absolute path",
        ),
        (
            "public_url",
            "root = \"/opt/x;rm\"\npublic_url",
            "absolute path",
        ),
        (
            "admin = \"demo-admin\"",
            "admin = \"demo-admin\"\nsettlement = \"CXYZ\"",
            "not a C... contract",
        ),
        (
            "admin = \"demo-admin\"",
            "admin = \"demo-admin\"\nsettlement_wasm = \"abc\"",
            "not a sha256",
        ),
        (
            "admin = \"demo-admin\"",
            "admin = \"demo-admin\"\nextra = 1",
            "unknown field",
        ),
        ("name = \"3\"", "name = \"third\"", "set its port"),
        (
            "key = \"demo-v3\"",
            "key = \"demo-v3\"\nport = 8081",
            "port 8081 is already taken",
        ),
    ];
    for (from, to, why) in cases {
        let env = ENV.replacen(from, to, 1);
        assert_ne!(&env, ENV, "{from:?} not in the table");
        let e = err(&env);
        assert!(e.contains(why), "{why}: {e}");
    }
    // Several broken rules are reported together.
    let e = err(&ENV
        .replace("threshold = 2", "threshold = 9")
        .replace("escape_timeout_secs = 1800", "escape_timeout_secs = 0"));
    assert!(
        e.contains("threshold 9") && e.contains("escape_timeout_secs"),
        "{e}"
    );
}

#[test]
fn a_local_deployment() {
    let env = ENV
        .replace("[env.testnet]", "[env.local]")
        .replace("[env.testnet.", "[env.local.")
        .replace("[[env.testnet.", "[[env.local.")
        .replace("network = \"testnet\"", "network = \"local\"")
        .replace("token = \"circle-usdc\"", "token = { local = \"USDC\" }")
        .replace(
            "provider = \"ssh\"\naddress = \"ops@lane.example\"",
            "provider = \"local\"",
        );
    let m = Manifest::parse(&file(&env), "local").unwrap();
    assert_eq!(m.env.host.provider, Provider::Local);
    assert_eq!(m.rpc_url(), "http://localhost:8000/rpc");
    assert_eq!(
        m.env.network.passphrase(),
        "Standalone Network ; February 2017"
    );
}

#[test]
fn any_token_can_settle_a_lane() {
    // A Stellar asset (its asset contract), a SEP-41 contract, or Circle's USDC.
    for token in [
        "{ asset = \"EURC:GCQJVJPUPJTVTABP7FK7RXBNFIKKLSM5EO7JP6DECJ77SOBUKWSPB64N\" }",
        "{ asset = \"LONGERCODE12:GCQJVJPUPJTVTABP7FK7RXBNFIKKLSM5EO7JP6DECJ77SOBUKWSPB64N\" }",
        "{ contract = \"CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA\" }",
        "\"circle-usdc\"",
    ] {
        let env = ENV.replace("token = \"circle-usdc\"", &format!("token = {token}"));
        Manifest::parse(&file(&env), "testnet").unwrap_or_else(|e| panic!("{token}: {e:#}"));
    }
}
