//! `caravel-node replay` (spec §16): rebuilds the lane from Stellar data only.
//!
//! It reads the settlement contract's config and checkpoint records, pulls
//! each checkpoint's `header` and `batch` out of its `submit_checkpoint`
//! transaction, re-executes every block through the engine Wasm, and rebuilds
//! every header byte for byte. With `--prove-escape` / `--prove-withdrawals`
//! it prints the proofs a user needs to recover funds, with nothing but
//! Stellar RPC and this binary.

use anyhow::{anyhow, bail, Context, Result};
use caravel_core::batch::BatchV1;
use caravel_core::block::BlockRecordV1;
use caravel_core::checkpoint::CheckpointHeaderV1;
use caravel_core::receipts::ReceiptsV1;
use caravel_perps_node::{app as perps_app, PerpsApp};
use caravel_runtime::checkpoint::{self, block_hash, sha256, HeaderIds, Leaf};
use caravel_runtime::sequencer::{hex, Executor};
use caravel_runtime::views;
use caravel_runtime::LaneApp;
use caravel_types::state::StateV1;
use serde::Serialize;
use serde_json::{json, Value};
use stellar_xdr::{
    ContractDataDurability, ContractId, Hash, HostFunction, LedgerEntryData, LedgerKey,
    LedgerKeyContractData, Limits, OperationBody, ReadXdr, ScAddress, ScMap, ScSymbol, ScVal,
    ScVec, TransactionEnvelope, WriteXdr,
};

use crate::lane_toml::LaneFile;
use crate::stellar_rpc::{LastCheckpoint, Rpc};

/// The config fields replay checks (`Config` in the contract).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OnChainConfig {
    pub lane_id: [u8; 32],
    pub engine_wasm_hash: [u8; 32],
    pub genesis_state_hash: [u8; 32],
    pub config_hash: [u8; 32],
}

/// `Ckpt(seq)` as the contract stores it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointRecord {
    pub header_hash: [u8; 32],
    pub withdrawal_count: u32,
    pub stellar_ledger: u32,
}

/// Where replay reads Stellar's data from: RPC, or a fake in tests.
#[allow(async_fn_in_trait)]
pub trait ReplaySource {
    async fn config(&self) -> Result<OnChainConfig>;
    async fn last_checkpoint(&self) -> Result<LastCheckpoint>;
    async fn checkpoint(&self, seq: u64) -> Result<Option<CheckpointRecord>>;
    /// The `header` and `batch` arguments of the transaction that accepted `seq`.
    async fn checkpoint_args(
        &self,
        seq: u64,
        record: &CheckpointRecord,
    ) -> Result<(Vec<u8>, Vec<u8>)>;
    async fn is_claimed(&self, seq: u64, index: u32) -> Result<bool>;
}

/// A replayed checkpoint.
pub struct Replayed {
    pub header: CheckpointHeaderV1,
    pub blocks: Vec<(BlockRecordV1, ReceiptsV1)>,
}

pub struct Outcome {
    pub checkpoints: Vec<Replayed>,
    pub final_state: Vec<u8>,
}

#[derive(Serialize)]
pub struct Report {
    pub ok: bool,
    pub checkpoints: u64,
    pub final_height: u64,
    pub final_state_hash: String,
    pub summary: String,
}

fn mismatch(seq: u64, what: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("MISMATCH at checkpoint {seq}: {what}")
}

