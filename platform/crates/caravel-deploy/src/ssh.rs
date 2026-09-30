//! The `ssh` provider: a Linux host the team already has (spec §20.3), set up
//! once with `lanes/perps/deploy/testnet/provision.sh` (Ubuntu with systemd,
//! Caddy, Node 22, a `caravel` user, `<root>` with `keys/` and `data/` at
//! mode 700). The lane's nodes are systemd units, as on lane #1's VM
//! (DEC-046). The tool checks the host and installs nothing on it besides the
//! lane.
//!
//! Every command runs over ssh, or `gcloud compute ssh --tunnel-through-iap`.
//! Secrets travel on the connection's stdin, straight into files of mode 600,
//! and are never written on this machine.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};

use crate::manifest::{HostSpec, Manifest, Transport};
use crate::plan::{Host, Key, NodeReport, NodeState};
use crate::release::Release;
use crate::render::{validator_node, validator_port};

pub struct Ssh {
    pub root: String,
    spec: HostSpec,
    template: String,
    /// Where this machine keeps the lane's exported proofs.
    pub local_dir: PathBuf,
}

fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn hex(k: &[u8]) -> String {
    k.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_key(s: &str) -> Option<Key> {
    let v: Vec<u8> = (0..32)
        .map(|i| u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok())
        .collect::<Option<_>>()?;
    v.try_into().ok()
}

/// The systemd unit of a node.
pub fn unit_of(node: &str) -> String {
    match node.strip_prefix("validator-") {
        Some(name) => format!("caravel-validator@{name}"),
        None => format!("caravel-{node}"),
    }
}

impl Ssh {
    pub fn new(m: &Manifest, template: &str) -> Result<Self> {
        let local_dir = std::env::current_dir()?
            .join(".caravel")
            .join(&m.lane.lane.name)
            .join(&m.env_name);
        Ok(Self {
            root: m.env.host.root.clone(),
            spec: m.env.host.clone(),
            template: template.to_string(),
            local_dir,
        })
    }

    fn address(&self) -> &str {
        self.spec.address.as_deref().unwrap_or_default()
    }

    fn remote(&self, script: &str) -> Command {
        match self.spec.transport {
            Transport::Ssh => {
                let mut c = Command::new("ssh");
                c.args([
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "ConnectTimeout=20",
                    self.address(),
                    "bash",
                    "-c",
                ])
                .arg(q(script));
                c
            }
            Transport::GcloudIap => {
                let mut c = Command::new("gcloud");
                c.args(["compute", "ssh", self.address()])
                    .args(["--zone", self.spec.zone.as_deref().unwrap_or_default()])
                    .args([
                        "--project",
                        self.spec.project.as_deref().unwrap_or_default(),
                    ])
                    .args(["--tunnel-through-iap", "--quiet", "--command"])
                    .arg(format!("bash -c {}", q(script)));
                c
            }
        }
    }

    /// Runs `script` on the host with `stdin`; retries a dropped connection
    /// (exit 255) twice, since every step is safe to repeat.
    pub fn exec(&self, script: &str, stdin: &[u8]) -> Result<String> {
        let mut last = String::new();
        for _ in 0..3 {
            let mut child = self
                .remote(script)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .context("running ssh")?;
            child.stdin.take().expect("piped").write_all(stdin)?;
            let out = child.wait_with_output()?;
            if out.status.success() {
                return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
            }
            last = String::from_utf8_lossy(&out.stderr)
                .lines()
                .filter(|l| {
                    !l.contains("NumPy")
                        && !l.contains("increasing_the_tcp")
                        && !l.trim().is_empty()
                        && !l.starts_with("WARNING")
                })
                .collect::<Vec<_>>()
                .join("\n");
            if out.status.code() != Some(255) {
                bail!("on {}: {last}", self.address());
            }
            std::thread::sleep(Duration::from_secs(3));
        }
        bail!("could not reach {}: {last}", self.address())
    }

    fn upload(&self, local: &Path, remote: &str) -> Result<()> {
        let dest = format!("{}:{remote}", self.address());
        let mut c = match self.spec.transport {
            Transport::Ssh => {
                let mut c = Command::new("scp");
                c.args(["-q", "-o", "BatchMode=yes"]).arg(local).arg(&dest);
                c
            }
            Transport::GcloudIap => {
                let mut c = Command::new("gcloud");
                c.args(["compute", "scp"])
                    .args(["--zone", self.spec.zone.as_deref().unwrap_or_default()])
                    .args([
                        "--project",
                        self.spec.project.as_deref().unwrap_or_default(),
                    ])
                    .args(["--tunnel-through-iap", "--quiet"])
                    .arg(local)
                    .arg(&dest);
                c
            }
        };
        for attempt in 0..3 {
            if c.stderr(Stdio::null()).status()?.success() {
                return Ok(());
            }
            if attempt < 2 {
                std::thread::sleep(Duration::from_secs(3));
            }
        }
        bail!("uploading to {dest} failed")
    }

    /// What the host has, from one script.
    pub async fn read(&self, m: &Manifest) -> Result<Host> {
        let r = &self.root;
        let mut nodes = vec!["sequencer".to_string(), "relayer".to_string()];
        let mut ports = vec![("sequencer".to_string(), m.env.sequencer.port)];
        for (i, v) in m.env.validators.iter().enumerate() {
            nodes.push(validator_node(&v.name));
            ports.push((validator_node(&v.name), validator_port(m, i)));
        }
        let mut script = format!(
            "R={r}\n\
             command -v systemctl >/dev/null || echo 'MISSING systemd'\n\
             sudo -n true 2>/dev/null || echo 'MISSING passwordless sudo'\n\
             id caravel >/dev/null 2>&1 || echo 'MISSING a caravel user'\n\
             [ -d $R ] || echo \"MISSING $R\"\n\
             command -v rsync >/dev/null || echo 'MISSING rsync'\n\
             command -v curl >/dev/null || echo 'MISSING curl'\n\
             v=$(node --version 2>/dev/null | sed 's/^v//; s/\\..*//'); [ \"${{v:-0}}\" -ge 22 ] || echo 'MISSING Node 22 or newer'\n",
            r = q(r)
        );
        if m.env.host.public_url.is_some() {
            script += "command -v caddy >/dev/null || echo 'MISSING caddy'\n";
        }
        script += "echo \"COMMIT $(sudo cat $R/COMMIT 2>/dev/null | cut -c1-12)\"\n\
            for f in $R/config/*; do [ -f \"$f\" ] && echo \"FILE $(basename $f) $(sudo sha256sum $f | cut -c1-64)\"; done\n\
            for f in /etc/systemd/system/caravel-*.service; do [ -f \"$f\" ] && echo \"FILE systemd/$(basename $f) $(sha256sum $f | cut -c1-64)\"; done\n\
            [ -f /etc/caddy/Caddyfile ] && echo \"FILE caddy/Caddyfile $(sha256sum /etc/caddy/Caddyfile | cut -c1-64)\"\n\
            for f in $R/run/*.started; do [ -f \"$f\" ] && echo \"STARTED $(basename $f .started) $(cat $f)\"; done\n\
            sudo sh -c \"ls $R/data/*.sqlite\" >/dev/null 2>&1 && echo DATA\n";
        for n in &nodes {
            script += &format!(
                "echo \"UNIT {n} $(systemctl is-active {} 2>/dev/null)\"\n",
                unit_of(n)
            );
        }
        for n in systemd_validators_script() {
            script += n;
        }
        for (n, p) in &ports {
            script += &format!("echo \"STATUS {n} $(curl -sf -m 3 http://127.0.0.1:{p}/v1/status | tr -d '\\n')\"\n");
        }
        let out = self.exec(&script, b"")?;
        let mut host = Host::default();
        for line in out.lines() {
            let (tag, rest) = line.split_once(' ').unwrap_or((line, ""));
            match tag {
                "MISSING" => host.missing.push(rest.to_string()),
                "COMMIT" if !rest.trim().is_empty() => host.release = Some(rest.trim().to_string()),
                "FILE" => {
                    if let Some((name, h)) = rest.split_once(' ') {
                        if let Some(k) = hex_key(h) {
                            host.files.insert(name.to_string(), k);
                        }
                    }
                }
                "STARTED" => {
                    if let Some((node, h)) = rest.split_once(' ') {
                        host.nodes.entry(node.to_string()).or_default().started_with =
                            hex_key(h.trim());
                    }
                }
                "DATA" => host.has_data = true,
                "UNIT" => {
                    if let Some((node, state)) = rest.split_once(' ') {
                        host.nodes.entry(node.to_string()).or_default().running =
                            state.trim() == "active";
                    }
                }
                "RUNNING" => {
                    // A validator unit running that the lane file doesn't list.
                    let node = format!("validator-{}", rest.trim());
                    host.nodes.entry(node).or_default().running = true;
                }
                "STATUS" => {
                    if let Some((node, json)) = rest.split_once(' ') {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(json) {
                            host.nodes.entry(node.to_string()).or_default().report =
                                crate::local::node_report(&v);
                        }
                    }
                }
                _ => {}
            }
        }
        // A node with a fingerprint or status but no unit line isn't running.
        host.nodes.retain(|_, s: &mut NodeState| {
            s.running || s.started_with.is_some() || s.report.is_some()
        });
        Ok(host)
    }

    pub fn install_release(&self, r: &Release) -> Result<()> {
        let stage = tempdir()?;
        let s = stage.as_path();
        std::fs::create_dir_all(s.join("bin"))?;
        std::fs::copy(
            &r.node_binary,
            s.join("bin")
                .join(crate::release::node_binary(&self.template)),
        )?;
        copy_dir(&r.contracts, &s.join("contracts"))?;
        copy_dir(&r.relayer, &s.join("relayer"))?;
        if let Some(f) = &r.feeds {
            copy_dir(f, &s.join("relayer-feeds").join(&self.template))?;
        }
        if let Some(w) = &r.web {
            copy_dir(w, &s.join("web"))?;
        }
        std::fs::write(s.join("COMMIT"), format!("{}\n", r.commit))?;
        let tgz = s.with_extension("tgz");
        let ok = Command::new("tar")
            .env("COPYFILE_DISABLE", "1")
            .args(["--no-xattrs", "-czf"])
            .arg(&tgz)
            .arg("-C")
            .arg(s)
            .arg(".")
            .status()?
            .success();
        if !ok {
            bail!("packing the release");
        }
        let remote = format!("/tmp/caravel-release-{}.tgz", r.commit);
        self.upload(&tgz, &remote)?;
        let _ = std::fs::remove_file(&tgz);
        let _ = std::fs::remove_dir_all(s);
        self.exec(
            &format!(
                "set -e; R={r}; S=$(mktemp -d); tar -xzf {remote} -C $S; rm -f {remote}\n\
                 for d in bin contracts relayer relayer-feeds web; do [ -d $S/$d ] && sudo mkdir -p $R/$d && sudo rsync -a --delete $S/$d/ $R/$d/; done\n\
                 sudo cp $S/COMMIT $R/COMMIT\n\
                 sudo chown -R caravel:caravel $R/bin $R/contracts $R/relayer $R/COMMIT\n\
                 [ -d $R/relayer-feeds ] && sudo chown -R caravel:caravel $R/relayer-feeds\n\
                 [ -d $R/web ] && sudo chown -R caravel:caravel $R/web\n\
                 sudo chmod 755 $R/bin/*; rm -rf $S",
                r = q(&self.root),
                remote = q(&remote)
            ),
            b"",
        )
        .map(|_| ())
    }

    /// Writes a rendered file where it belongs: a node config under
    /// `<root>/config/`, a unit under `/etc/systemd/system/`, or the Caddyfile.
    pub fn write_file(&self, name: &str, text: &str) -> Result<()> {
        let script = if let Some(unit) = name.strip_prefix("systemd/") {
            format!("sudo install -m 644 /dev/stdin /etc/systemd/system/{} && sudo systemctl daemon-reload", q(unit))
        } else if name == "caddy/Caddyfile" {
            "sudo install -m 644 /dev/stdin /etc/caddy/Caddyfile && (sudo systemctl reload caddy || sudo systemctl restart caddy)".to_string()
        } else {
            format!(
                "sudo install -d -o caravel -g caravel -m 755 {r}/config && sudo install -o caravel -g caravel -m 644 /dev/stdin {r}/config/{}",
                q(name),
                r = q(&self.root)
            )
        };
        self.exec(&script, text.as_bytes()).map(|_| ())
    }

    /// Streams the validators' keys and the environment file over stdin into
    /// mode-600 files; the internal token is made on the host once and kept.
    pub fn write_keys(
        &self,
        validators: &[(String, String)],
        env: &[(String, String)],
    ) -> Result<()> {
        let r = q(&self.root);
        self.exec(
            &format!("sudo install -d -o caravel -g caravel -m 700 {r}/keys {r}/data {r}/run"),
            b"",
        )?;
        for (node, secret) in validators {
            self.exec(
                &format!(
                    "sudo install -o caravel -g caravel -m 600 /dev/stdin {r}/keys/{}",
                    q(&format!("{node}.key"))
                ),
                format!("{secret}\n").as_bytes(),
            )?;
        }
        let body: String = env.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
        self.exec(
            &format!(
                "set -e; T=$(sudo sed -n 's/^CARAVEL_INTERNAL_TOKEN=//p' {r}/keys/env 2>/dev/null || true)\n\
                 [ -n \"$T\" ] || T=$(openssl rand -hex 24)\n\
                 {{ echo \"CARAVEL_INTERNAL_TOKEN=$T\"; cat; }} | sudo install -o caravel -g caravel -m 600 /dev/stdin {r}/keys/env"
            ),
            body.as_bytes(),
        )
        .map(|_| ())
    }

    /// Starts (or restarts) a node's unit and records the configs it read.
    pub fn start(&self, node: &str, fingerprint: &Key) -> Result<()> {
        let unit = q(&unit_of(node));
        self.exec(
            &format!(
                "set -e; sudo systemctl enable {unit} >/dev/null 2>&1; sudo systemctl restart {unit}; echo {} | sudo install -o caravel -g caravel -m 644 /dev/stdin {r}/run/{}",
                hex(fingerprint),
                q(&format!("{node}.started")),
                r = q(&self.root)
            ),
            b"",
        )
        .map(|_| ())
    }

    pub fn stop(&self, node: &str) -> Result<()> {
        self.exec(
            &format!(
                "sudo systemctl disable --now {} >/dev/null 2>&1 || true; sudo rm -f {r}/run/{}",
                q(&unit_of(node)),
                q(&format!("{node}.started")),
                r = q(&self.root)
            ),
            b"",
        )
        .map(|_| ())
    }

    pub fn wipe(&self, nodes: &[String]) -> Result<()> {
        for n in nodes {
            self.stop(n)?;
        }
        self.exec(&format!("sudo rm -rf {r}/data/*", r = self.root), b"")
            .map(|_| ())
    }

    /// `export-proofs` on the host; the JSON comes back to `out` here.
    pub fn export_proofs(&self, node: &str, out: &Path) -> Result<()> {
        let bin = format!(
            "{}/bin/{}",
            self.root,
            crate::release::node_binary(&self.template)
        );
        let json = self.exec(
            &format!(
                "sudo -u caravel {} export-proofs --config {}",
                q(&bin),
                q(&format!("{}/config/{node}.toml", self.root))
            ),
            b"",
        )?;
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(out, json)?;
        Ok(())
    }

    pub async fn status(&self, port: u16) -> Option<serde_json::Value> {
        let out = self
            .exec(
                &format!("curl -sf -m 3 http://127.0.0.1:{port}/v1/status"),
                b"",
            )
            .ok()?;
        serde_json::from_str(&out).ok()
    }

    pub fn log_tail(&self, node: &str) -> String {
        self.exec(
            &format!("sudo journalctl -u {} -n 8 --no-pager", q(&unit_of(node))),
            b"",
        )
        .unwrap_or_default()
    }

    /// Waits until the node on `port` reports `want`.
    pub async fn wait_healthy(
        &self,
        port: u16,
        want: &NodeReport,
        timeout: Duration,
    ) -> Result<()> {
        let deadline = Instant::now() + timeout;
        let mut last = "no answer".to_string();
        while Instant::now() < deadline {
            if let Some(v) = self.status(port).await {
                match crate::local::node_report(&v) {
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
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        Err(anyhow!("the node on port {port} is not healthy: {last}"))
    }
}

/// Lists running validator units the lane file may not name (`RUNNING <name>`).
fn systemd_validators_script() -> [&'static str; 1] {
    ["systemctl list-units --type=service --state=active --no-legend 'caravel-validator@*' 2>/dev/null | sed -n 's/^ *caravel-validator@\\([^.]*\\)\\.service.*/RUNNING \\1/p'\n"]
}

fn tempdir() -> Result<PathBuf> {
    let d = std::env::temp_dir().join(format!("caravel-release-{}", std::process::id()));
    if d.exists() {
        std::fs::remove_dir_all(&d)?;
    }
    std::fs::create_dir_all(&d)?;
    Ok(d)
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
