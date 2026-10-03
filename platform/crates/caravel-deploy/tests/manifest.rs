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

/// Deployments composed with `include` and `extends` (M0.6): one shared
/// base, a local and a testnet deployment of the same lane.
#[test]
fn composed_deployments() {
    let dir = tempfile::tempdir().unwrap();
    // The base holds what both share; each deployment says where it runs.
    let host = "\n[env.testnet.host]\nprovider = \"ssh\"\naddress = \"ops@lane.example\"\npublic_url = \"https://lane.example\"\n";
    let base = ENV
        .replace(host, "\n")
        .replace("[env.testnet]", "[env.base]\nabstract = true")
        .replace("[env.testnet.", "[env.base.")
        .replace("[[env.testnet.", "[[env.base.");
    assert!(!base.contains("host"));
    std::fs::write(dir.path().join("base.toml"), base).unwrap();
    let lane = format!(
        "include = [\"base.toml\"]\n{LANE}
[env.testnet]
extends = \"base\"
default = true
host = {{ provider = \"ssh\", address = \"ops@lane.example\", public_url = \"https://lane.example\" }}

[env.local]
extends = \"base\"
network = \"local\"
token = {{ local = \"USDC\" }}
host = {{ provider = \"local\" }}
[env.local.settlement_params]
force_inclusion_window_secs = 20
"
    );
    let path = dir.path().join("lane.toml");
    std::fs::write(&path, &lane).unwrap();
    let testnet = Manifest::load(&path, "testnet").unwrap();
    assert!(testnet.env.default);
    assert_eq!(testnet.env.network, Network::Testnet);
    assert_eq!(testnet.env.host.provider, Provider::Ssh);
    assert_eq!(testnet.env.validators.len(), 3);
    let local = Manifest::load(&path, "local").unwrap();
    assert!(!local.env.default, "default is not inherited");
    assert_eq!(local.env.network, Network::Local);
    assert_eq!(
        local.env.token,
        Token::Local {
            local: "USDC".into()
        }
    );
    assert_eq!(local.env.host.provider, Provider::Local);
    assert_eq!(local.env.host.address, None);
    // A table merges: the base's other params stay.
    assert_eq!(local.env.settlement_params.force_inclusion_window_secs, 20);
    assert_eq!(local.env.settlement_params.escape_timeout_secs, 1800);
    assert_eq!(local.env.relayer.account, "demo-relayer");
    // The base itself can't be planned.
    let e = format!("{:#}", Manifest::load(&path, "base").unwrap_err());
    assert!(e.contains("[env.base] is abstract"), "{e}");
    // The genesis is the lane file's own.
    assert_eq!(local.lane.raw, testnet.lane.raw);
    assert!(!local.lane.raw.contains_key("include"));
    // A typo in extends points at the line.
    std::fs::write(
        &path,
        lane.replace("extends = \"base\"\nnetwork", "extends = \"bsae\"\nnetwork"),
    )
    .unwrap();
    let e = format!("{:#}", Manifest::load(&path, "local").unwrap_err());
    assert!(
        e.contains("extends \"bsae\", which the lane file doesn't have"),
        "{e}"
    );
    assert!(e.contains("lane.toml:"), "{e}");
    assert!(e.contains("did you mean \"base\"?"), "{e}");
}

/// Vars, locals, for_each and a deployment's own [node] (M0.6, C-08).
#[test]
fn deployments_from_vars() {
    let lane = format!(
        "{LANE}
[vars.validators]
type = \"list\"
default = [\"1\", \"2\", \"3\"]
[vars.network]
type = \"string\"
default = \"testnet\"

[locals]
prefix = \"demo-${{env.name}}\"

[env.testnet]
network = \"${{var.network}}\"
admin = \"${{local.prefix}}-admin\"
token = \"circle-usdc\"
threshold = \"${{length(var.validators) * 2 / 3 + 1}}\"
relayer = {{ account = \"${{local.prefix}}-relayer\" }}
host = {{ provider = \"ssh\", address = \"ops@lane.example\" }}
node = {{ checkpoint_every_blocks = 5 }}
[env.testnet.settlement_params]
force_inclusion_window_secs = 600
escape_timeout_secs = 1800
min_rotation_delay_secs = 3600
signer_retention_epochs = 2
[env.testnet.validators]
for_each = \"${{var.validators}}\"
name = \"${{each.value}}\"
key = \"${{local.prefix}}-v${{each.value}}\"
"
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lane.toml");
    std::fs::write(&path, &lane).unwrap();
    let m = Manifest::load(&path, "testnet").unwrap();
    assert_eq!(m.env.admin, "demo-testnet-admin");
    assert_eq!(m.env.threshold, 3);
    let keys: Vec<_> = m.env.validators.iter().map(|v| v.key.as_str()).collect();
    assert_eq!(
        keys,
        ["demo-testnet-v1", "demo-testnet-v2", "demo-testnet-v3"]
    );
    assert_eq!(
        m.vars,
        r#"network = "testnet", validators = ["1", "2", "3"]"#
    );
    // The deployment's [node] over the lane file's; genesis is the same.
    assert_eq!(m.lane.node.checkpoint_every_blocks, 5);
    assert_eq!(m.lane.raw["node"]["block_time_ms"].as_integer(), Some(1000));
    let plain = Manifest::parse(&file(ENV), "testnet").unwrap();
    assert_eq!(m.lane.lane_id(), plain.lane.lane_id());
    for k in ["lane", "app", "access", "limits", "demo"] {
        assert_eq!(m.lane.raw[k], plain.lane.raw[k], "[{k}]");
    }

    // Inputs: a rotation is a --var.
    let rotated = caravel_deploy::manifest::Inputs {
        vars: vec![("validators".into(), r#"["1","2","4"]"#.into())],
        ..Default::default()
    };
    let r = Manifest::load_with(&path, "testnet", &rotated).unwrap();
    assert_eq!(r.env.validators[2].key, "demo-testnet-v4");

    // Mainnet and secrets are refused when a var brings them, too.
    let main = caravel_deploy::manifest::Inputs {
        vars: vec![("network".into(), "mainnet".into())],
        ..Default::default()
    };
    let e = format!(
        "{:#}",
        Manifest::load_with(&path, "testnet", &main).unwrap_err()
    );
    assert!(e.contains("testnet only"), "{e}");
    let secret = stellar_strkey::ed25519::PrivateKey([7; 32]).to_string();
    let secret = secret.as_str();
    std::fs::write(
        &path,
        lane.replace("default = \"testnet\"", &format!("default = \"{secret}\"")),
    )
    .unwrap();
    let e = format!("{:#}", Manifest::load(&path, "testnet").unwrap_err());
    assert!(e.contains("vars.network") && !e.contains(secret), "{e}");
    std::fs::write(&path, &lane).unwrap();
    let via_flag = caravel_deploy::manifest::Inputs {
        vars: vec![("network".into(), secret.into())],
        ..Default::default()
    };
    let e = format!(
        "{:#}",
        Manifest::load_with(&path, "testnet", &via_flag).unwrap_err()
    );
    assert!(e.contains("--var network") && !e.contains(secret), "{e}");
    // A bad node setting names the deployment.
    std::fs::write(
        &path,
        lane.replace("checkpoint_every_blocks = 5", "block_time_ms = 10"),
    )
    .unwrap();
    let e = format!("{:#}", Manifest::load(&path, "testnet").unwrap_err());
    assert!(
        e.contains("[env.testnet.node]") && e.contains("block_time_ms"),
        "{e}"
    );
}

/// A rule broken in a deployment points at where its value is written,
/// inherited or not.
#[test]
fn problems_point_at_the_lane_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lane.toml");
    let base = ENV
        .replace("[env.testnet]", "[env.base]\nabstract = true")
        .replace("[env.testnet.", "[env.base.")
        .replace("[[env.testnet.", "[[env.base.");
    let write = |extra: &str| {
        std::fs::write(
            &path,
            format!("{LANE}{base}\n[env.testnet]\nextends = \"base\"\n{extra}"),
        )
        .unwrap();
    };
    write("threshold = 9\n");
    let e = format!("{:#}", Manifest::load(&path, "testnet").unwrap_err());
    assert!(e.contains("threshold") && e.contains("lane.toml:"), "{e}");
    let line = |e: &str| -> usize {
        let at = e.split("lane.toml:").nth(1).unwrap();
        at.split(':').next().unwrap().parse().unwrap()
    };
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().nth(line(&e) - 1).unwrap(), "threshold = 9");
    // An inherited value says where it came from.
    write("");
    std::fs::write(
        &path,
        std::fs::read_to_string(&path).unwrap().replace(
            "provider = \"ssh\"",
            "provider = \"ssh\"\ntransport = \"gcloud-iap\"",
        ),
    )
    .unwrap();
    let e = format!("{:#}", Manifest::load(&path, "testnet").unwrap_err());
    assert!(
        e.contains("gcloud-iap") && e.contains("(from [env.base])"),
        "{e}"
    );
    // A typo'd field too.
    write("tresholdd = 2\n");
    let e = format!("{:#}", Manifest::load(&path, "testnet").unwrap_err());
    assert!(
        e.contains("unknown field `tresholdd`") && e.contains("lane.toml:"),
        "{e}"
    );
}

/// Values known once keys and addresses are (M0.6, C-10): a relayer feed
/// that names the settlement contract, and outputs.
#[test]
fn feeds_and_outputs_from_addresses() {
    use caravel_deploy::attrs::attributes;
    use caravel_deploy::deploy::{addresses, Keys};
    use caravel_lanefile::expr::Value;

    let lane = format!(
        "{LANE}
[outputs]
settlement = \"${{contract.settlement.address}}\"
api = {{ value = \"${{node.sequencer.url}}\", description = \"the lane's API\" }}
admin = \"${{account.admin.public_key}}\"
sequencer_key = \"${{node.sequencer.key}}\"
lane_id = \"${{lane.id}}\"
{}
[[env.testnet.relayer.feeds]]
module = \"feeds/x.js\"
options = {{ settlement = \"${{contract.settlement.address}}\", lane = \"${{lane.name}}\" }}
",
        ENV
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lane.toml");
    std::fs::write(&path, &lane).unwrap();
    let mut m = Manifest::load(&path, "testnet").unwrap();
    assert_eq!(m.deferred, ["relayer.feeds[0].options.settlement"]);
    assert_eq!(
        m.env.relayer.feeds[0]["options"]["lane"].as_str(),
        Some("demo-0")
    );
    let keys = Keys {
        admin: [0xA0; 32],
        relayer: [0xA1; 32],
        validators: vec![[1; 32], [2; 32], [3; 32]],
        accounts: Default::default(),
        sequencer: Some([5; 32]),
    };
    let a = addresses(&m, &keys.admin).unwrap();
    let attrs = attributes(&m, &keys, &a, None, None);
    m.finish(&attrs).unwrap();
    let settlement = stellar_strkey::Contract(a.settlement).to_string();
    assert_eq!(
        m.env.relayer.feeds[0]["options"]["settlement"].as_str(),
        Some(settlement.as_str())
    );
    let o = m.outputs(&attrs).unwrap();
    let get = |n: &str| {
        o.iter()
            .find(|x| x.name == n)
            .map(|x| x.value.clone())
            .unwrap()
    };
    assert_eq!(get("settlement"), Value::Str(settlement.to_string()));
    assert_eq!(get("api"), Value::Str("https://lane.example".into()));
    // What a validator run elsewhere checks requests against (DEC-095).
    assert_eq!(
        get("sequencer_key"),
        Value::Str(
            stellar_strkey::ed25519::PublicKey([5; 32])
                .to_string()
                .as_str()
                .into()
        )
    );
    assert_eq!(
        get("admin"),
        Value::Str(
            stellar_strkey::ed25519::PublicKey([0xA0; 32])
                .to_string()
                .as_str()
                .into()
        )
    );
    assert_eq!(
        get("lane_id"),
        Value::Str(
            m.lane
                .lane_id()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        )
    );
    // Not where the value is needed earlier.
    std::fs::write(
        &path,
        lane.replace(
            "admin = \"demo-admin\"",
            "admin = \"${contract.settlement.address}\"",
        ),
    )
    .unwrap();
    let e = format!("{:#}", Manifest::load(&path, "testnet").unwrap_err());
    assert!(
        e.contains("admin uses a value known only once") && e.contains("lane.toml:"),
        "{e}"
    );
}

#[test]
fn declared_accounts_parse_with_their_defaults() {
    let env = format!(
        "{ENV}\n[env.testnet.accounts.alice]\ntrustlines = [\"settlement\"]\nbalances = {{ settlement = \"12.5\" }}\n\n[env.testnet.accounts.bob]\nidentity = \"demo-bob\"\nfund = false\ndepends_on = [\"account.alice\"]\n"
    );
    let m = Manifest::parse(&file(&env), "testnet").unwrap();
    let alice = &m.env.accounts["alice"];
    assert_eq!(alice.identity_of("alice"), "alice");
    assert_eq!(alice.fund, None);
    assert_eq!(alice.balances["settlement"], "12.5");
    let bob = &m.env.accounts["bob"];
    assert_eq!(bob.identity_of("bob"), "demo-bob");
    assert_eq!(bob.fund, Some(false));
    assert_eq!(bob.depends_on, ["account.alice"]);
}

#[test]
fn declared_accounts_are_checked() {
    let bad = |table: &str| err(&format!("{ENV}\n{table}"));
    assert!(bad("[env.testnet.accounts.admin]\n").contains("the deployment's own account"));
    assert!(bad("[env.testnet.accounts.x]\ntrustlines = [\"USDC\"]\n")
        .contains("is \"settlement\", a declared token's name, or CODE:ISSUER"));
    assert!(bad(
        "[env.testnet.accounts.x]\ntrustlines = [\"settlement\"]\nbalances = { xlm = \"1\" }\n"
    )
    .contains("\"settlement\" or a declared token's name"));
    assert!(bad("[env.testnet.accounts.x]\ntrustlines = [\"settlement\"]\nbalances = { settlement = \"1.5x\" }\n")
        .contains("an amount in token units"));
    assert!(
        bad("[env.testnet.accounts.x]\nbalances = { settlement = \"1\" }\n")
            .contains("add \"settlement\" to its trustlines")
    );
    let e = bad(&format!(
        "[env.testnet.accounts.x]\nidentity = \"{}\"\n",
        secret()
    ));
    assert!(
        e.contains("holds a secret") && !e.contains(&secret()),
        "{e}"
    );
    assert!(bad("[env.testnet.accounts.x]\nfunded = true\n").contains("unknown field"));
    // A contract token has no trustline and no balance this tool reads.
    let contract = ENV.replace(
        "token = \"circle-usdc\"",
        "token = { contract = \"CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA\" }",
    );
    let e = err(&format!(
        "{contract}\n[env.testnet.accounts.x]\ntrustlines = [\"settlement\"]\n"
    ));
    assert!(e.contains("no trustline to it"), "{e}");
}

#[test]
fn declared_tokens_are_checked_and_can_settle_a_lane() {
    let tokens = "[env.testnet.tokens.usd]\ncode = \"USD\"\nissuer = \"admin\"\n\n[env.testnet.tokens.eur]\ncode = \"EUR\"\nissuer = \"treasury\"\n\n[env.testnet.accounts.treasury]\n\n[env.testnet.accounts.alice]\ntrustlines = [\"usd\", \"eur\"]\nbalances = { usd = \"10\", eur = \"2\" }\n";
    let m = Manifest::parse(&file(&format!("{ENV}\n{tokens}")), "testnet").unwrap();
    assert_eq!(m.env.tokens["eur"].issuer, "treasury");
    // `token = "usd"`: the admin issues it, so it reads as `{ local = "USD" }`
    // (CODE:<admin>), on testnet too.
    let env = ENV.replace("token = \"circle-usdc\"", "token = \"usd\"");
    let m = Manifest::parse(&file(&format!("{env}\n{tokens}")), "testnet").unwrap();
    assert_eq!(
        m.env.token,
        Token::Local {
            local: "USD".into()
        }
    );
    // A settlement token must be the admin's or a G… issuer's.
    let env = ENV.replace("token = \"circle-usdc\"", "token = \"eur\"");
    assert!(err(&format!("{env}\n{tokens}")).contains("issued by the admin or a G… address"));
    let g = "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";
    let env = ENV.replace("token = \"circle-usdc\"", "token = \"x\"");
    let m = Manifest::parse(
        &file(&format!(
            "{env}\n[env.testnet.tokens.x]\ncode = \"USDC\"\nissuer = \"{g}\"\n"
        )),
        "testnet",
    )
    .unwrap();
    assert_eq!(
        m.env.token,
        Token::Asset {
            asset: format!("USDC:{g}")
        }
    );
    let bad = |table: &str| err(&format!("{ENV}\n{table}"));
    assert!(
        bad("[env.testnet.tokens.settlement]\ncode = \"X\"\nissuer = \"admin\"\n")
            .contains("not `settlement`")
    );
    assert!(
        bad("[env.testnet.tokens.t]\ncode = \"TOO-LONG-CODE\"\nissuer = \"admin\"\n")
            .contains("1–12 letters or digits")
    );
    assert!(
        bad("[env.testnet.tokens.t]\ncode = \"T\"\nissuer = \"nobody\"\n")
            .contains("a declared account's name, or a G… address")
    );
    assert!(bad("[env.testnet.accounts.x]\nbalances = { usd = \"1\" }\n[env.testnet.tokens.usd]\ncode = \"USD\"\nissuer = \"admin\"\n")
        .contains("add \"usd\" to its trustlines"));
}

#[test]
fn declared_contracts_are_checked() {
    let ok = format!(
        "{ENV}\n[env.testnet.contracts.oracle]\nwasm = \"wasm/oracle.wasm\"\nsalt = \"v1\"\nlifecycle = {{ prevent_destroy = true }}\n[env.testnet.contracts.oracle.args]\nadmin = \"${{account.admin.public_key}}\"\ndecimals = 7\nfeeds = [\"BTC\", \"ETH\"]\n"
    );
    let m = Manifest::parse(&file(&ok), "testnet").unwrap();
    let c = &m.env.contracts["oracle"];
    assert_eq!(c.deployer(), "admin");
    assert!(c.lifecycle.prevent_destroy);
    // Arguments as `--name value`: numbers as written, lists as JSON; a
    // value known only with the keys stays as written until then.
    let args = c.arg_strings();
    assert!(args.contains(&("decimals".into(), "7".into())));
    assert!(args.contains(&("feeds".into(), "[\"BTC\",\"ETH\"]".into())));
    let bad = |table: &str| err(&format!("{ENV}\n{table}"));
    assert!(
        bad("[env.testnet.contracts.settlement]\nwasm = \"x.wasm\"\n").contains("not `settlement`")
    );
    assert!(bad("[env.testnet.contracts.c]\nwasm = \"x.zip\"\n").contains("a .wasm file"));
    assert!(
        bad("[env.testnet.contracts.c]\nwasm = \"x.wasm\"\ndeployer = \"nobody\"\n")
            .contains("\"admin\" or a declared account's name")
    );
    assert!(
        bad("[env.testnet.contracts.c]\nwasm = \"x.wasm\"\nargs = { \"a-b\" = 1 }\n")
            .contains("letters, digits and '_'")
    );
    // Values known only with the keys are refused elsewhere in a deployment.
    assert!(bad(
        "[env.testnet.contracts.c]\nwasm = \"x.wasm\"\nsalt = \"${account.admin.public_key}\"\n"
    )
    .contains("only relayer feeds, contracts' args, web.config and [outputs] can"));
    let hash = "ab".repeat(32);
    let m = Manifest::parse(
        &file(&format!(
            "{ENV}\n[env.testnet.contracts.c]\nwasm = \"{hash}\"\n"
        )),
        "testnet",
    )
    .unwrap();
    assert_eq!(m.env.contracts["c"].wasm_hash(), Some([0xab; 32]));
}

/// Several hosts (C-22): the sequencer's host and the validators placed
/// on others, each reachable from the other.
#[test]
fn several_hosts() {
    let hosts = ENV.replace(
        "[env.testnet.host]\nprovider = \"ssh\"\naddress = \"ops@lane.example\"\npublic_url = \"https://lane.example\"\n",
        "[env.testnet.sequencer]\nkey = \"demo-seq\"\nhost = \"a\"\n\n[env.testnet.hosts.a]\nprovider = \"ssh\"\naddress = \"ops@a.example\"\npublic_url = \"https://lane.example\"\nprivate_address = \"10.0.0.2\"\n\n[env.testnet.hosts.b]\nprovider = \"ssh\"\naddress = \"ops@b.example\"\nprivate_address = \"10.0.0.3\"\n",
    );
    let placed = hosts.replace(
        "name = \"3\"\nkey = \"demo-v3\"",
        "name = \"3\"\nkey = \"demo-v3\"\nhost = \"b\"",
    );
    let m = Manifest::parse(&file(&placed), "testnet").unwrap();
    let e = &m.env;
    // `host` is the sequencer's; the others are by name.
    assert_eq!(e.primary_host(), "a");
    assert_eq!(e.host.address.as_deref(), Some("ops@a.example"));
    let others: Vec<_> = e.other_hosts().into_iter().map(|(n, _)| n).collect();
    assert_eq!(others, ["b"]);
    assert_eq!(e.host_of("sequencer"), "a");
    assert_eq!(e.host_of("validator-1"), "a");
    assert_eq!(e.host_of("validator-3"), "b");
    // Across hosts, a node is reached at its host's private address and
    // listens there, also for its own host's nodes; else at loopback.
    let url = |from: &str, node: &str| e.url_of(from, node);
    assert_eq!(
        url("b", "sequencer").as_deref(),
        Some("http://10.0.0.2:8080")
    );
    assert_eq!(
        url("a", "validator-3").as_deref(),
        Some("http://10.0.0.3:8083")
    );
    assert_eq!(
        url("a", "sequencer").as_deref(),
        Some("http://10.0.0.2:8080")
    );
    assert_eq!(
        url("a", "validator-1").as_deref(),
        Some("http://127.0.0.1:8081")
    );
    assert_eq!(e.listen_on("a", "sequencer"), "10.0.0.2");
    assert_eq!(e.listen_on("b", "validator-3"), "10.0.0.3");
    assert_eq!(e.listen_on("a", "validator-1"), "127.0.0.1");
    // Unplaced, the sequencer has no caller elsewhere: loopback.
    let m = Manifest::parse(&file(&hosts), "testnet").unwrap();
    assert_eq!(m.env.listen_on("a", "sequencer"), "127.0.0.1");

    // Two local hosts reach each other at loopback.
    let local = placed
        .replace("network = \"testnet\"", "network = \"local\"")
        .replace("token = \"circle-usdc\"", "token = { local = \"USDC\" }")
        .replace(
            "provider = \"ssh\"\naddress = \"ops@a.example\"",
            "provider = \"local\"",
        )
        .replace(
            "provider = \"ssh\"\naddress = \"ops@b.example\"",
            "provider = \"local\"",
        )
        .replace("private_address = \"10.0.0.2\"\n", "")
        .replace("private_address = \"10.0.0.3\"\n", "")
        .replace("public_url = \"https://lane.example\"\n", "");
    let m = Manifest::parse(&file(&local), "testnet").unwrap();
    assert_eq!(
        m.env.url_of("b", "sequencer").as_deref(),
        Some("http://127.0.0.1:8080")
    );
    assert_eq!(m.env.listen_on("b", "validator-3"), "127.0.0.1");

    // A host one validator's peer can't reach, one undeclared, one named
    // like a resource, and `host` with `hosts`.
    let e = err(&placed.replace("private_address = \"10.0.0.3\"\n", ""));
    assert!(
        e.contains("validator \"3\" on host \"b\"")
            && e.contains("give [hosts.b] a private_address or a public_url"),
        "{e}"
    );
    let e = err(&placed.replace("host = \"b\"", "host = \"c\""));
    assert!(
        e.contains("host = \"c\", but the deployment has [hosts.a], [hosts.b]"),
        "{e}"
    );
    let e = err(&ENV.replace(
        "name = \"3\"\nkey = \"demo-v3\"",
        "name = \"3\"\nkey = \"demo-v3\"\nhost = \"b\"",
    ));
    assert!(
        e.contains("one host (declare [hosts.<name>] for several)"),
        "{e}"
    );
    let e = err(&hosts.replace("hosts.b]", "hosts.data]"));
    assert!(e.contains("not data, release or file"), "{e}");
    let e = err(&hosts.replace(
        "private_address = \"10.0.0.3\"",
        "private_address = \"b.internal\"",
    ));
    assert!(e.contains("an IP address"), "{e}");
    let e = err(&format!(
        "{hosts}\n[env.testnet.host]\nprovider = \"local\"\n"
    ));
    assert!(e.contains("not both"), "{e}");
    let e = err(&hosts.replace("host = \"a\"\n", ""));
    assert!(e.contains("say which runs the sequencer"), "{e}");
    let e = err(&hosts.replace("host = \"a\"", "host = \"z\""));
    assert!(e.contains("no [hosts.z]"), "{e}");
}

/// Across hosts without a private network, and validators run elsewhere
/// (C-23): reached through public URLs, and only for requests the
/// sequencer signed.
#[test]
fn public_urls_and_validators_run_elsewhere() {
    let hosts = ENV.replace(
        "[env.testnet.host]\nprovider = \"ssh\"\naddress = \"ops@lane.example\"\npublic_url = \"https://lane.example\"\n",
        "[env.testnet.sequencer]\nkey = \"demo-seq\"\nhost = \"a\"\n\n[env.testnet.hosts.a]\nprovider = \"ssh\"\naddress = \"ops@a.example\"\npublic_url = \"https://lane.example\"\n\n[env.testnet.hosts.b]\nprovider = \"ssh\"\naddress = \"ops@b.example\"\npublic_url = \"https://b.example/\"\n",
    )
    .replace(
        "name = \"3\"\nkey = \"demo-v3\"",
        "name = \"3\"\nkey = \"demo-v3\"\nhost = \"b\"",
    );
    let m = Manifest::parse(&file(&hosts), "testnet").unwrap();
    let e = &m.env;
    assert_eq!(
        e.url_of("b", "sequencer").as_deref(),
        Some("https://lane.example")
    );
    assert_eq!(
        e.url_of("a", "validator-3").as_deref(),
        Some("https://b.example/validators/3")
    );
    // The proxy on b passes validator 3's /v1/sign; nothing listens off
    // loopback.
    assert!(e.sign_via_public("validator-3"));
    assert!(!e.sign_via_public("validator-1"));
    assert_eq!(e.listen_on("b", "validator-3"), "127.0.0.1");
    assert_eq!(e.listen_on("a", "sequencer"), "127.0.0.1");
    // Without the sequencer's key, a validator elsewhere is refused.
    let e = err(&hosts.replace("key = \"demo-seq\"\n", ""));
    assert!(e.contains("give [sequencer] key = \"<identity>\""), "{e}");
    let g = stellar_strkey::ed25519::PublicKey([1; 32]).to_string();
    let e = err(&hosts.replace("key = \"demo-seq\"", &format!("key = \"{}\"", g.as_str())));
    assert!(e.contains("sequencer.key"), "{e}");

    // A validator someone else runs: in the set, never deployed.
    let partner = "\n[[env.testnet.validators]]\nname = \"partner\"\nkey = \"partner-v\"\nurl = \"https://v.partner.example/\"\n";
    let with_key = ENV.replace(
        "[env.testnet.relayer]",
        "[env.testnet.sequencer]\nkey = \"demo-seq\"\n\n[env.testnet.relayer]",
    );
    let m = Manifest::parse(&file(&format!("{with_key}{partner}")), "testnet").unwrap();
    assert_eq!(m.env.validators.len(), 4);
    let run: Vec<_> = m
        .env
        .run_validators()
        .map(|(_, v)| v.name.clone())
        .collect();
    assert_eq!(run, ["1", "2", "3"]);
    assert_eq!(
        m.env.url_of("default", "validator-partner").as_deref(),
        Some("https://v.partner.example")
    );
    let e = err(&format!("{ENV}{partner}"));
    assert!(e.contains("give [sequencer] key"), "{e}");
    let e = err(&format!("{with_key}{partner}host = \"b\"\n"));
    assert!(e.contains("run elsewhere (url), so no host or port"), "{e}");
    let e = err(&format!(
        "{with_key}{}",
        partner.replace("https://v.partner.example/", "v.partner.example")
    ));
    assert!(e.contains("an http(s) URL"), "{e}");
}

/// Namespaces (C-24): several lanes on one ssh host.
#[test]
fn namespaces() {
    let ns = ENV.replace(
        "public_url = \"https://lane.example\"\n",
        "public_url = \"https://lane.example\"\nnamespace = \"pay\"\n",
    );
    let m = Manifest::parse(&file(&ns), "testnet").unwrap();
    let h = &m.env.host;
    // Its own root, units and Caddy snippet, unless the root is given.
    assert_eq!(h.root, "/opt/caravel-pay");
    assert_eq!(h.unit_prefix(), "caravel-pay-");
    assert_eq!(h.caddy_file(), "caddy/caravel.d/pay.caddy");
    let own = ns.replace(
        "namespace = \"pay\"\n",
        "namespace = \"pay\"\nroot = \"/srv/pay\"\n",
    );
    assert_eq!(
        Manifest::parse(&file(&own), "testnet")
            .unwrap()
            .env
            .host
            .root,
        "/srv/pay"
    );
    // Without one, the names of a host with one lane.
    let m = Manifest::parse(&file(ENV), "testnet").unwrap();
    assert_eq!(
        (
            m.env.host.root.as_str(),
            m.env.host.unit_prefix(),
            m.env.host.caddy_file()
        ),
        (
            "/opt/caravel",
            "caravel-".to_string(),
            "caddy/Caddyfile".to_string()
        )
    );
    for bad in ["Pay", "pay_1", "-pay", ""] {
        let e = err(&ns.replace("namespace = \"pay\"", &format!("namespace = \"{bad}\"")));
        assert!(
            e.contains("lowercase letters, digits and '-'"),
            "{bad}: {e}"
        );
    }
    let local = ENV
        .replace("network = \"testnet\"", "network = \"local\"")
        .replace("token = \"circle-usdc\"", "token = { local = \"USDC\" }")
        .replace(
            "provider = \"ssh\"\naddress = \"ops@lane.example\"\npublic_url = \"https://lane.example\"\n",
            "provider = \"local\"\nnamespace = \"pay\"\n",
        )
        .replace("[env.testnet", "[env.local")
        .replace("[[env.testnet", "[[env.local");
    let e = format!("{:#}", Manifest::parse(&file(&local), "local").unwrap_err());
    assert!(e.contains("keeps each lane apart already"), "{e}");
}

/// The web app's config (C-25): values known once keys and addresses are,
/// filled in like the relayer's feeds.
#[test]
fn the_web_apps_config() {
    use caravel_deploy::attrs::attributes;
    use caravel_deploy::deploy::{addresses, Keys};

    let lane = format!(
        "{LANE}{ENV}
[env.testnet.web.config]
sequencerUrl = \"${{node.sequencer.url}}\"
settlementContract = \"${{contract.settlement.address}}\"
networkName = \"${{network.name}}\"
validatorUrls = [\"/validators/1\", \"/validators/2\"]
"
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lane.toml");
    std::fs::write(&path, &lane).unwrap();
    let mut m = Manifest::load(&path, "testnet").unwrap();
    assert_eq!(m.env.web_host().as_deref(), Some("default"));
    let keys = Keys {
        admin: [0xA0; 32],
        relayer: [0xA1; 32],
        validators: vec![[1; 32], [2; 32], [3; 32]],
        accounts: Default::default(),
        sequencer: None,
    };
    let a = addresses(&m, &keys.admin).unwrap();
    let attrs = attributes(&m, &keys, &a, None, None);
    m.finish(&attrs).unwrap();
    let c = &m.env.web.as_ref().unwrap().config;
    assert_eq!(c["sequencerUrl"].as_str(), Some("https://lane.example"));
    assert_eq!(
        c["settlementContract"].as_str(),
        Some(stellar_strkey::Contract(a.settlement).to_string().as_str())
    );
    assert_eq!(c["networkName"].as_str(), Some("testnet"));
    assert_eq!(c["validatorUrls"].as_array().map(Vec::len), Some(2));

    // Served at a public URL, from a declared host.
    let e = err(&format!("{ENV}\n[env.testnet.web]\nhost = \"b\"\n"));
    assert!(
        e.contains("web.host = \"b\": not one of the deployment's hosts"),
        "{e}"
    );
    let e = err(&format!(
        "{}\n[env.testnet.web]\n",
        ENV.replace("public_url = \"https://lane.example\"\n", "")
    ));
    assert!(
        e.contains("has no public_url to serve the web app at"),
        "{e}"
    );
}
