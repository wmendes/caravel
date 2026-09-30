//! The `local` provider: the lane's nodes as processes on this machine, under
//! `.caravel/<lane>/<env>/` (the layout of `render.rs`), each with a PID file
//! and a log. It reads the host back from those files and from the nodes'
//! `/v1/status`, so re-running `apply` sees what is already there.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use caravel_runtime::checkpoint::sha256;

use crate::manifest::Manifest;
use crate::plan::{Host, Key, NodeReport, NodeState};
use crate::release::Release;
use crate::render::{validator_node, validator_port};

pub struct Local {
    pub root: PathBuf,
    template: String,
}

fn hex_to_key(v: &serde_json::Value) -> Option<Key> {
    let h = v.as_str()?;
    let bytes: Vec<u8> = (0..32)
        .map(|i| u8::from_str_radix(h.get(2 * i..2 * i + 2)?, 16).ok())
        .collect::<Option<_>>()?;
    bytes.try_into().ok()
}

fn write_private(path: &Path, text: &str) -> Result<()> {
    std::fs::write(path, text)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let e = e?;
        let dest = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &dest)?;
        } else {
            std::fs::copy(e.path(), &dest)?;
        }
    }
    Ok(())
}

fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

impl Local {
    /// `.caravel/<lane>/<env>` under the current directory, absolute.
    pub fn new(m: &Manifest, template: &str) -> Result<Self> {
        let root = std::env::current_dir()?
            .join(".caravel")
            .join(&m.lane.lane.name)
            .join(&m.env_name);
        Ok(Self {
            root,
            template: template.to_string(),
        })
    }

    pub fn root_str(&self) -> String {
        self.root.display().to_string()
    }

    fn pid_file(&self, node: &str) -> PathBuf {
        self.root.join("run").join(format!("{node}.pid"))
    }

    fn pid(&self, node: &str) -> Option<u32> {
        std::fs::read_to_string(self.pid_file(node))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    fn started_file(&self, node: &str) -> PathBuf {
        self.root.join("run").join(format!("{node}.started"))
    }

    /// The hashes of the config files as they are on disk now.
    fn config_hashes(&self) -> BTreeMap<String, Key> {
        let mut files = BTreeMap::new();
        if let Ok(dir) = std::fs::read_dir(self.root.join("config")) {
            for e in dir.flatten() {
                if let Ok(bytes) = std::fs::read(e.path()) {
                    files.insert(e.file_name().to_string_lossy().into_owned(), sha256(&bytes));
                }
            }
        }
        files
    }

    /// Every node's port: the sequencer's and each validator's.
    fn ports(m: &Manifest) -> BTreeMap<String, u16> {
        let mut p = BTreeMap::from([("sequencer".to_string(), m.env.sequencer.port)]);
        for (i, v) in m.env.validators.iter().enumerate() {
            p.insert(validator_node(&v.name), validator_port(m, i));
        }
        p
    }

    /// What the host has now.
    pub async fn read(&self, m: &Manifest) -> Result<Host> {
        let mut host = Host {
            missing: prerequisites(),
            release: std::fs::read_to_string(self.root.join("COMMIT"))
                .ok()
                .map(|s| s.trim().to_string()),
            ..Host::default()
        };
        host.files = self.config_hashes();
        host.has_data = std::fs::read_dir(self.root.join("data"))
            .map(|d| {
                d.flatten()
                    .any(|e| e.path().extension().is_some_and(|x| x == "sqlite"))
            })
            .unwrap_or(false);
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()?;
        let ports = Self::ports(m);
        let mut names: Vec<String> = ports.keys().cloned().collect();
        names.push("relayer".into());
        if let Ok(dir) = std::fs::read_dir(self.root.join("run")) {
            for e in dir.flatten() {
                if let Some(n) = e.file_name().to_string_lossy().strip_suffix(".pid") {
                    if !names.iter().any(|x| x == n) {
                        names.push(n.to_string());
                    }
                }
            }
        }
        for name in names {
            let running = self.pid(&name).is_some_and(alive);
            let mut report = None;
            if running {
                if let Some(port) = ports.get(&name) {
                    if let Ok(r) = http
                        .get(format!("http://127.0.0.1:{port}/v1/status"))
                        .send()
                        .await
                    {
                        if let Ok(v) = r.json::<serde_json::Value>().await {
                            report = node_report(&v);
                        }
                    }
                }
            }
            let started_with = std::fs::read_to_string(self.started_file(&name))
                .ok()
                .and_then(|t| hex_to_key(&serde_json::Value::String(t.trim().to_string())));
            host.nodes.insert(
                name,
                NodeState {
                    running,
                    report,
                    started_with,
                },
            );
        }
        Ok(host)
    }

    pub fn install_release(&self, r: &Release) -> Result<()> {
        for d in ["bin", "contracts", "relayer", "relayer-feeds"] {
            let p = self.root.join(d);
            if p.exists() {
                std::fs::remove_dir_all(&p)?;
            }
        }
        let bin = self.root.join("bin");
        std::fs::create_dir_all(&bin)?;
        let node_bin = bin.join(crate::release::node_binary(&self.template));
        std::fs::copy(&r.node_binary, &node_bin)
            .with_context(|| format!("copying {}", r.node_binary.display()))?;
        std::fs::set_permissions(&node_bin, std::fs::Permissions::from_mode(0o755))?;
        copy_dir(&r.contracts, &self.root.join("contracts"))?;
        copy_dir(&r.relayer, &self.root.join("relayer"))?;
        if let Some(f) = &r.feeds {
            copy_dir(f, &self.root.join("relayer-feeds").join(&self.template))?;
        }
        std::fs::write(self.root.join("COMMIT"), format!("{}\n", r.commit))?;
        Ok(())
    }

    pub fn write_file(&self, name: &str, text: &str) -> Result<()> {
        let dir = self.root.join("config");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(name), text)?;
        Ok(())
    }

