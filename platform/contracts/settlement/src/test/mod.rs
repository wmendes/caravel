//! Settlement contract tests (spec §13.7). A harness runs the real engine on a
//! native lane, feeds it the inbox messages the contract recorded, and builds
//! checkpoint headers and batches from the lane, the way the sequencer does.

mod checkpoint;
mod claims;
mod freeze;
mod misc;
mod rotation;

extern crate std;

use std::vec::Vec as StdVec;

use caravel_core::batch::BatchV1;
use caravel_core::block::BlockInputV1;
use caravel_core::checkpoint::CheckpointHeaderV1;
use caravel_core::inbox::{InboxKind, InboxMsgV1};
use caravel_core::merkle::{MerkleTree, NativeSha256};
use caravel_core::preimage::{account_leaf_preimage, withdrawal_leaf_preimage};
use caravel_harness::sha256;
use caravel_harness::Lane;
use caravel_harness::{config, T0};
use ed25519_dalek::{Signer, SigningKey};
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::token::{StellarAssetClient, TokenClient};
use soroban_sdk::xdr::{self, ScAddress, ToXdr};
use soroban_sdk::{Address, Bytes, BytesN, Env, Vec};

use crate::types::*;
use crate::{Settlement, SettlementClient, MAX_BATCH_BYTES};

pub const USDC: i128 = 10_000_000;
/// The engine hash the tests configure.
pub const ENGINE_HASH: [u8; 32] = [0xEE; 32];
/// Ledger time when the tests start: well after the lane's first block (T0).
pub const START: u64 = T0 / 1000 + 10;
pub const PARAMS: Params = Params {
    force_inclusion_window_secs: 600,
    escape_timeout_secs: 1200,
    min_rotation_delay_secs: 3600,
    signer_retention_epochs: 2,
    min_deposit: USDC,
};
/// Lane users (key seeds).
pub const A: u8 = 0x41;
pub const B: u8 = 0x42;
pub const C: u8 = 0x43;

/// The settlement Wasm of record, built by `scripts/build-contracts.sh`.
pub fn settlement_wasm() -> StdVec<u8> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../target/contracts/settlement.wasm"
    );
    std::fs::read(path)
        .unwrap_or_else(|_| panic!("{path} is missing: run ./scripts/build-contracts.sh first"))
}

pub fn key32(env: &Env, seed: u8) -> BytesN<32> {
    BytesN::from_array(env, &caravel_harness::pk(seed))
}

pub fn contract_error(e: Error) -> soroban_sdk::Error {
    soroban_sdk::Error::from_contract_error(e as u32)
}

/// The contract error of a `try_` call, if it failed with one.
pub fn code_of<T>(
    r: Result<T, Result<soroban_sdk::Error, soroban_sdk::InvokeError>>,
) -> Option<soroban_sdk::Error> {
    match r {
        Err(Ok(e)) if e.is_type(xdr::ScErrorType::Contract) => Some(e),
        _ => None,
    }
}

pub fn expect_err<T: core::fmt::Debug>(
    r: Result<T, Result<soroban_sdk::Error, soroban_sdk::InvokeError>>,
    want: Error,
) {
    let got = code_of(r);
    assert_eq!(got, Some(contract_error(want)), "expected {want:?}");
}

/// A validator key from a fixed seed.
pub fn validator(i: u8) -> SigningKey {
    SigningKey::from_bytes(&[0x61 + i; 32])
}

pub fn raw_pk(sk: &SigningKey) -> [u8; 32] {
    sk.verifying_key().to_bytes()
}

/// A signer set of `keys`, sorted by key, each weight 1.
pub fn signer_set(
    env: &Env,
    keys: &[SigningKey],
    threshold: u32,
) -> (WeightedSigners, StdVec<SigningKey>) {
    let mut sorted: StdVec<SigningKey> = keys.to_vec();
    sorted.sort_by_key(raw_pk);
    let mut signers = Vec::new(env);
    for k in &sorted {
        signers.push_back(WeightedSigner {
            key: BytesN::from_array(env, &raw_pk(k)),
            weight: 1,
        });
    }
    (WeightedSigners { signers, threshold }, sorted)
}

/// The `G...` address of a raw ed25519 key.
pub fn account_address(env: &Env, key: &[u8; 32]) -> Address {
    Address::from_str(env, &stellar_strkey::ed25519::PublicKey(*key).to_string())
}

fn add_entry(env: &Env, key: xdr::LedgerKey, data: xdr::LedgerEntryData) {
    let entry = xdr::LedgerEntry {
        data,
        last_modified_ledger_seq: 0,
        ext: xdr::LedgerEntryExt::V0,
    };
    env.host()
        .add_ledger_entry(&std::rc::Rc::new(key), &std::rc::Rc::new(entry), None)
        .unwrap();
}

