//! Running a deployment day to day (M0.6, C-11, DEC-083): stopping and
//! starting nodes without winding the lane down, its API, and waiting for
//! what scripts used to poll for (a checkpoint, a value in the API, healthy
//! nodes, a freeze).

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

use crate::deploy::Prepared;
use crate::plan::{Plan, Step};

/// A node's name as the tool uses it: `sequencer`, `relayer`, `validator-<n>`
/// (`<n>` alone is a validator too).
pub fn node_name(given: &str, all: &[String]) -> Result<String> {
    if all.iter().any(|n| n == given) {
        return Ok(given.to_string());
    }
    let v = format!("validator-{given}");
    if all.contains(&v) {
        return Ok(v);
    }
    let mut msg = format!("no node {given:?}; the deployment has {}", all.join(", "));
    if let Some(m) = caravel_lanefile::did_you_mean(given, all.iter().map(String::as_str)) {
        msg += &format!(" (did you mean {m:?}?)");
    }
    bail!("{msg}")
}

/// The order nodes stop in: the relayer, the sequencer, then validators.
fn stop_order(nodes: &mut [String]) {
    nodes.sort_by_key(|n| match n.as_str() {
        "relayer" => 0,
        "sequencer" => 1,
        _ => 2,
    });
}

impl Prepared {
    /// The nodes named (all of them when none is), in stop order.
    pub fn nodes(&self, given: &[String]) -> Result<Vec<String>> {
        let all = self.all_nodes();
        let mut out = if given.is_empty() {
            all.clone()
        } else {
            given
                .iter()
                .map(|g| node_name(g, &all))
                .collect::<Result<Vec<_>>>()?
        };
        out.dedup();
        stop_order(&mut out);
        Ok(out)
    }

    /// Stops nodes and leaves the lane as it is. While the sequencer is down
    /// no checkpoint is posted, and after the contract's escape timeout
    /// anyone may freeze the lane: [`Prepared::freeze_possible_at`] says when.
    pub fn stop_nodes_named(&self, nodes: &[String]) -> Result<()> {
        for n in nodes {
            eprintln!("→ stop {n}");
            self.host_provider.stop(n)?;
        }
        Ok(())
    }

    /// The part of the plan that starts the named nodes (all when none is).
    /// Anything else in the plan, a file or a contract to change, is for
    /// `apply`, and `start` refuses it.
    pub fn start_plan(&self, nodes: &[String]) -> Result<Plan> {
        let plan = self.plan();
        if !plan.problems.is_empty() {
            bail!("the plan has problems: run `caravel plan`");
        }
        let starts = |s: &Step| matches!(s, Step::Start { .. } | Step::Restart { .. });
        if let Some(other) = plan.steps.iter().find(|s| !starts(s)) {
            bail!(
                "the deployment differs from the lane file beyond its nodes ({}): run `caravel apply`",
                crate::plan::step_line(other).trim()
            );
        }
        let mut out = plan.clone();
        out.steps.retain(|s| match s {
            Step::Start { node } | Step::Restart { node } => {
                nodes.is_empty() || nodes.contains(node)
            }
            _ => false,
        });
        Ok(out)
    }

    /// Where users reach the lane's API, or one validator's.
    pub fn api_base(&self, validator: Option<&str>) -> Result<String> {
        match validator {
            None => crate::deploy::api_url(&self.m)
                .ok_or_else(|| anyhow!("the host has no public_url: pass --api-url")),
            Some(v) => {
                let all = self.all_nodes();
                let node = node_name(v, &all)?;
                let i = self
                    .desired
                    .validators
                    .iter()
                    .position(|n| *n == node)
                    .ok_or_else(|| anyhow!("{node} is not one of the lane file's validators"))?;
                crate::deploy::validator_url(&self.m, i)
                    .ok_or_else(|| anyhow!("the host has no public_url: pass --api-url"))
            }
        }
    }
}

/// GET `<base><path>` as JSON.
pub async fn get(base: &str, path: &str) -> Result<Value> {
    let url = format!("{}{}", base.trim_end_matches('/'), path);
    let r = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?
        .get(&url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = r.status();
    let body: Value = r.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        bail!("GET {url}: {status} {body}");
    }
    Ok(body)
}

/// What `caravel wait` waits for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wait {
    /// Checkpoint `seq` (the next one when none) accepted on Stellar, or only
    /// signed; at `epoch` if given.
    Checkpoint {
        seq: Option<u64>,
        signed: bool,
        epoch: Option<u64>,
    },
    /// The JSON at `path` in the lane's API has `pointer` (`/0/oracle_price`),
    /// equal to `value` when given.
    Api {
        path: String,
        pointer: String,
        value: Option<String>,
    },
    /// Every node running and answering.
    Healthy,
    /// The settlement contract frozen.
    Frozen,
}