    /// Validator keys and the sequencer and relayer environment, mode 600 in
    /// a mode-700 directory. The internal token is made once and kept.
    pub fn write_keys(
        &self,
        validators: &[(String, String)],
        env: &[(String, String)],
    ) -> Result<()> {
        let dir = self.root.join("keys");
        std::fs::create_dir_all(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        for (node, secret) in validators {
            write_private(&dir.join(format!("{node}.key")), &format!("{secret}\n"))?;
        }
        let token = match self.env_var("CARAVEL_INTERNAL_TOKEN") {
            Some(t) => t,
            None => {
                use std::io::Read;
                let mut b = [0u8; 24];
                std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
                b.iter().map(|x| format!("{x:02x}")).collect()
            }
        };
        let mut text = format!("CARAVEL_INTERNAL_TOKEN={token}\n");
        for (k, v) in env {
            text += &format!("{k}={v}\n");
        }
        write_private(&dir.join("env"), &text)
    }

    fn env_var(&self, name: &str) -> Option<String> {
        let text = std::fs::read_to_string(self.root.join("keys").join("env")).ok()?;
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{name}=")).map(str::to_string))
    }

    fn env_file(&self) -> Vec<(String, String)> {
        std::fs::read_to_string(self.root.join("keys").join("env"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                l.split_once('=')
                    .map(|(k, v)| (k.to_string(), v.to_string()))
            })
            .collect()
    }

    /// Starts `node` detached from this process, logging to `logs/<node>.log`.
    pub fn start(&self, node: &str, fingerprint: &Key) -> Result<()> {
        let config = self.root.join("config");
        let bin = self
            .root
            .join("bin")
            .join(crate::release::node_binary(&self.template));
        let mut cmd = match node {
            "sequencer" => {
                let mut c = Command::new(&bin);
                c.args(["sequencer", "--config"])
                    .arg(config.join("sequencer.toml"));
                c
            }
            "relayer" => {
                let mut c = Command::new("node");
                c.arg(self.root.join("relayer/dist/main.js"))
                    .arg("--config")
                    .arg(config.join("relayer.json"));
                c
            }
            v if v.starts_with("validator-") => {
                let mut c = Command::new(&bin);
                c.args(["validator", "--config"])
                    .arg(config.join(format!("{v}.toml")));
                c
            }
            other => bail!("unknown node {other}"),
        };
        if node == "sequencer" || node == "relayer" {
            cmd.envs(self.env_file());
        }
        std::fs::create_dir_all(self.root.join("logs"))?;
        std::fs::create_dir_all(self.root.join("run"))?;
        std::fs::create_dir_all(self.root.join("data"))?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("logs").join(format!("{node}.log")))?;
        let child = cmd
            .env("RUST_LOG", "info")
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .process_group(0)
            .spawn()
            .with_context(|| format!("starting {node}"))?;
        std::fs::write(self.pid_file(node), format!("{}\n", child.id()))?;
        let hex: String = fingerprint.iter().map(|b| format!("{b:02x}")).collect();
        std::fs::write(self.started_file(node), format!("{hex}\n"))?;
        Ok(())
    }

