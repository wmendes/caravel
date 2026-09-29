//! Replay (spec §16) against checkpoints a real sequencer core produced on
//! the local lane, served by an in-memory stand-in for Stellar: it must
//! reproduce every header through the Wasm, find each kind of tampering,
//! and produce proofs that verify.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Result;
use caravel_lane::checkpoint::{network_id, settlement_addr_hash, sha256, HeaderIds};
use caravel_lane::sequencer::{unhex, Core, Executor, InboxReport, SequencerConfig};
use caravel_lane::store::{CheckpointStatus, Store};
use caravel_lane::WasmExecutor;
use caravel_node::lane_toml::LaneFile;
use caravel_node::replay::{self, CheckpointRecord, OnChainConfig, ReplaySource};
use caravel_node::stellar_rpc::LastCheckpoint;
use caravel_types::batch::BatchV1;
use caravel_types::checkpoint::CheckpointHeaderV1;
use caravel_types::inbox::{inbox_acc_preimage, InboxKind, InboxMsgV1};
use caravel_types::oracle::OracleUpdateV1;
use caravel_types::tx::{LaneTxV1, PlaceOrder, Side, SigScheme, Tif, TxBody};
use caravel_types::vectors::{key, pk};
use ed25519_dalek::Signer;

const A: u8 = 0x41;
const B: u8 = 0x42;
const USDC: i128 = 10_000_000;
const T0: u64 = 1_790_000_000_000;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn lane() -> LaneFile {
    LaneFile::load(&root().join("config/lane.caravel-perps.local.toml")).unwrap()
}

fn wasm() -> (WasmExecutor, [u8; 32]) {
    let bytes = std::fs::read(root().join("target/contracts/perps_engine.wasm"))
        .expect("run ./scripts/build-contracts.sh first");
    let hash = sha256(&bytes);
    let (_, config_bytes, _) = caravel_node::lane_toml::genesis(&lane()).unwrap();
    let g = caravel_types::config::GenesisConfigV1::decode(&config_bytes).unwrap();
    (
        WasmExecutor::new(bytes, hash, g.exec_cpu_limit, g.exec_mem_limit).unwrap(),
        hash,
    )
}

fn ids(wasm_hash: [u8; 32]) -> HeaderIds {
    HeaderIds {
        network_id: network_id("Test SDF Network ; September 2015"),
        settlement_addr_hash: settlement_addr_hash(&[7; 32]),
        engine_wasm_hash: wasm_hash,
    }
}

/// A native sequencer core that produced 25 blocks (checkpoints 1 and 2).
fn produced() -> (Core, [u8; 32]) {
    let lane = lane();
    let (_, config_bytes, genesis) = caravel_node::lane_toml::genesis(&lane).unwrap();
    let config_hash = sha256(&config_bytes);
    let (_, wasm_hash) = wasm();
    let store = Store::open_in_memory(&lane.lane_id(), &config_hash, &genesis).unwrap();
    let cfg = SequencerConfig {
        ids: ids(wasm_hash),
        checkpoint_every_blocks: 10,
        max_batch_bytes: 96_000,
        mempool_max: 1000,
        mempool_max_per_account: 100,
    };
    let mut core = Core::open(Executor::Native, store, config_hash, cfg).unwrap();
    let lane_id = lane.lane_id();
    let mut now = T0;
    let mut acc = [0u8; 32];
    for (i, seed) in [A, B].into_iter().enumerate() {
        let msg = InboxMsgV1 {
            kind: InboxKind::Deposit,
            index: i as u64,
            lane_account: pk(seed),
            amount: 10_000 * USDC,
            enqueued_at: now / 1000,
        };
        acc = sha256(&inbox_acc_preimage(&acc, &msg.encode()));
        assert_eq!(core.report_inbox(msg, acc).unwrap(), InboxReport::Added);
    }
    let oracle = |core: &mut Core, now: u64| {
        let mut u = OracleUpdateV1 {
            market_id: 1,
            price: 65_000_000,
            publish_time_ms: now,
            oracle_key: pk(0x31),
            signature: [0; 64],
        };
        u.signature = key(0x31)
            .sign(&sha256(&u.signing_preimage(&lane_id)))
            .to_bytes();
        assert!(core.report_oracle(u).unwrap());
    };
    oracle(&mut core, now);
    core.produce_block(now).unwrap();
    now += 1000;
    let tx = |core: &Core, seed: u8, nonce: u64, body: TxBody, now: u64| {
        let mut tx = LaneTxV1 {
            lane_id,
            account: pk(seed),
            signer: pk(seed),
            nonce,
            expiry_ms: now + 60_000,
            sig_scheme: SigScheme::RawEd25519,
            body,
            signature: [0; 64],
        };
        tx.signature = key(seed)
            .sign(&sha256(&tx.tx_hash_preimage(&core.config_hash())))
            .to_bytes();
        tx.encode()
    };
    let nonce = |core: &Core, seed: u8| {
        core.state()
            .accounts
            .iter()
            .find(|a| a.key == pk(seed))
            .unwrap()
            .next_nonce
    };
    let order = |side, lots| {
        TxBody::PlaceOrder(PlaceOrder {
            market_id: 1,
            side,
            tif: Tif::Gtc,
            reduce_only: false,
            price: 65_000_000,
            lots,
            client_order_id: 0,
        })
    };
    let (na, nb) = (nonce(&core, A), nonce(&core, B));
    core.submit_tx(&tx(&core, A, na, order(Side::Buy, 10), now), now)
        .unwrap();
    core.submit_tx(&tx(&core, B, nb, order(Side::Sell, 4), now), now)
        .unwrap();
    core.submit_tx(
        &tx(
            &core,
            A,
            na + 1,
            TxBody::Withdraw { amount: 100 * USDC },
            now,
        ),
        now,
    )
    .unwrap();
    while core.height() < 25 {
        if core.height().is_multiple_of(5) {
            oracle(&mut core, now);
        }
        core.produce_block(now).unwrap();
        now += 1000;
    }
    (core, wasm_hash)
}