fn num(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// Whether `/v1/status` shows checkpoint `seq` past the stage asked for.
pub fn checkpoint_reached(status: &Value, seq: u64, signed: bool) -> bool {
    let c = &status["checkpoints"];
    let have = if signed {
        num(&c["signed"]).max(num(&c["accepted"]))
    } else {
        num(&c["accepted"])
    };
    have.is_some_and(|h| h >= seq)
}

/// Whether `/v1/checkpoints/<seq>` is at the stage and epoch asked for.
pub fn checkpoint_is(doc: &Value, signed: bool, epoch: Option<u64>) -> bool {
    let stage = doc["status"].as_str().unwrap_or_default();
    let stage_ok = stage == "accepted" || (signed && stage == "signed");
    stage_ok && epoch.is_none_or(|e| num(&doc["epoch"]) == Some(e))
}

/// Whether `pointer` in `v` exists, and equals `want` (as JSON, or as the
/// string form of the value) when given.
pub fn api_matches(v: &Value, pointer: &str, want: Option<&str>) -> bool {
    let Some(got) = v.pointer(pointer) else {
        return false;
    };
    match want {
        None => !got.is_null(),
        Some(w) => {
            let as_text = match got {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            as_text == w || serde_json::from_str::<Value>(w).is_ok_and(|j| j == *got)
        }
    }
}

impl Prepared {
    /// Waits for `w`, polling every `every`; false when `timeout` runs out.
    pub async fn wait(
        &self,
        w: &Wait,
        timeout: Duration,
        every: Duration,
        api: &str,
    ) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        // The next checkpoint: one past what is accepted now.
        let target = match w {
            Wait::Checkpoint {
                seq: None, signed, ..
            } => {
                let s = get(api, "/v1/status").await?;
                let c = &s["checkpoints"];
                let now = if *signed {
                    num(&c["signed"])
                } else {
                    num(&c["accepted"])
                };
                Some(now.unwrap_or(0) + 1)
            }
            _ => None,
        };
        loop {
            let done = match w {
                Wait::Checkpoint { seq, signed, epoch } => match seq {
                    Some(n) => get(api, &format!("/v1/checkpoints/{n}"))
                        .await
                        .is_ok_and(|d| checkpoint_is(&d, *signed, *epoch)),
                    None => get(api, "/v1/status")
                        .await
                        .is_ok_and(|s| checkpoint_reached(&s, target.unwrap_or(1), *signed)),
                },
                Wait::Api {
                    path,
                    pointer,
                    value,
                } => get(api, path)
                    .await
                    .is_ok_and(|v| api_matches(&v, pointer, value.as_deref())),
                Wait::Healthy => {
                    let host = self.host_provider.read(&self.m).await?;
                    self.all_nodes().iter().all(|n| {
                        host.nodes
                            .get(n)
                            .is_some_and(|s| s.running && (n == "relayer" || s.report.is_some()))
                    })
                }
                Wait::Frozen => {
                    let rpc = caravel_node::stellar_rpc::Rpc::new(self.m.rpc_url())?;
                    crate::chain::read(&rpc, &self.desired)
                        .await
                        .is_ok_and(|(c, _)| c.settlement.is_some_and(|s| s.frozen))
                }
            };
            if done {
                return Ok(true);
            }
            if Instant::now() + every > deadline {
                return Ok(false);
            }
            tokio::time::sleep(every).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn node_names() {
        let all: Vec<String> = ["sequencer", "relayer", "validator-1", "validator-2"]
            .map(String::from)
            .into();
        assert_eq!(node_name("1", &all).unwrap(), "validator-1");
        assert_eq!(node_name("relayer", &all).unwrap(), "relayer");
        let e = node_name("sequncer", &all).unwrap_err().to_string();
        assert!(e.contains("did you mean \"sequencer\"?"), "{e}");
        let mut order: Vec<String> = ["validator-2", "sequencer", "relayer"]
            .map(String::from)
            .into();
        stop_order(&mut order);
        assert_eq!(order, ["relayer", "sequencer", "validator-2"]);
    }

    #[test]
    fn conditions() {
        let s = json!({ "checkpoints": { "accepted": "4", "signed": "5" } });
        assert!(checkpoint_reached(&s, 4, false));
        assert!(!checkpoint_reached(&s, 5, false));
        assert!(checkpoint_reached(&s, 5, true));
        assert!(!checkpoint_reached(&json!({ "checkpoints": {} }), 1, false));
        let c = json!({ "status": "accepted", "epoch": "2" });
        assert!(checkpoint_is(&c, false, Some(2)));
        assert!(!checkpoint_is(&c, false, Some(1)));
        assert!(checkpoint_is(&json!({ "status": "signed" }), true, None));
        assert!(!checkpoint_is(&json!({ "status": "signed" }), false, None));
        let m = json!([{ "oracle_price": "65000000", "n": 3, "ok": true }]);
        assert!(api_matches(&m, "/0/oracle_price", Some("65000000")));
        assert!(api_matches(&m, "/0/n", Some("3")));
        assert!(api_matches(&m, "/0/ok", Some("true")));
        assert!(api_matches(&m, "/0/oracle_price", None));
        assert!(!api_matches(&m, "/0/oracle_price", Some("1")));
        assert!(!api_matches(&m, "/1/oracle_price", None));
    }
}