    /// Stops `node`: SIGINT, then SIGKILL after 15 s.
    pub fn stop(&self, node: &str) -> Result<()> {
        let Some(pid) = self.pid(node) else {
            return Ok(());
        };
        if alive(pid) {
            let _ = Command::new("kill")
                .args(["-INT", &pid.to_string()])
                .status();
            let deadline = Instant::now() + Duration::from_secs(15);
            while alive(pid) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(200));
            }
            if alive(pid) {
                let _ = Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .status();
            }
        }
        let _ = std::fs::remove_file(self.pid_file(node));
        let _ = std::fs::remove_file(self.started_file(node));
        Ok(())
    }

    /// Stops every node and removes the stores.
    pub fn wipe(&self) -> Result<()> {
        if let Ok(dir) = std::fs::read_dir(self.root.join("run")) {
            for e in dir.flatten() {
                if let Some(n) = e.file_name().to_string_lossy().strip_suffix(".pid") {
                    self.stop(n)?;
                }
            }
        }
        let data = self.root.join("data");
        if data.exists() {
            std::fs::remove_dir_all(&data)?;
        }
        Ok(())
    }

    /// `export-proofs` with a validator's config, into `out`.
    pub fn export_proofs(&self, node: &str, out: &Path) -> Result<()> {
        let cfg = self.root.join("config").join(format!("{node}.toml"));
        self.run_node(&[
            "export-proofs",
            "--config",
            &cfg.display().to_string(),
            "--out",
            &out.display().to_string(),
        ])
        .map(|_| ())
    }

    /// Runs the host's node binary with `args` and returns its stdout.
    pub fn run_node(&self, args: &[&str]) -> Result<String> {
        let bin = self
            .root
            .join("bin")
            .join(crate::release::node_binary(&self.template));
        let out = Command::new(&bin)
            .args(args)
            .output()
            .with_context(|| format!("running {}", bin.display()))?;
        if !out.status.success() {
            bail!(
                "{} {}: {}",
                bin.display(),
                args.first().unwrap_or(&""),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8(out.stdout)?)
    }

    /// A node's `/v1/status`, if it answers.
    pub async fn status(&self, port: u16) -> Option<serde_json::Value> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .ok()?;
        http.get(format!("http://127.0.0.1:{port}/v1/status"))
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()
    }

    pub fn log_tail(&self, node: &str) -> String {
        std::fs::read_to_string(self.root.join("logs").join(format!("{node}.log")))
            .map(|t| {
                t.lines()
                    .rev()
                    .take(8)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    }
}

/// A node's `/v1/status`, as the plan compares it.
pub fn node_report(v: &serde_json::Value) -> Option<NodeReport> {
    let settlement = stellar_strkey::Contract::from_string(v["settlement"].as_str()?)
        .ok()?
        .0;
    Some(NodeReport {
        lane_id: hex_to_key(&v["lane_id"])?,
        config_hash: hex_to_key(&v["config_hash"])?,
        settlement,
        engine_wasm_hash: hex_to_key(&v["engine_wasm_sha256"])?,
        key: v["key"]
            .as_str()
            .and_then(|k| stellar_strkey::ed25519::PublicKey::from_string(k).ok())
            .map(|k| k.0),
        epoch: v["signers"]["epoch"].as_str().and_then(|e| e.parse().ok()),
        release: v["release"]["commit"].as_str().map(str::to_string),
    })
}

/// What this machine lacks to run a lane: Node 22+ for the relayer.
fn prerequisites() -> Vec<String> {
    let node = Command::new("node").arg("--version").output();
    let major = node
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|v| {
            v.trim()
                .trim_start_matches('v')
                .split('.')
                .next()
                .and_then(|m| m.parse::<u32>().ok())
        });
    match major {
        Some(m) if m >= 22 => vec![],
        Some(m) => vec![format!("Node 22 or newer (found {m})")],
        None => vec!["Node 22 or newer (for the relayer)".to_string()],
    }
}

/// Waits until `node` answers `/v1/status` with `want`.
pub async fn wait_healthy(port: u16, want: &NodeReport, timeout: Duration) -> Result<()> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let deadline = Instant::now() + timeout;
    let mut last = String::from("no answer");
    while Instant::now() < deadline {
        if let Ok(r) = http
            .get(format!("http://127.0.0.1:{port}/v1/status"))
            .send()
            .await
        {
            if let Ok(v) = r.json::<serde_json::Value>().await {
                match node_report(&v) {
                    Some(r) if want.key.is_some() && r.key != want.key => {
                        last = "another validator answers on its port".into()
                    }
                    Some(r)
                        if r.lane_id == want.lane_id
                            && r.config_hash == want.config_hash
                            && r.settlement == want.settlement
                            && r.engine_wasm_hash == want.engine_wasm_hash =>
                    {
                        return Ok(())
                    }
                    Some(_) => last = "it runs another lane, config, settlement or engine".into(),
                    None => {
                        last = "its status lacks the identity fields (an older release?)".into()
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(anyhow!("the node on port {port} is not healthy: {last}"))
}