/// Replays every accepted checkpoint. The first mismatch is the error.
pub async fn replay<S: ReplaySource>(
    src: &S,
    lane: &LaneFile,
    exec: &Executor,
    wasm_hash: [u8; 32],
    ids: &HeaderIds,
) -> Result<Outcome> {
    let (_, config_bytes, native_genesis) = crate::lane_toml::genesis(lane)?;
    let cfg = src
        .config()
        .await
        .context("reading config() from the contract")?;
    if cfg.lane_id != lane.lane_id() {
        bail!(
            "MISMATCH: the contract's lane_id is {}, the lane file gives {}",
            hex(&cfg.lane_id),
            hex(&lane.lane_id())
        );
    }
    if cfg.config_hash != sha256(&config_bytes) {
        bail!(
            "MISMATCH: config_hash {} is not H(GenesisConfigV1 from the lane file) {}",
            hex(&cfg.config_hash),
            hex(&sha256(&config_bytes))
        );
    }
    if cfg.engine_wasm_hash != wasm_hash {
        bail!(
            "MISMATCH: engine_wasm_hash {} is not the hash of the engine Wasm given, {}",
            hex(&cfg.engine_wasm_hash),
            hex(&wasm_hash)
        );
    }
    let genesis_state = match exec {
        Executor::Wasm(w) => {
            w.genesis(&config_bytes)
                .map_err(|e| anyhow!("wasm genesis: {e:?}"))?
                .0
        }
        Executor::Native => native_genesis,
    };
    if cfg.genesis_state_hash != sha256(&genesis_state) {
        bail!(
            "MISMATCH: genesis_state_hash {} is not H(genesis(config)) {}",
            hex(&cfg.genesis_state_hash),
            hex(&sha256(&genesis_state))
        );
    }
    let last = src
        .last_checkpoint()
        .await
        .context("reading last_checkpoint()")?;
    let mut state = genesis_state;
    let mut prev_header = [0u8; 32];
    let mut prev_block_hash = [0u8; 32];
    let mut out = Vec::new();
    for seq in 1..=last.seq {
        let record = src
            .checkpoint(seq)
            .await?
            .ok_or_else(|| mismatch(seq, "no Ckpt record on Stellar"))?;
        let (header_bytes, batch_bytes) = src
            .checkpoint_args(seq, &record)
            .await
            .with_context(|| format!("fetching the submit_checkpoint transaction for {seq}"))?;
        if sha256(&header_bytes) != record.header_hash {
            return Err(mismatch(
                seq,
                "H(header) from the transaction is not Ckpt(seq).header_hash",
            ));
        }
        let header = CheckpointHeaderV1::decode(&header_bytes)
            .map_err(|_| mismatch(seq, "header does not decode"))?;
        if sha256(&batch_bytes) != header.batch_hash {
            return Err(mismatch(seq, "H(batch) is not header.batch_hash"));
        }
        let batch =
            BatchV1::decode(&batch_bytes).map_err(|_| mismatch(seq, "batch does not decode"))?;
        if batch.checkpoint_seq != seq || batch.lane_id != cfg.lane_id {
            return Err(mismatch(seq, "batch seq or lane_id"));
        }
        let mut blocks = Vec::with_capacity(batch.blocks.len());
        for record in &batch.blocks {
            let input = record
                .decode_input()
                .map_err(|_| mismatch(seq, "a block does not decode"))?;
            if input.prev_block_hash != prev_block_hash {
                return Err(mismatch(
                    seq,
                    format!(
                        "block {}: prev_block_hash does not continue the chain",
                        input.height
                    ),
                ));
            }
            let (o, _) = exec.step(&PerpsApp, &state, &record.input).map_err(|e| {
                mismatch(
                    seq,
                    format!("block {} does not execute: {e:?}", input.height),
                )
            })?;
            if sha256(&o.state) != record.state_hash_after {
                return Err(mismatch(
                    seq,
                    format!(
                        "block {}: state_hash_after differs from re-execution",
                        input.height
                    ),
                ));
            }
            state = o.state;
            prev_block_hash = block_hash(record);
            if !PerpsApp.receipts_decode(&o.receipts) {
                return Err(mismatch(seq, "receipts"));
            }
            let receipts =
                ReceiptsV1::decode(&o.receipts).map_err(|_| mismatch(seq, "receipts"))?;
            blocks.push((record.clone(), receipts));
        }
        if PerpsApp.decode_state(&state).is_none() {
            return Err(mismatch(seq, "state does not decode"));
        }
        let (_, rebuilt) = checkpoint::assemble(ids, prev_header, &batch.blocks, &state)
            .map_err(|e| mismatch(seq, format!("the batch does not end a checkpoint: {e:?}")))?;
        if rebuilt.encode() != header.encode() {
            return Err(mismatch(seq, first_difference(&rebuilt, &header)));
        }
        prev_header = sha256(&header_bytes);
        out.push(Replayed { header, blocks });
    }
    if last.seq > 0 && last.header_hash != prev_header {
        bail!(
            "MISMATCH: last_checkpoint().header_hash is not the hash of checkpoint {}",
            last.seq
        );
    }
    Ok(Outcome {
        checkpoints: out,
        final_state: state,
    })
}

/// Names the first header field that differs.
fn first_difference(ours: &CheckpointHeaderV1, theirs: &CheckpointHeaderV1) -> String {
    let a = views::header(ours);
    let b = views::header(theirs);
    let (a, b) = (
        serde_json::to_value(a).unwrap_or_default(),
        serde_json::to_value(b).unwrap_or_default(),
    );
    if let (Value::Object(a), Value::Object(b)) = (&a, &b) {
        for (k, v) in a {
            if b.get(k) != Some(v) {
                return format!(
                    "header field {k}: re-execution gives {v}, Stellar has {}",
                    b.get(k).cloned().unwrap_or_default()
                );
            }
        }
    }
    "the rebuilt header differs".into()
}

