//! `status` and `destroy` (spec §20.3).
//!
//! A settlement contract can't be deleted, and it freezes only when the lane
//! is stuck: no checkpoint for `escape_timeout_secs`, or an inbox message left
//! unprocessed for `force_inclusion_window_secs`. So `destroy` winds a lane
//! down rather than deleting it:
//! 1. drain: the last signed checkpoint is accepted, and a validator saw it;
//! 2. stop the relayer and the sequencer;
//! 3. export every exit (escape and withdrawal leaves, with proofs) to
//!    `exit.json`, checked against Stellar's last checkpoint;
//! 4. trigger: the admin asks for a 1-stroop forced withdrawal of its own
//!    lane account, which nobody processes now, so the contract allows a
//!    freeze after the force-inclusion window;
//! 5. freeze;
//! 6. the validators keep serving proofs unless asked to stop.
//!
//! Each step checks what is already done, so `destroy` can be run again.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::address::strkey;
use crate::deploy::Prepared;
use crate::render::{g, validator_node};

pub struct DestroyOptions {
    pub no_wait: bool,
    pub stop_validators: bool,
    pub pay_out: bool,
    pub wipe: bool,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Prepared {
    /// When the contract allows a freeze, from what the chain says now.
    pub fn freeze_possible_at(&self) -> Option<u64> {
        let p = &self.desired.params;
        let stall = self
            .extra
            .last_checkpoint
            .map(|(_, at)| at + p.escape_timeout_secs);
        let censor = self
            .extra
            .oldest_unprocessed_at
            .map(|at| at + p.force_inclusion_window_secs);
        match (stall, censor) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    pub async fn status_json(&self) -> Value {
        let d = &self.desired;
        let plan = self.plan();
        let mut nodes = serde_json::Map::new();
        let seq_port = self.m.env.sequencer.port;
        for (name, state) in &self.host.nodes {
            nodes.insert(
                name.clone(),
                json!({ "running": state.running, "answers": state.report.is_some() }),
            );
        }
        let seq = self.host_provider.status(seq_port).await;
        let oc = self.chain.settlement.as_ref();
        json!({
            "lane": d.lane_name,
            "env": d.env,
            "network": d.network.name(),
            "rpc_url": self.m.rpc_url(),
            "settlement": strkey(&d.settlement),
            "deployed": oc.is_some(),
            "frozen": oc.is_some_and(|o| o.frozen),
            "token": strkey(&d.token),
            "admin": g(&d.admin),
            "sequencer_url": crate::deploy::api_url(&self.m),
            "validators": self.m.env.validators.iter().enumerate().map(|(i, v)| json!({
                "node": validator_node(&v.name),
                "url": crate::deploy::validator_url(&self.m, i),
                "key": self.validator_keys.get(&validator_node(&v.name)).map(g),
            })).collect::<Vec<_>>(),
            "epoch": oc.map(|o| o.epoch),
            "height": seq.as_ref().and_then(|s| s["height"].as_str().map(str::to_string)),
            "last_checkpoint": self.extra.last_checkpoint.map(|(seq, at)| json!({ "seq": seq, "accepted_at": at, "age_secs": now().saturating_sub(at) })),
            "freeze_possible_at": self.freeze_possible_at(),
            "relayer_xlm": self.extra.relayer_balance.map(|b| format!("{}.{:07}", b / 10_000_000, b % 10_000_000)),
            "ttl": self.extra.ttl.iter().map(|(what, until)| json!({ "entry": what, "ledgers_left": until.saturating_sub(self.extra.latest_ledger) })).collect::<Vec<_>>(),
            "nodes": nodes,
            "exit_file": self.host_provider.exit_path().exists().then(|| self.host_provider.exit_path().display().to_string()),
            "plan": { "steps": plan.steps.len(), "problems": plan.problems.len() },
        })
    }

    pub async fn render_status(&self) -> String {
        let s = self.status_json().await;
        let mut o = format!(
            "Lane {} · env {} · network {}\n  settlement {}{}\n",
            s["lane"].as_str().unwrap_or(""),
            s["env"].as_str().unwrap_or(""),
            s["network"].as_str().unwrap_or(""),
            s["settlement"].as_str().unwrap_or(""),
            if s["frozen"] == true {
                " (frozen)"
            } else if s["deployed"] == true {
                ""
            } else {
                " (not deployed)"
            }
        );
        if let Some(h) = s["height"].as_str() {
            o += &format!("  height {h}, signer epoch {}\n", s["epoch"]);
        }
        match &s["last_checkpoint"] {
            Value::Null => o += "  no checkpoint accepted yet\n",
            c => {
                o += &format!(
                    "  last accepted checkpoint {}, {} s ago\n",
                    c["seq"], c["age_secs"]
                )
            }
        }
        if let Some(at) = s["freeze_possible_at"].as_u64() {
            let left = at.saturating_sub(now());
            o += &if left == 0 {
                "  anyone may freeze the lane now\n".to_string()
            } else {
                format!("  a freeze becomes possible in {left} s if nothing changes\n")
            };
        }
        if let Some(x) = s["relayer_xlm"].as_str() {
            o += &format!("  relayer {x} XLM\n");
        }
        for t in s["ttl"].as_array().into_iter().flatten() {
            o += &format!(
                "  {}: {} ledgers until archival\n",
                t["entry"].as_str().unwrap_or(""),
                t["ledgers_left"]
            );
        }
        let nodes: Vec<String> = s["nodes"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(n, v)| format!("{n} {}", if v["running"] == true { "up" } else { "down" }))
            .collect();
        o += &format!("  nodes: {}\n", nodes.join(", "));
        o += &format!(
            "  plan: {} step(s), {} problem(s){}\n",
            s["plan"]["steps"],
            s["plan"]["problems"],
            if s["plan"]["steps"] == 0 && s["plan"]["problems"] == 0 {
                " (matches the lane file)"
            } else {
                " (run plan)"
            }
        );
        o
    }

    /// Winds the lane down (see the module docs).
    pub async fn destroy(&self, o: &DestroyOptions) -> Result<()> {
        let d = &self.desired;
        let admin = self.m.env.admin.as_str();
        let Some(oc) = &self.chain.settlement else {
            eprintln!(
                "No settlement contract at {}: stopping the host's nodes.",
                strkey(&d.settlement)
            );
            return self.stop_nodes(true, o.wipe);
        };
        let exit = self.host_provider.exit_path();
        if !oc.frozen {
            self.drain().await?;
            eprintln!("→ stop the relayer and the sequencer");
            self.host_provider.stop("relayer")?;
            self.host_provider.stop("sequencer")?;
            // A checkpoint already sent can still land.
            tokio::time::sleep(Duration::from_secs(10)).await;
            self.export(&exit).await?;
            let unprocessed = self.extra.inbox_count > self.extra.inbox_through;
            if !unprocessed {
                eprintln!("→ trigger: a 1-stroop forced withdrawal of the admin's lane account, left unprocessed");
                self.cli
                    .request_forced_withdrawal(admin, &d.settlement, &d.admin, 1)?;
            }
            let window_end = now() + d.params.force_inclusion_window_secs;
            let at = self
                .freeze_possible_at()
                .map_or(window_end, |t| t.min(window_end))
                + 5;
            if o.no_wait {
                eprintln!(
                    "A freeze is possible from {} (in {} s). Run destroy again then.",
                    at,
                    at.saturating_sub(now())
                );
                return Ok(());
            }
            let wait = at.saturating_sub(now());
            if wait > 0 {
                eprintln!("→ wait {wait} s for the contract to allow a freeze");
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
            eprintln!("→ freeze");
            let deadline = now() + 600;
            loop {
                match self.cli.freeze(admin, &d.settlement) {
                    Ok(()) => break,
                    Err(e) if now() < deadline && e.to_string().contains("Error(Contract, #") => {
                        tokio::time::sleep(Duration::from_secs(10)).await;
                    }
                    Err(e) => return Err(e),
                }
            }
            eprintln!("Frozen. Every account's exit is in {}.", exit.display());
        } else {
            eprintln!("The lane is already frozen.");
            if !exit.exists() {
                self.export(&exit).await?;
            }
        }
        if o.pay_out {
            self.pay_out(&exit)?;
        }
        self.stop_nodes(o.stop_validators, o.wipe)
    }

    /// Waits until the last signed checkpoint is accepted on Stellar, and
    /// until there is one: every exit is proven against an accepted
    /// checkpoint, so a lane destroyed right after its first apply waits
    /// for its first.
    async fn drain(&self) -> Result<()> {
        let port = self.m.env.sequencer.port;
        if self.host_provider.status(port).await.is_none() {
            return Ok(());
        }
        eprintln!("→ drain: wait until every signed checkpoint is accepted, and at least one is");
        let deadline = now() + 600;
        loop {
            let s = self
                .host_provider
                .status(port)
                .await
                .ok_or_else(|| anyhow!("the sequencer stopped answering"))?;
            let n = |k: &str| {
                s["checkpoints"][k]
                    .as_str()
                    .and_then(|x| x.parse::<u64>().ok())
                    .unwrap_or(0)
            };
            if n("signed") == n("accepted") && n("accepted") >= 1 {
                return Ok(());
            }
            if now() > deadline {
                if n("accepted") == 0 && n("signed") == 0 {
                    bail!("no checkpoint was signed in 10 minutes: are the validators running?");
                }
                bail!("checkpoint {} was signed but not accepted in 10 minutes: is the relayer running?", n("signed"));
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    /// `export-proofs` on a validator that has seen Stellar's last checkpoint.
    async fn export(&self, exit: &std::path::Path) -> Result<()> {
        eprintln!("→ export every exit to {}", exit.display());
        let deadline = now() + 300;
        loop {
            let mut last_err = None;
            for v in &self.m.env.validators {
                let node = validator_node(&v.name);
                match self.host_provider.export_proofs(&node, exit) {
                    Ok(_) => return Ok(()),
                    Err(e) => last_err = Some(e),
                }
            }
            if now() > deadline {
                return Err(last_err
                    .unwrap_or_else(|| anyhow!("no validator"))
                    .context("no validator could export the last checkpoint's proofs"));
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }

    /// Claims every escape and withdrawal in `exit.json` for its owner.
    fn pay_out(&self, exit: &std::path::Path) -> Result<()> {
        let admin = self.m.env.admin.as_str();
        let e: Value = serde_json::from_str(&std::fs::read_to_string(exit)?)?;
        let key = |a: &Value| -> Result<[u8; 32]> {
            Ok(
                stellar_strkey::ed25519::PublicKey::from_string(a.as_str().unwrap_or(""))
                    .map_err(|_| anyhow!("bad account in exit.json"))?
                    .0,
            )
        };
        let proof = |p: &Value| -> Vec<String> {
            p.as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        };
        let (mut paid, mut skipped) = (0, 0);
        for l in e["escape"].as_array().into_iter().flatten() {
            let r = self.cli.escape_claim(
                admin,
                &self.desired.settlement,
                &key(&l["account"])?,
                l["index"].as_u64().unwrap_or(0) as u32,
                l["equity"].as_str().unwrap_or("0"),
                &proof(&l["proof"]),
            );
            match r {
                Ok(()) => paid += 1,
                Err(err) => {
                    skipped += 1;
                    eprintln!(
                        "  escape for {} not paid: {}",
                        l["account"],
                        err.to_string().lines().last().unwrap_or("")
                    );
                }
            }
        }
        for l in e["withdrawals"].as_array().into_iter().flatten() {
            let r = self.cli.claim_withdrawal(
                admin,
                &self.desired.settlement,
                &key(&l["account"])?,
                l["seq"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0),
                l["index"].as_u64().unwrap_or(0) as u32,
                l["amount"].as_str().unwrap_or("0"),
                &proof(&l["proof"]),
            );
            if r.is_ok() {
                paid += 1;
            } else {
                skipped += 1;
            }
        }
        eprintln!("Paid {paid} exit(s); {skipped} not paid (already claimed, or the owner can't hold USDC).");
        Ok(())
    }

    fn stop_nodes(&self, validators: bool, wipe: bool) -> Result<()> {
        if wipe {
            eprintln!("→ stop every node and wipe the host's lane data");
            return self.host_provider.wipe(&self.all_nodes());
        }
        self.host_provider.stop("relayer")?;
        self.host_provider.stop("sequencer")?;
        if validators {
            eprintln!("→ stop the validators");
            for v in &self.m.env.validators {
                self.host_provider.stop(&validator_node(&v.name))?;
            }
        } else {
            eprintln!("The validators keep running as the public proof source (--stop-validators stops them).");
        }
        Ok(())
    }
}