/// What Stellar would hold after the relayer submitted checkpoints 1..=n.
struct FakeStellar {
    config: OnChainConfig,
    checkpoints: Vec<(Vec<u8>, Vec<u8>)>,
    claimed: Mutex<BTreeSet<(u64, u32)>>,
}

impl FakeStellar {
    fn from(core: &Core, wasm_hash: [u8; 32], n: u64) -> Self {
        let (_, config_bytes, genesis) = caravel_node::lane_toml::genesis(&lane()).unwrap();
        let checkpoints = (1..=n)
            .map(|s| {
                let row = core.store().checkpoint(s).unwrap().unwrap();
                (row.header, row.batch)
            })
            .collect();
        Self {
            config: OnChainConfig {
                lane_id: lane().lane_id(),
                engine_wasm_hash: wasm_hash,
                genesis_state_hash: sha256(&genesis),
                config_hash: sha256(&config_bytes),
            },
            checkpoints,
            claimed: Mutex::new(BTreeSet::new()),
        }
    }
}

impl ReplaySource for FakeStellar {
    async fn config(&self) -> Result<OnChainConfig> {
        Ok(self.config.clone())
    }

    async fn last_checkpoint(&self) -> Result<LastCheckpoint> {
        let n = self.checkpoints.len() as u64;
        Ok(LastCheckpoint {
            seq: n,
            header_hash: self.checkpoints.last().map_or([0; 32], |(h, _)| sha256(h)),
        })
    }

    async fn checkpoint(&self, seq: u64) -> Result<Option<CheckpointRecord>> {
        Ok(self
            .checkpoints
            .get(seq as usize - 1)
            .map(|(h, _)| CheckpointRecord {
                header_hash: sha256(h),
                withdrawal_count: CheckpointHeaderV1::decode(h).unwrap().withdrawal_count,
                stellar_ledger: 100 + seq as u32,
            }))
    }

    async fn checkpoint_args(
        &self,
        seq: u64,
        _record: &CheckpointRecord,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        Ok(self.checkpoints[seq as usize - 1].clone())
    }

    async fn is_claimed(&self, seq: u64, index: u32) -> Result<bool> {
        Ok(self.claimed.lock().unwrap().contains(&(seq, index)))
    }
}

async fn run<S: ReplaySource>(src: &S, wasm_hash: [u8; 32]) -> Result<replay::Outcome> {
    let (exec, _) = wasm();
    replay::replay(
        src,
        &lane(),
        &Executor::Wasm(exec),
        wasm_hash,
        &ids(wasm_hash),
    )
    .await
}

