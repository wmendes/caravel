//! Builds a Payments checkpoint on the native harness lane, then runs the
//! guest over it: `exec` counts cycles, `prove` makes a receipt (`succinct`
//! or `groth16`), `tamper` checks a changed block can't be proven.
//!
//!   cargo run --release -p zk-payments-host -- exec   --accounts 64 --blocks 20 --txs 10
//!   cargo run --release -p zk-payments-host -- prove  --accounts 64 --blocks 20 --txs 10 --kind groth16 --out proof.json
//!   cargo run --release -p zk-payments-host -- tamper --accounts 8 --blocks 2 --txs 2

use std::time::Instant;

use anyhow::{anyhow, bail, ensure, Context, Result};
use caravel_app_sdk::AppEngine;
use caravel_core::tx::SigScheme;
use caravel_harness::{pk, Lane, USDC};
use caravel_payments::{Params, Payments, Transfer, TRANSFER};
use caravel_runtime::checkpoint::{assemble, network_id, settlement_addr_hash, sha256, HeaderIds};
use risc0_zkvm::{default_executor, default_prover, ExecutorEnv, ProverOpts};
use zk_payments_methods::{CHECKPOINT_ELF, CHECKPOINT_ID};

const TREASURY: u8 = 0x22;

struct Args {
    mode: String,
    accounts: u8,
    blocks: usize,
    txs: usize,
    kind: String,
    out: Option<String>,
}

fn args() -> Result<Args> {
    let mut a = std::env::args().skip(1);
    let mode = a.next().ok_or_else(|| anyhow!("mode: exec | prove | tamper"))?;
    let mut out = Args { mode, accounts: 16, blocks: 10, txs: 4, kind: "succinct".into(), out: None };
    while let Some(k) = a.next() {
        let v = a.next().ok_or_else(|| anyhow!("{k} needs a value"))?;
        match k.as_str() {
            "--accounts" => out.accounts = v.parse()?,
            "--blocks" => out.blocks = v.parse()?,
            "--txs" => out.txs = v.parse()?,
            "--kind" => out.kind = v,
            "--out" => out.out = Some(v),
            _ => bail!("unknown {k}"),
        }
    }
    ensure!(out.accounts >= 2 && out.accounts < 200, "--accounts 2..199");
    Ok(out)
}

/// The previous checkpoint (deposits for every account), then a batch of
/// `blocks` blocks with `txs` transfers each, the last a CHECKPOINT_END.
struct Fixture {
    ids: HeaderIds,
    prev_header: Vec<u8>,
    prev_state: Vec<u8>,
    batch: Vec<u8>,
    header: Vec<u8>,
}

fn fixture(a: &Args) -> Result<Fixture> {
    let mut config = caravel_harness::config();
    config.template = Payments::TEMPLATE;
    config.system_keys = vec![pk(TREASURY)];
    config.app_params = Params { transfer_fee: USDC / 100, min_transfer: 1 }.encode();
    let mut l = Lane::<Payments>::new(config);
    let seed = |i: u8| 100 + i;
    for i in 0..a.accounts {
        l.deposit(seed(i), 1_000 * USDC);
    }
    l.checkpoint();
    let ids = HeaderIds {
        network_id: network_id("Test SDF Network ; September 2015"),
        settlement_addr_hash: settlement_addr_hash(&[7; 32]),
        engine_wasm_hash: [0xEE; 32],
    };
    let (_, prev) = assemble(&ids, [0; 32], &l.records, &l.state_bytes).map_err(|e| anyhow!("{e:?}"))?;
    let prev_state = l.state_bytes.clone();
    let first = l.records.len();
    let mut n = 0usize;
    for b in 0..a.blocks {
        for _ in 0..a.txs {
            let from = (n % a.accounts as usize) as u8;
            let to = ((n * 7 + 1) % a.accounts as usize) as u8;
            let to = if to == from { (to + 1) % a.accounts } else { to };
            let body = Transfer { to: pk(seed(to)), amount: USDC, memo: n as u64 }.encode();
            l.tx_as(seed(from), seed(from), SigScheme::RawEd25519, TRANSFER, body);
            n += 1;
        }
        if b + 1 == a.blocks {
            l.checkpoint();
        } else {
            l.block();
        }
    }
    let (batch, header) = assemble(&ids, sha256(&prev.encode()), &l.records[first..], &l.state_bytes)
        .map_err(|e| anyhow!("{e:?}"))?;
    Ok(Fixture {
        ids,
        prev_header: prev.encode().to_vec(),
        prev_state,
        batch,
        header: header.encode().to_vec(),
    })
}