pub fn report(o: &Outcome) -> Report {
    let n = o.checkpoints.len() as u64;
    let height = o
        .checkpoints
        .last()
        .map_or(0, |c| c.header.last_block_height);
    let hash = hex(&sha256(&o.final_state));
    Report {
        ok: true,
        checkpoints: n,
        final_height: height,
        final_state_hash: hash.clone(),
        summary: format!("OK seq=1..{n} final_state_hash={hash}"),
    }
}

/// The escape leaf of `account` in the last accepted checkpoint.
pub fn escape_proof(o: &Outcome, account: &[u8; 32]) -> Result<Value> {
    let last = o
        .checkpoints
        .last()
        .ok_or_else(|| anyhow!("no checkpoint accepted yet"))?;
    let st = StateV1::decode(&o.final_state).map_err(|_| anyhow!("state"))?;
    let leaves = perps_app::account_leaves(&last.header, &st)
        .map_err(|_| anyhow!("account leaves do not match the header"))?;
    let hashes = checkpoint::account_hashes(&last.header, &leaves);
    let p = views::proofs_for(&last.header, &leaves, &hashes, account)
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("the account has no leaf in checkpoint {}", last.header.seq))?;
    Ok(
        json!({ "seq": p.seq, "index": p.index, "account": p.account, "equity": p.amount, "proof": p.proof }),
    )
}

/// Every withdrawal leaf of `account` not yet claimed on Stellar.
pub async fn withdrawal_proofs<S: ReplaySource>(
    src: &S,
    o: &Outcome,
    account: &[u8; 32],
) -> Result<Value> {
    let mut out = Vec::new();
    for c in &o.checkpoints {
        let leaves: Vec<Leaf> =
            checkpoint::withdrawal_leaves(&c.header, &c.blocks).map_err(|_| {
                anyhow!(
                    "withdrawal leaves of {} do not match the header",
                    c.header.seq
                )
            })?;
        let hashes = checkpoint::withdrawal_hashes(&c.header, &leaves);
        for p in views::proofs_for(&c.header, &leaves, &hashes, account) {
            if !src.is_claimed(c.header.seq, p.index).await? {
                out.push(p);
            }
        }
    }
    Ok(json!({ "account": views::g_address(account), "withdrawals": out }))
}

// --- Stellar RPC source ------------------------------------------------------------------------

pub struct RpcSource {
    pub rpc: Rpc,
    pub contract: [u8; 32],
}

fn sym(s: &str) -> ScVal {
    ScVal::Symbol(ScSymbol(s.try_into().expect("short symbol")))
}

/// A `#[contracttype]` enum variant key: `Vec[Symbol(name), fields...]`.
pub fn variant(name: &str, fields: Vec<ScVal>) -> ScVal {
    let mut v = vec![sym(name)];
    v.extend(fields);
    ScVal::Vec(Some(ScVec(v.try_into().expect("small vec"))))
}

fn field<'a>(m: &'a ScMap, name: &str) -> Result<&'a ScVal> {
    m.0.iter()
        .find(|e| e.key == sym(name))
        .map(|e| &e.val)
        .ok_or_else(|| anyhow!("no field {name}"))
}

fn bytes32(v: &ScVal, what: &str) -> Result<[u8; 32]> {
    match v {
        ScVal::Bytes(b) => {
            b.0.as_slice()
                .try_into()
                .map_err(|_| anyhow!("{what} is not 32 bytes"))
        }
        _ => bail!("{what} is not bytes"),
    }
}

impl RpcSource {
    fn persistent_key(&self, key: ScVal) -> LedgerKey {
        LedgerKey::ContractData(LedgerKeyContractData {
            contract: ScAddress::Contract(ContractId(Hash(self.contract))),
            key,
            durability: ContractDataDurability::Persistent,
        })
    }

    async fn persistent(&self, key: ScVal) -> Result<Option<ScVal>> {
        match self.rpc.ledger_entry(&self.persistent_key(key)).await? {
            Some(LedgerEntryData::ContractData(d)) => Ok(Some(d.val)),
            Some(_) => bail!("not a contract data entry"),
            None => Ok(None),
        }
    }

    fn contract_strkey(&self) -> String {
        stellar_strkey::Contract(self.contract)
            .to_string()
            .as_str()
            .to_owned()
    }
}

