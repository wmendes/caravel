//! The host a deployment runs on: this machine (`local`) or a Linux host over
//! ssh (`ssh`), its nodes as processes, systemd units or containers
//! (`runtime = "docker"`, D-02). All are read back and changed with the same
//! operations.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;

use crate::docker::Docker;
use crate::local::Local;
use crate::manifest::Manifest;
use crate::plan::{Host, Key, NodeReport};
use crate::release::Release;
use crate::ssh::Ssh;

pub enum HostProvider {
    Local(Local),
    Ssh(Box<Ssh>),
    Docker(Box<Docker>),
}

impl HostProvider {
    /// Host `host` of the deployment, by its provider (C-22).
    pub fn named(
        m: &Manifest,
        template: &str,
        state_root: &std::path::Path,
        host: &str,
    ) -> Result<Self> {
        let spec = m.env.host_spec(host);
        if spec.runtime() == crate::manifest::Runtime::Docker {
            return Ok(Self::Docker(Box::new(Docker::named(
                m, template, state_root, host,
            )?)));
        }
        Ok(match spec.provider {
            crate::manifest::Provider::Local => {
                Self::Local(Local::named(m, template, state_root, host)?)
            }
            crate::manifest::Provider::Ssh => {
                Self::Ssh(Box::new(Ssh::named(m, template, state_root, host)?))
            }
        })
    }

    /// The lane's root on the host.
    pub fn root_str(&self) -> String {
        match self {
            Self::Local(l) => l.root_str(),
            Self::Ssh(s) => s.root.clone(),
            Self::Docker(d) => d.root.clone(),
        }
    }

    /// Where the exported exits land on this machine.
    pub fn exit_path(&self) -> PathBuf {
        match self {
            Self::Local(l) => l.root.join("exit.json"),
            Self::Ssh(s) => s.local_dir.join("exit.json"),
            Self::Docker(d) => d.exit_path(),
        }
    }

    pub async fn read(&self, m: &Manifest) -> Result<Host> {
        match self {
            Self::Local(l) => l.read(m).await,
            Self::Ssh(s) => s.read(m).await,
            Self::Docker(d) => d.read(m).await,
        }
    }

    pub fn install_release(&self, r: &Release) -> Result<()> {
        match self {
            Self::Local(l) => l.install_release(r),
            Self::Ssh(s) => s.install_release(r),
            Self::Docker(d) => d.install_release(r),
        }
    }

    pub fn write_file(&self, name: &str, text: &str) -> Result<()> {
        match self {
            Self::Local(l) => l.write_file(name, text),
            Self::Ssh(s) => s.write_file(name, text),
            Self::Docker(d) => d.write_file(name, text),
        }
    }

    pub fn write_keys(
        &self,
        validators: &[(String, String)],
        env: &[(String, String)],
    ) -> Result<()> {
        match self {
            Self::Local(l) => l.write_keys(validators, env),
            Self::Ssh(s) => s.write_keys(validators, env),
            Self::Docker(d) => d.write_keys(validators, env),
        }
    }

    /// Starts `node`, recording the fingerprint of the configs it reads.
    pub fn start(&self, node: &str, fingerprint: &Key) -> Result<()> {
        match self {
            Self::Local(l) => l.start(node, fingerprint),
            Self::Ssh(s) => s.start(node, fingerprint),
            Self::Docker(d) => d.start(node, fingerprint),
        }
    }

    pub fn stop(&self, node: &str) -> Result<()> {
        match self {
            Self::Local(l) => l.stop(node),
            Self::Ssh(s) => s.stop(node),
            Self::Docker(d) => d.stop(node),
        }
    }

    pub fn wipe(&self, nodes: &[String]) -> Result<()> {
        match self {
            Self::Local(l) => l.wipe(),
            Self::Ssh(s) => s.wipe(nodes),
            Self::Docker(d) => d.wipe(nodes),
        }
    }

    pub fn export_proofs(&self, node: &str, out: &std::path::Path) -> Result<()> {
        match self {
            Self::Local(l) => l.export_proofs(node, out),
            Self::Ssh(s) => s.export_proofs(node, out),
            Self::Docker(d) => d.export_proofs(node, out),
        }
    }

    pub fn read_file(&self, name: &str) -> Result<Option<String>> {
        match self {
            Self::Local(l) => l.read_file(name),
            Self::Ssh(s) => s.read_file(name),
            Self::Docker(d) => d.read_file(name),
        }
    }

    /// A node's `/v1/status`, where it listens on the host (`addr`).
    pub async fn status(&self, addr: &str, port: u16) -> Option<serde_json::Value> {
        match self {
            Self::Local(l) => l.status(port).await,
            Self::Ssh(s) => s.status(addr, port).await,
            Self::Docker(d) => d.status(port).await,
        }
    }

    /// A node's log: the last `lines`, then with `follow` what it writes.
    pub fn logs(&self, node: &str, lines: usize, follow: bool) -> Result<()> {
        match self {
            Self::Local(l) => l.logs(node, lines, follow),
            Self::Ssh(s) => s.logs(node, lines, follow),
            Self::Docker(d) => d.logs(node, lines, follow),
        }
    }

    pub fn log_tail(&self, node: &str) -> String {
        match self {
            Self::Local(l) => l.log_tail(node),
            Self::Ssh(s) => s.log_tail(node),
            Self::Docker(d) => d.log_tail(node),
        }
    }

    pub async fn wait_healthy(
        &self,
        addr: &str,
        port: u16,
        want: &NodeReport,
        timeout: Duration,
    ) -> Result<()> {
        match self {
            Self::Local(_) => crate::local::wait_healthy(port, want, timeout).await,
            Self::Ssh(s) => s.wait_healthy(addr, port, want, timeout).await,
            Self::Docker(d) => d.wait_healthy(port, want, timeout).await,
        }
    }
}