#[tokio::test]
async fn replay_reproduces_every_checkpoint_through_the_wasm() {
    let (core, wasm_hash) = produced();
    let src = FakeStellar::from(&core, wasm_hash, 2);
    let outcome = run(&src, wasm_hash).await.unwrap();
    let r = replay::report(&outcome);
    assert_eq!((r.checkpoints, r.final_height), (2, 20));
    let (_, snapshot) = core.store().snapshot(2).unwrap().unwrap();
    assert_eq!(outcome.final_state, snapshot);
    assert!(r.summary.starts_with("OK seq=1..2 final_state_hash="));
}

#[tokio::test]
async fn replay_finds_tampering() {
    let (core, wasm_hash) = produced();
    let expect = |e: anyhow::Error, what: &str| {
        assert!(
            format!("{e:#}").contains(what),
            "{e:#} should mention {what}"
        )
    };

    // The transaction's header is not the one Stellar recorded.
    let mut src = FakeStellar::from(&core, wasm_hash, 2);
    let fake = FakeStellar::from(&core, wasm_hash, 2);
    src.checkpoints[1].0[100] ^= 1;
    let record_hash = sha256(&fake.checkpoints[1].0);
    assert_ne!(sha256(&src.checkpoints[1].0), record_hash);
    let e = run(
        &TamperedRecord {
            inner: src,
            seq: 2,
            header_hash: record_hash,
        },
        wasm_hash,
    )
    .await
    .err()
    .unwrap();
    expect(
        e,
        "checkpoint 2: H(header) from the transaction is not Ckpt(seq).header_hash",
    );

    // A batch that does not hash to header.batch_hash.
    let mut src = FakeStellar::from(&core, wasm_hash, 2);
    let last = src.checkpoints[0].1.len() - 1;
    src.checkpoints[0].1[last] ^= 1;
    expect(
        run(&src, wasm_hash).await.err().unwrap(),
        "checkpoint 1: H(batch) is not header.batch_hash",
    );

    // A consistent forgery: a block's state_hash_after changed, and the
    // batch hash and header re-signed around it. Re-execution catches it.
    let mut src = FakeStellar::from(&core, wasm_hash, 2);
    let mut batch = BatchV1::decode(&src.checkpoints[0].1).unwrap();
    batch.blocks[3].state_hash_after[0] ^= 1;
    let batch_bytes = batch.encode().unwrap();
    let mut header = CheckpointHeaderV1::decode(&src.checkpoints[0].0).unwrap();
    header.batch_hash = sha256(&batch_bytes);
    src.checkpoints[0] = (header.encode().to_vec(), batch_bytes);
    expect(
        run(&src, wasm_hash).await.err().unwrap(),
        "block 4: state_hash_after differs from re-execution",
    );

    // A header field that re-execution does not give.
    let mut src = FakeStellar::from(&core, wasm_hash, 1);
    let mut header = CheckpointHeaderV1::decode(&src.checkpoints[0].0).unwrap();
    header.escape_total += 1;
    src.checkpoints[0].0 = header.encode().to_vec();
    expect(
        run(&src, wasm_hash).await.err().unwrap(),
        "header field escape_total",
    );

    // A contract configured for another genesis or engine.
    let mut src = FakeStellar::from(&core, wasm_hash, 1);
    src.config.config_hash[0] ^= 1;
    expect(run(&src, wasm_hash).await.err().unwrap(), "config_hash");
    let mut src = FakeStellar::from(&core, wasm_hash, 1);
    src.config.engine_wasm_hash[0] ^= 1;
    expect(
        run(&src, wasm_hash).await.err().unwrap(),
        "engine_wasm_hash",
    );
}

/// Reports a chosen header hash for one seq, as if the transaction found were another one.
struct TamperedRecord {
    inner: FakeStellar,
    seq: u64,
    header_hash: [u8; 32],
}

impl ReplaySource for TamperedRecord {
    async fn config(&self) -> Result<OnChainConfig> {
        self.inner.config().await
    }
    async fn last_checkpoint(&self) -> Result<LastCheckpoint> {
        Ok(LastCheckpoint {
            seq: self.inner.checkpoints.len() as u64,
            header_hash: self.header_hash,
        })
    }
    async fn checkpoint(&self, seq: u64) -> Result<Option<CheckpointRecord>> {
        let mut r = self.inner.checkpoint(seq).await?;
        if seq == self.seq {
            if let Some(r) = r.as_mut() {
                r.header_hash = self.header_hash;
            }
        }
        Ok(r)
    }
    async fn checkpoint_args(
        &self,
        seq: u64,
        record: &CheckpointRecord,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        self.inner.checkpoint_args(seq, record).await
    }
    async fn is_claimed(&self, seq: u64, index: u32) -> Result<bool> {
        self.inner.is_claimed(seq, index).await
    }
}