impl ReplaySource for RpcSource {
    async fn config(&self) -> Result<OnChainConfig> {
        let storage = self
            .rpc
            .instance_storage(&self.contract)
            .await?
            .ok_or_else(|| anyhow!("the settlement contract has no instance storage"))?;
        let key = variant("Config", vec![]);
        let ScVal::Map(Some(m)) = &storage
            .0
            .iter()
            .find(|e| e.key == key)
            .ok_or_else(|| anyhow!("no Config in instance storage"))?
            .val
        else {
            bail!("Config is not a map");
        };
        Ok(OnChainConfig {
            lane_id: bytes32(field(m, "lane_id")?, "lane_id")?,
            engine_wasm_hash: bytes32(field(m, "engine_wasm_hash")?, "engine_wasm_hash")?,
            genesis_state_hash: bytes32(field(m, "genesis_state_hash")?, "genesis_state_hash")?,
            config_hash: bytes32(field(m, "config_hash")?, "config_hash")?,
        })
    }

    async fn last_checkpoint(&self) -> Result<LastCheckpoint> {
        self.rpc
            .last_checkpoint(&self.contract)
            .await?
            .ok_or_else(|| anyhow!("no LastCkpt in instance storage"))
    }

    async fn checkpoint(&self, seq: u64) -> Result<Option<CheckpointRecord>> {
        let Some(ScVal::Map(Some(m))) = self
            .persistent(variant("Ckpt", vec![ScVal::U64(seq)]))
            .await?
        else {
            return Ok(None);
        };
        let ScVal::U32(withdrawal_count) = field(&m, "withdrawal_count")? else {
            bail!("withdrawal_count")
        };
        let ScVal::U32(stellar_ledger) = field(&m, "stellar_ledger")? else {
            bail!("stellar_ledger")
        };
        Ok(Some(CheckpointRecord {
            header_hash: bytes32(field(&m, "header_hash")?, "header_hash")?,
            withdrawal_count: *withdrawal_count,
            stellar_ledger: *stellar_ledger,
        }))
    }

    async fn checkpoint_args(
        &self,
        seq: u64,
        record: &CheckpointRecord,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        let topics = vec![
            sym("ckpt").to_xdr_base64(Limits::none())?,
            ScVal::U64(seq).to_xdr_base64(Limits::none())?,
        ];
        let events = self
            .rpc
            .call(
                "getEvents",
                json!({ "startLedger": record.stellar_ledger, "filters": [{ "type": "contract", "contractIds": [self.contract_strkey()], "topics": [topics] }], "pagination": { "limit": 5 } }),
            )
            .await
            .with_context(|| format!("getEvents from ledger {} (RPC keeps about 7 days; older data needs an archive or a validator store)", record.stellar_ledger))?;
        let tx_hash = events["events"]
            .as_array()
            .and_then(|e| e.first())
            .and_then(|e| e["txHash"].as_str())
            .ok_or_else(|| {
                anyhow!(
                    "no ckpt event for seq {seq} from ledger {}",
                    record.stellar_ledger
                )
            })?
            .to_string();
        let tx = self
            .rpc
            .call("getTransaction", json!({ "hash": tx_hash }))
            .await?;
        if tx["status"] != "SUCCESS" {
            bail!("transaction {tx_hash} status {}", tx["status"]);
        }
        let env = TransactionEnvelope::from_xdr_base64(
            tx["envelopeXdr"]
                .as_str()
                .ok_or_else(|| anyhow!("no envelopeXdr"))?,
            crate::stellar_rpc::read_limits(),
        )?;
        submit_checkpoint_args(&env).with_context(|| format!("transaction {tx_hash}"))
    }

    async fn is_claimed(&self, seq: u64, index: u32) -> Result<bool> {
        Ok(self
            .persistent(variant("Claimed", vec![ScVal::U64(seq), ScVal::U32(index)]))
            .await?
            .is_some())
    }
}

/// `header` and `batch` from a `submit_checkpoint` invocation envelope.
pub fn submit_checkpoint_args(env: &TransactionEnvelope) -> Result<(Vec<u8>, Vec<u8>)> {
    let ops = match env {
        TransactionEnvelope::Tx(t) => &t.tx.operations,
        TransactionEnvelope::TxFeeBump(f) => match &f.tx.inner_tx {
            stellar_xdr::FeeBumpTransactionInnerTx::Tx(t) => &t.tx.operations,
        },
        TransactionEnvelope::TxV0(_) => bail!("a v0 envelope cannot invoke a contract"),
    };
    for op in ops.iter() {
        if let OperationBody::InvokeHostFunction(ih) = &op.body {
            if let HostFunction::InvokeContract(args) = &ih.host_function {
                if args.function_name.0.as_slice() == b"submit_checkpoint" && args.args.len() >= 2 {
                    let bytes = |v: &ScVal| match v {
                        ScVal::Bytes(b) => Ok(b.0.to_vec()),
                        _ => Err(anyhow!("argument is not bytes")),
                    };
                    return Ok((bytes(&args.args[0])?, bytes(&args.args[1])?));
                }
            }
        }
    }
    bail!("no submit_checkpoint call in the transaction")
}