/// Creates the Stellar account and a USDC trustline for it (written for these
/// tests; SAC balances of `G...` accounts live in trustlines).
pub fn create_account_with_trustline(env: &Env, who: &Address, asset: &xdr::Asset) {
    let ScAddress::Account(account_id) = ScAddress::from(who) else {
        panic!("not an account address")
    };
    add_entry(
        env,
        xdr::LedgerKey::Account(xdr::LedgerKeyAccount {
            account_id: account_id.clone(),
        }),
        xdr::LedgerEntryData::Account(xdr::AccountEntry {
            account_id: account_id.clone(),
            balance: 0,
            flags: 0,
            home_domain: Default::default(),
            inflation_dest: None,
            num_sub_entries: 0,
            seq_num: xdr::SequenceNumber(0),
            thresholds: xdr::Thresholds([1; 4]),
            signers: xdr::VecM::default(),
            ext: xdr::AccountEntryExt::V0,
        }),
    );
    let tl_asset = match asset.clone() {
        xdr::Asset::Native => xdr::TrustLineAsset::Native,
        xdr::Asset::CreditAlphanum4(a) => xdr::TrustLineAsset::CreditAlphanum4(a),
        xdr::Asset::CreditAlphanum12(a) => xdr::TrustLineAsset::CreditAlphanum12(a),
    };
    add_entry(
        env,
        xdr::LedgerKey::Trustline(xdr::LedgerKeyTrustLine {
            account_id: account_id.clone(),
            asset: tl_asset.clone(),
        }),
        xdr::LedgerEntryData::Trustline(xdr::TrustLineEntry {
            account_id,
            asset: tl_asset,
            balance: 0,
            limit: i64::MAX,
            flags: xdr::TrustLineFlags::AuthorizedFlag as u32,
            ext: xdr::TrustLineEntryExt::V0,
        }),
    );
}

/// A header and batch ready to submit, plus the leaves they commit to.
#[derive(Clone)]
pub struct Checkpoint {
    pub header: CheckpointHeaderV1,
    pub batch: StdVec<u8>,
    /// `(key, amount)` withdrawal leaves in order.
    pub withdrawals: StdVec<([u8; 32], i128)>,
    /// `(key, escape equity)` account leaves in order.
    pub accounts: StdVec<([u8; 32], i128)>,
}

/// Signatures over `H(msg)` by `keys[i]` for each `i` in `which`.
pub fn sign_with(env: &Env, keys: &[SigningKey], msg: &[u8], which: &[u32]) -> Vec<Sig> {
    let hash = sha256(msg);
    let mut sigs = Vec::new(env);
    for i in which {
        let sig = keys[*i as usize].sign(&hash).to_bytes();
        sigs.push_back(Sig {
            signer_index: *i,
            signature: BytesN::from_array(env, &sig),
        });
    }
    sigs
}

pub struct Harness {
    pub env: Env,
    pub id: Address,
    pub usdc: Address,
    pub asset: xdr::Asset,
    pub admin: Address,
    pub validators: StdVec<SigningKey>,
    pub lane: Lane,
    pub last_header_hash: [u8; 32],
    /// Index of the first lane record not yet in an accepted batch.
    pub next_record: usize,
    /// Contract inbox messages already pushed to the lane.
    pub inbox_synced: u64,
    /// Seeds whose Stellar account and trustline exist.
    pub accounts: std::cell::RefCell<std::collections::BTreeSet<u8>>,
}

impl Harness {
    /// The contract registered natively (fast; no Wasm costs).
    pub fn new() -> Self {
        Self::build(false)
    }

    /// The contract registered from the Wasm of record, so costs include the VM.
    pub fn new_wasm() -> Self {
        Self::build(true)
    }