#[tokio::test]
async fn replay_proofs_match_the_sequencer() {
    let (mut core, wasm_hash) = produced();
    let src = FakeStellar::from(&core, wasm_hash, 2);
    let outcome = run(&src, wasm_hash).await.unwrap();
    // The sequencer, told both checkpoints were accepted, serves the same escape leaf.
    for seq in [1, 2] {
        let row = core.store().checkpoint(seq).unwrap().unwrap();
        assert_eq!(row.status, CheckpointStatus::Sequenced);
        core.store_mut().set_signed(seq, 1, "[]").unwrap();
        core.store_mut().set_accepted(seq, "00", 1).unwrap();
    }
    let ours = replay::escape_proof(&outcome, &pk(B)).unwrap();
    let theirs = caravel_node::api::escape_proof(core.store(), &pk(B), Some(2)).unwrap();
    let body = axum::body::to_bytes(theirs.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        ours,
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()
    );

    // A's withdrawal is unclaimed, then claimed.
    let w = replay::withdrawal_proofs(&src, &outcome, &pk(A))
        .await
        .unwrap();
    let leaves = w["withdrawals"].as_array().unwrap();
    assert_eq!(leaves.len(), 1);
    assert_eq!(leaves[0]["amount"], (100 * USDC).to_string());
    let seq: u64 = leaves[0]["seq"].as_str().unwrap().parse().unwrap();
    let index = leaves[0]["index"].as_u64().unwrap() as u32;
    let header = CheckpointHeaderV1::decode(&src.checkpoints[seq as usize - 1].0).unwrap();
    let proof: Vec<[u8; 32]> = leaves[0]["proof"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| unhex(p.as_str().unwrap()).unwrap().try_into().unwrap())
        .collect();
    let leaf = sha256(&caravel_types::preimage::withdrawal_leaf_preimage(
        &header.lane_id,
        seq,
        index,
        &pk(A),
        100 * USDC,
    ));
    assert!(caravel_merkle::verify(
        &caravel_merkle::NativeSha256,
        &leaf,
        index,
        header.withdrawal_count,
        &proof,
        &header.withdrawals_root
    ));
    src.claimed.lock().unwrap().insert((seq, index));
    let w = replay::withdrawal_proofs(&src, &outcome, &pk(A))
        .await
        .unwrap();
    assert_eq!(w["withdrawals"].as_array().unwrap().len(), 0);
}

#[test]
fn submit_checkpoint_args_come_out_of_the_envelope() {
    use stellar_xdr::*;
    let call = InvokeContractArgs {
        contract_address: ScAddress::Contract(ContractId(Hash([7; 32]))),
        function_name: ScSymbol("submit_checkpoint".try_into().unwrap()),
        args: vec![
            ScVal::Bytes(ScBytes(vec![1u8; 442].try_into().unwrap())),
            ScVal::Bytes(ScBytes(vec![2u8; 900].try_into().unwrap())),
            ScVal::U64(1),
            ScVal::Vec(Some(ScVec(vec![].try_into().unwrap()))),
        ]
        .try_into()
        .unwrap(),
    };
    let op = Operation {
        source_account: None,
        body: OperationBody::InvokeHostFunction(InvokeHostFunctionOp {
            host_function: HostFunction::InvokeContract(call),
            auth: vec![].try_into().unwrap(),
        }),
    };
    let tx = Transaction {
        source_account: MuxedAccount::Ed25519(Uint256([3; 32])),
        fee: 100,
        seq_num: SequenceNumber(1),
        cond: Preconditions::None,
        memo: Memo::None,
        operations: vec![op].try_into().unwrap(),
        ext: TransactionExt::V0,
    };
    let env = TransactionEnvelope::Tx(TransactionV1Envelope {
        tx,
        signatures: vec![].try_into().unwrap(),
    });
    // Through base64, as getTransaction returns it.
    let env = TransactionEnvelope::from_xdr_base64(
        env.to_xdr_base64(Limits::none()).unwrap(),
        Limits::none(),
    )
    .unwrap();
    let (header, batch) = replay::submit_checkpoint_args(&env).unwrap();
    assert_eq!((header, batch), (vec![1u8; 442], vec![2u8; 900]));
}