fn env(f: &Fixture, batch: &[u8]) -> Result<ExecutorEnv<'static>> {
    let mut ids = Vec::with_capacity(96);
    ids.extend_from_slice(&f.ids.network_id);
    ids.extend_from_slice(&f.ids.settlement_addr_hash);
    ids.extend_from_slice(&f.ids.engine_wasm_hash);
    let mut b = ExecutorEnv::builder();
    for part in [ids, f.prev_header.clone(), f.prev_state.clone(), batch.to_vec()] {
        let mut padded = part.clone();
        padded.resize((part.len() + 3) & !3, 0);
        b.write_slice(&(part.len() as u32).to_le_bytes());
        b.write_slice(&padded);
    }
    b.build()
}

fn expected_journal(f: &Fixture) -> Vec<u8> {
    [sha256(&f.prev_header), sha256(&f.header)].concat()
}

fn image_id_hex() -> String {
    CHECKPOINT_ID.iter().flat_map(|w| w.to_le_bytes()).map(|b| format!("{b:02x}")).collect()
}

fn main() -> Result<()> {
    let a = args()?;
    let t = Instant::now();
    let f = fixture(&a)?;
    let txs = a.blocks * a.txs;
    eprintln!(
        "fixture: {} accounts, {} blocks, {} transfers; state {} B, batch {} B ({:.1?})",
        a.accounts, a.blocks, txs, f.prev_state.len(), f.batch.len(), t.elapsed()
    );
    match a.mode.as_str() {
        "exec" => {
            let t = Instant::now();
            let s = default_executor().execute(env(&f, &f.batch)?, CHECKPOINT_ELF)?;
            ensure!(s.journal.bytes == expected_journal(&f), "journal differs from the node's header");
            let cycles: u64 = s.segments.iter().map(|g| 1u64 << g.po2).sum();
            println!(
                "{}",
                serde_json::json!({
                    "accounts": a.accounts, "blocks": a.blocks, "transfers": txs,
                    "state_bytes": f.prev_state.len(), "batch_bytes": f.batch.len(),
                    "user_cycles": s.cycles(), "segments": s.segments.len(), "padded_cycles": cycles,
                    "exec_ms": t.elapsed().as_millis(), "journal_matches_node_header": true,
                    "image_id": image_id_hex(),
                })
            );
        }
        "prove" => {
            let opts = match a.kind.as_str() {
                "succinct" => ProverOpts::succinct(),
                "groth16" => ProverOpts::groth16(),
                "composite" => ProverOpts::composite(),
                k => bail!("unknown kind {k}"),
            };
            let t = Instant::now();
            let info = default_prover().prove_with_opts(env(&f, &f.batch)?, CHECKPOINT_ELF, &opts)?;
            let prove_ms = t.elapsed().as_millis();
            info.receipt.verify(CHECKPOINT_ID).context("receipt verifies")?;
            ensure!(info.receipt.journal.bytes == expected_journal(&f), "journal differs");
            let mut out = serde_json::json!({
                "kind": a.kind, "accounts": a.accounts, "blocks": a.blocks, "transfers": txs,
                "user_cycles": info.stats.user_cycles, "total_cycles": info.stats.total_cycles,
                "segments": info.stats.segments, "prove_ms": prove_ms,
                "image_id": image_id_hex(),
                "journal": hex::encode(&info.receipt.journal.bytes),
                "journal_digest": hex::encode(sha256(&info.receipt.journal.bytes)),
                "prev_header": hex::encode(&f.prev_header),
                "header": hex::encode(&f.header),
                "batch": hex::encode(&f.batch),
            });
            if let Ok(g) = info.receipt.inner.groth16() {
                let selector = &g.verifier_parameters.as_bytes()[..4];
                out["seal"] = hex::encode([selector, g.seal.as_slice()].concat()).into();
            }
            let text = serde_json::to_string_pretty(&out)?;
            match &a.out {
                Some(p) => std::fs::write(p, &text)?,
                None => println!("{text}"),
            }
            eprintln!("proved {} in {prove_ms} ms", a.kind);
        }
        "tamper" => {
            // A transfer amount in the batch's last block, changed: the
            // guest must refuse it (the state hashes no longer match).
            let mut bad = f.batch.clone();
            let i = bad.len() - 40;
            bad[i] ^= 1;
            let r = default_executor().execute(env(&f, &bad)?, CHECKPOINT_ELF);
            ensure!(r.is_err(), "a tampered batch executed");
            println!("tampered batch refused: {}", r.err().map(|e| e.to_string()).unwrap_or_default().lines().next().unwrap_or(""));
        }
        m => bail!("unknown mode {m}"),
    }
    Ok(())
}