    fn build(wasm: bool) -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.cost_estimate().budget().reset_unlimited();
        env.ledger().with_mut(|l| {
            l.timestamp = START;
            l.sequence_number = 1_000;
            l.network_id = sha256(b"Test SDF Network ; September 2015");
        });
        let issuer = Address::generate(&env);
        let sac = env.register_stellar_asset_contract_v2(issuer);
        let admin = Address::generate(&env);
        let keys: StdVec<SigningKey> = (0..3).map(validator).collect();
        let (signers, validators) = signer_set(&env, &keys, 2);
        let cfg = config();
        let lane = Lane::native(cfg.clone());
        let args = (
            admin.clone(),
            sac.address(),
            BytesN::from_array(&env, &cfg.lane_id),
            BytesN::from_array(&env, &ENGINE_HASH),
            BytesN::from_array(&env, &lane.state_hash()),
            BytesN::from_array(&env, &lane.config_hash),
            signers,
            PARAMS,
        );
        let id = if wasm {
            env.register(settlement_wasm().as_slice(), args)
        } else {
            env.register(Settlement, args)
        };
        Self {
            usdc: sac.address(),
            asset: sac.asset(),
            env,
            id,
            admin,
            validators,
            lane,
            last_header_hash: [0; 32],
            next_record: 0,
            inbox_synced: 0,
            accounts: Default::default(),
        }
    }

    pub fn c(&self) -> SettlementClient<'_> {
        SettlementClient::new(&self.env, &self.id)
    }

    pub fn token(&self) -> TokenClient<'_> {
        TokenClient::new(&self.env, &self.usdc)
    }

    /// The `G...` address of lane user `seed`, with an account, a trustline and `usdc` minted.
    pub fn user(&self, seed: u8, usdc: i128) -> Address {
        let who = account_address(&self.env, &caravel_harness::pk(seed));
        if self.accounts.borrow_mut().insert(seed) {
            create_account_with_trustline(&self.env, &who, &self.asset);
        }
        if usdc > 0 {
            StellarAssetClient::new(&self.env, &self.usdc).mint(&who, &usdc);
        }
        who
    }

    pub fn advance(&self, secs: u64) {
        self.env.ledger().with_mut(|l| {
            l.timestamp += secs;
            l.sequence_number += (secs / 5) as u32;
        });
    }

    /// Deposits on Stellar into the lane account of the same key.
    pub fn deposit(&mut self, seed: u8, amount: i128) -> u64 {
        let who = self.user(seed, amount);
        self.c().deposit(
            &who,
            &amount,
            &BytesN::from_array(&self.env, &caravel_harness::pk(seed)),
        )
    }

    /// Pushes every inbox message the contract recorded to the lane, as the relayer does.
    pub fn sync_inbox(&mut self) {
        let count = self.c().inbox_count();
        while self.inbox_synced < count {
            let m = self.c().inbox(&self.inbox_synced).expect("recorded");
            let kind = if m.kind == 0 {
                InboxKind::Deposit
            } else {
                InboxKind::ForcedWithdrawal
            };
            self.lane.push_inbox(InboxMsgV1 {
                kind,
                index: self.inbox_synced,
                lane_account: m.lane_account.to_array(),
                amount: m.amount,
                enqueued_at: m.enqueued_at,
            });
            self.inbox_synced += 1;
        }
    }

    /// Runs an empty CHECKPOINT_END block and builds its header and batch from
    /// the lane (spec §14.3), with the pending list the commitment covers.
    pub fn checkpoint(&mut self) -> Checkpoint {
        let pending: StdVec<([u8; 32], i128)> = self
            .lane
            .state()
            .pending
            .iter()
            .map(|p| (p.key, p.amount))
            .collect();
        self.lane.checkpoint();
        self.parts(pending)
    }

    pub fn parts(&self, withdrawals: StdVec<([u8; 32], i128)>) -> Checkpoint {
        let st = self.lane.state();
        let c = st.last_commitment;
        let records = self.lane.records[self.next_record..].to_vec();
        let last_ts = BlockInputV1::decode(&records.last().unwrap().input)
            .unwrap()
            .timestamp_ms;
        let batch = BatchV1 {
            lane_id: st.lane_id,
            checkpoint_seq: c.seq,
            blocks: records,
        }
        .encode()
        .unwrap();
        let accounts = self.lane.escape_leaves();
        let header = CheckpointHeaderV1 {
            lane_id: st.lane_id,
            network_id: self.env.ledger().network_id().to_array(),
            settlement_addr_hash: sha256(&self.id.clone().to_xdr(&self.env).to_alloc_vec()),
            engine_wasm_hash: ENGINE_HASH,
            seq: c.seq,
            prev_header_hash: self.last_header_hash,
            first_block_height: self.next_record as u64 + 1,
            last_block_height: self.lane.records.len() as u64,
            last_block_timestamp_ms: last_ts,
            last_block_hash: self.lane.prev_block_hash,
            batch_hash: sha256(&batch),
            state_hash: self.lane.state_hash(),
            accounts_root: c.accounts_root,
            account_count: c.account_count,
            escape_total: c.escape_total,
            withdrawals_root: c.withdrawals_root,
            withdrawal_count: c.withdrawal_count,
            withdrawals_total: c.withdrawals_total,
            inbox_through: c.inbox_through,
            inbox_acc: c.inbox_acc,
        };
        Checkpoint {
            header,
            batch,
            withdrawals,
            accounts,
        }
    }

    /// Signatures by validators `which` (indexes into the sorted set).
    pub fn sign(&self, header: &CheckpointHeaderV1, which: &[u32]) -> Vec<Sig> {
        self.sign_bytes(&header.encode(), which)
    }

    pub fn sign_bytes(&self, header: &[u8], which: &[u32]) -> Vec<Sig> {
        sign_with(&self.env, &self.validators, header, which)
    }

    pub fn try_submit(
        &self,
        cp: &Checkpoint,
        epoch: u64,
        sigs: &Vec<Sig>,
    ) -> Result<
        Result<(), soroban_sdk::ConversionError>,
        Result<soroban_sdk::Error, soroban_sdk::InvokeError>,
    > {
        let header = Bytes::from_slice(&self.env, &cp.header.encode());
        self.c().try_submit_checkpoint(
            &header,
            &Bytes::from_slice(&self.env, &cp.batch),
            &epoch,
            sigs,
        )
    }

    /// Submits with 2 of 3 signatures at epoch 1 and records it as accepted.
    pub fn accept(&mut self, cp: &Checkpoint) {
        let sigs = self.sign(&cp.header, &[0, 2]);
        self.accept_with(cp, 1, &sigs);
    }

    pub fn accept_with(&mut self, cp: &Checkpoint, epoch: u64, sigs: &Vec<Sig>) {
        self.try_submit(cp, epoch, sigs)
            .expect("checkpoint accepted")
            .unwrap();
        self.last_header_hash = sha256(&cp.header.encode());
        self.next_record = self.lane.records.len();
    }

    /// The Merkle proof for withdrawal leaf `index` of `cp`.
    pub fn withdrawal_proof(&self, cp: &Checkpoint, index: u32) -> Vec<BytesN<32>> {
        let leaves: StdVec<[u8; 32]> = cp
            .withdrawals
            .iter()
            .enumerate()
            .map(|(i, (k, a))| {
                sha256(&withdrawal_leaf_preimage(
                    &cp.header.lane_id,
                    cp.header.seq,
                    i as u32,
                    k,
                    *a,
                ))
            })
            .collect();
        self.proof_of(&leaves, index)
    }

    pub fn account_proof(&self, cp: &Checkpoint, index: u32) -> Vec<BytesN<32>> {
        let leaves: StdVec<[u8; 32]> = cp
            .accounts
            .iter()
            .enumerate()
            .map(|(j, (k, e))| {
                sha256(&account_leaf_preimage(
                    &cp.header.lane_id,
                    cp.header.seq,
                    j as u32,
                    k,
                    *e,
                ))
            })
            .collect();
        self.proof_of(&leaves, index)
    }

    fn proof_of(&self, leaves: &[[u8; 32]], index: u32) -> Vec<BytesN<32>> {
        let tree = MerkleTree::build(&NativeSha256, leaves).unwrap();
        let mut out = Vec::new(&self.env);
        for s in tree.proof(index).unwrap() {
            out.push_back(BytesN::from_array(&self.env, &s));
        }
        out
    }

    /// A checkpoint with chosen leaves, for solvency, freeze and escape tests.
    /// The contract checks the signatures and the chain and inbox fields, not
    /// the leaves, so any leaves are accepted when validators sign them. The
    /// inbox fields are the lane's: every message it has executed.
    pub fn crafted(
        &mut self,
        accounts: StdVec<([u8; 32], i128)>,
        withdrawals: StdVec<([u8; 32], i128)>,
    ) -> Checkpoint {
        if self.next_record == self.lane.records.len() {
            self.lane.block();
        }
        let mut cp = self.parts(withdrawals.clone());
        let seq = self.c().last_checkpoint().seq + 1;
        cp.header.seq = seq;
        let st = self.lane.state();
        cp.header.inbox_through = st.inbox_through;
        cp.header.inbox_acc = st.inbox_acc;
        let acc: StdVec<[u8; 32]> = accounts
            .iter()
            .enumerate()
            .map(|(j, (k, e))| {
                sha256(&account_leaf_preimage(
                    &cp.header.lane_id,
                    seq,
                    j as u32,
                    k,
                    *e,
                ))
            })
            .collect();
        let wdl: StdVec<[u8; 32]> = withdrawals
            .iter()
            .enumerate()
            .map(|(i, (k, a))| {
                sha256(&withdrawal_leaf_preimage(
                    &cp.header.lane_id,
                    seq,
                    i as u32,
                    k,
                    *a,
                ))
            })
            .collect();
        cp.header.accounts_root = caravel_core::merkle::root(&NativeSha256, &acc).unwrap();
        cp.header.account_count = accounts.len() as u32;
        cp.header.escape_total = accounts.iter().map(|(_, e)| *e).sum();
        cp.header.withdrawals_root = caravel_core::merkle::root(&NativeSha256, &wdl).unwrap();
        cp.header.withdrawal_count = withdrawals.len() as u32;
        cp.header.withdrawals_total = withdrawals.iter().map(|(_, a)| *a).sum();
        cp.accounts = accounts;
        cp.withdrawals = withdrawals;
        cp
    }
}
