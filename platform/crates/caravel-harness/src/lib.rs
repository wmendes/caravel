//! Caravel test harness (M0.5 P-09, test-only): a lane on the app SDK,
//! executed natively block by block, the way the sequencer builds blocks. The
//! platform's own tests use it instead of a particular lane's engine, so they
//! depend on no lane (`scripts/check-deps.mjs`). It runs the SDK's test app
//! by default; any `AppEngine` works.

use caravel_app_sdk::crypto::native::DiagnosticCrypto;
use caravel_app_sdk::testapp::TestApp;
use caravel_app_sdk::{
    conserved, genesis, step, AccessMode, AppEngine, AppGenesisV1, Fatal, SdkState,
};
use caravel_core::block::{block_hash_preimage, BlockInputV1, BlockRecordV1, Entry};
use caravel_core::inbox::{InboxKind, InboxMsgV1};
use caravel_core::preimage::lane_id_preimage;
use caravel_core::receipts::{ReceiptsV1, PSEUDO_BLOCK_START};
use caravel_core::tx::{sep53_preimage, sep53_tx_message, SigScheme, StandardBody, TxEnvelopeV1};
use ed25519_dalek::{Signer, SigningKey};
use sha2::Digest;

/// The first block's timestamp.
pub const T0: u64 = 1_790_000_000_000;
pub const BLOCK_MS: u64 = 1_000;
pub const USDC: i128 = 10_000_000;

pub fn sha256(data: &[u8]) -> [u8; 32] {
    sha2::Sha256::digest(data).into()
}

/// A deterministic test key: the ed25519 key with seed `[n; 32]`.
pub fn key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

pub fn pk(n: u8) -> [u8; 32] {
    key(n).verifying_key().to_bytes()
}

/// The harness lane's name.
pub const LANE_NAME: &str = "caravel-harness-0";

/// The test app's config: open access, two system accounts (seeds 0x21, 0x22).
pub fn config() -> AppGenesisV1 {
    AppGenesisV1 {
        lane_id: sha256(&lane_id_preimage(LANE_NAME)),
        template: TestApp::TEMPLATE,
        template_version: 1,
        system_keys: vec![pk(0x21), pk(0x22)],
        access_mode: AccessMode::Open,
        allowlist: vec![],
        min_deposit: USDC,
        min_withdrawal: USDC,
        max_accounts: 256,
        max_session_keys: 4,
        max_txs_per_account_per_block: 50,
        max_entries_per_block: 256,
        max_block_bytes: 12_000,
        max_pending_withdrawals: 512,
        exec_cpu_limit: 200_000_000,
        exec_mem_limit: 41_943_040,
        app_params: vec![],
    }
}

pub struct Lane<E: AppEngine = TestApp> {
    pub config: AppGenesisV1,
    pub config_hash: [u8; 32],
    pub state_bytes: Vec<u8>,
    /// The next block's timestamp.
    pub now: u64,
    pub prev_block_hash: [u8; 32],
    next_inbox: u64,
    queued_inbox: Vec<InboxMsgV1>,
    queued_user: Vec<TxEnvelopeV1>,
    /// Receipts of the last executed block.
    pub receipts: ReceiptsV1,
    /// Every executed block, in order.
    pub records: Vec<BlockRecordV1>,
    /// What the app itself holds, for SDK-INV1 after every block (the test app: nothing).
    pub app_held: fn(&SdkState) -> i128,
    _app: core::marker::PhantomData<E>,
}

impl Lane<TestApp> {
    pub fn native(config: AppGenesisV1) -> Self {
        Self::new(config)
    }
}

impl<E: AppEngine> Lane<E> {
    pub fn new(config: AppGenesisV1) -> Self {
        let bytes = config.encode().expect("config encodes");
        let state_bytes = genesis::<E>(&bytes, &DiagnosticCrypto).expect("genesis succeeds");
        Self {
            config_hash: sha256(&bytes),
            config,
            state_bytes,
            now: T0,
            prev_block_hash: [0; 32],
            next_inbox: 0,
            queued_inbox: Vec::new(),
            queued_user: Vec::new(),
            receipts: ReceiptsV1::default(),
            records: Vec::new(),
            app_held: |_| 0,
            _app: core::marker::PhantomData,
        }
    }

    pub fn state(&self) -> SdkState {
        SdkState::decode(&self.state_bytes, &E::STATE_MAGIC).expect("state decodes")
    }

    pub fn state_hash(&self) -> [u8; 32] {
        sha256(&self.state_bytes)
    }

    pub fn height(&self) -> u64 {
        self.records.len() as u64
    }

    pub fn advance(&mut self, ms: u64) {
        self.now += ms;
    }

    /// The balance of the account with key seed `seed` (0 if it has none).
    pub fn balance(&self, seed: u8) -> i128 {
        self.state()
            .accounts
            .iter()
            .find(|a| a.key == pk(seed))
            .map_or(0, |a| a.balance)
    }

    /// `(key, escape equity)` for every account, as the commitment's leaves.
    pub fn escape_leaves(&self) -> Vec<([u8; 32], i128)> {
        let st = self.state();
        let params = E::params(&st.config.app_params).expect("params decode");
        (0..st.accounts.len())
            .map(|j| {
                (
                    st.accounts[j].key,
                    E::escape_equity(&st, &params, j).expect("equity").max(0),
                )
            })
            .collect()
    }

    /// `(key, amount)` of the pending withdrawals the next commitment covers.
    pub fn pending(&self) -> Vec<([u8; 32], i128)> {
        self.state()
            .pending
            .iter()
            .map(|p| (p.key, p.amount))
            .collect()
    }

    // --- Entries ------------------------------------------------------------

    pub fn deposit(&mut self, seed: u8, amount: i128) -> InboxMsgV1 {
        self.inbox(InboxKind::Deposit, pk(seed), amount)
    }

    pub fn forced_withdrawal(&mut self, seed: u8, amount: i128) -> InboxMsgV1 {
        self.inbox(InboxKind::ForcedWithdrawal, pk(seed), amount)
    }

    fn inbox(&mut self, kind: InboxKind, key: [u8; 32], amount: i128) -> InboxMsgV1 {
        let m = InboxMsgV1 {
            kind,
            index: self.next_inbox,
            lane_account: key,
            amount,
            enqueued_at: self.now / 1000,
        };
        self.push_inbox(m);
        m
    }

    /// Queues a message recorded elsewhere (by the settlement contract).
    pub fn push_inbox(&mut self, msg: InboxMsgV1) {
        self.next_inbox = msg.index + 1;
        self.queued_inbox.push(msg);
    }

    /// The next nonce of `account_seed`, counting queued transactions.
    pub fn next_nonce(&self, account_seed: u8) -> u64 {
        let base = self
            .state()
            .accounts
            .iter()
            .find(|a| a.key == pk(account_seed))
            .map_or(0, |a| a.next_nonce);
        base + self
            .queued_user
            .iter()
            .filter(|t| t.account == pk(account_seed))
            .count() as u64
    }

    /// Queues a transaction for `account` signed by `signer`.
    pub fn tx_as(
        &mut self,
        signer: u8,
        account: u8,
        scheme: SigScheme,
        kind: u8,
        body: Vec<u8>,
    ) -> TxEnvelopeV1 {
        let mut tx = TxEnvelopeV1 {
            lane_id: self.config.lane_id,
            account: pk(account),
            signer: pk(signer),
            nonce: self.next_nonce(account),
            expiry_ms: self.now + 60_000,
            kind,
            sig_scheme: scheme,
            body,
            signature: [0; 64],
        };
        let hash = sha256(&tx.tx_hash_preimage(&self.config_hash));
        let msg = match scheme {
            SigScheme::RawEd25519 => hash.to_vec(),
            SigScheme::Sep53 => sha256(&sep53_preimage(&sep53_tx_message(&hash))).to_vec(),
        };
        tx.signature = key(signer).sign(&msg).to_bytes();
        self.queued_user.push(tx.clone());
        tx
    }

    /// Queues a standard transaction signed by the owner with SEP-53, as a wallet does.
    pub fn standard(&mut self, seed: u8, body: StandardBody) -> TxEnvelopeV1 {
        self.tx_as(seed, seed, SigScheme::Sep53, body.kind(), body.encode())
    }

    pub fn withdraw(&mut self, seed: u8, amount: i128) -> TxEnvelopeV1 {
        self.standard(seed, StandardBody::Withdraw { amount })
    }

    // --- Execution ------------------------------------------------------------

    /// Builds the next block from the queues (INBOX, then USER).
    pub fn build(&mut self, checkpoint_end: bool) -> BlockInputV1 {
        let mut entries: Vec<Entry> = self.queued_inbox.drain(..).map(Entry::Inbox).collect();
        entries.extend(self.queued_user.drain(..).map(Entry::User));
        BlockInputV1 {
            lane_id: self.config.lane_id,
            height: self.height() + 1,
            timestamp_ms: self.now,
            prev_block_hash: self.prev_block_hash,
            checkpoint_end,
            entries,
        }
    }

    /// Executes encoded block bytes. On success the lane advances and SDK-INV1
    /// is checked; on a fatal nothing changes.
    pub fn execute_bytes(&mut self, bytes: &[u8]) -> Result<&ReceiptsV1, Fatal> {
        let out = step::<E, _>(&self.state_bytes, bytes, &DiagnosticCrypto)?;
        let block = BlockInputV1::decode(bytes).expect("an executed block decodes");
        let state_hash_after = sha256(&out.state);
        self.prev_block_hash = sha256(&block_hash_preimage(&sha256(bytes), &state_hash_after));
        self.records.push(BlockRecordV1 {
            input: bytes.to_vec(),
            state_hash_after,
        });
        self.state_bytes = out.state;
        self.receipts = ReceiptsV1::decode(&out.receipts).expect("receipts decode");
        self.now = self.now.max(block.timestamp_ms) + BLOCK_MS;
        let st = self.state();
        assert!(
            conserved(&st, (self.app_held)(&st)),
            "SDK-INV1 broken at block {}",
            self.height()
        );
        Ok(&self.receipts)
    }

    pub fn try_block(&mut self, checkpoint_end: bool) -> Result<&ReceiptsV1, Fatal> {
        let b = self.build(checkpoint_end);
        let bytes = b.encode().expect("block encodes");
        self.execute_bytes(&bytes)
    }

    /// Executes the queued entries; panics on a fatal.
    pub fn block(&mut self) -> &ReceiptsV1 {
        self.run(false)
    }

    /// Executes the queued entries as a `CHECKPOINT_END` block; panics on a fatal.
    pub fn checkpoint(&mut self) -> &ReceiptsV1 {
        self.run(true)
    }

    fn run(&mut self, checkpoint_end: bool) -> &ReceiptsV1 {
        let height = self.height() + 1;
        match self.try_block(checkpoint_end) {
            Ok(r) => r,
            Err(f) => panic!("block {height} is fatal: {f:?}"),
        }
    }

    /// Asserts that every entry of the last block was accepted.
    pub fn all_ok(&self) {
        for r in &self.receipts.receipts {
            if r.entry_index < PSEUDO_BLOCK_START {
                assert_eq!(
                    r.code, 0,
                    "entry {} rejected with code {}",
                    r.entry_index, r.code
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lane_deposits_withdraws_and_checkpoints() {
        let mut lane = Lane::native(config());
        lane.deposit(0x41, 100 * USDC);
        lane.block();
        lane.all_ok();
        lane.withdraw(0x41, 30 * USDC);
        lane.block();
        lane.all_ok();
        assert_eq!(lane.pending(), vec![(pk(0x41), 30 * USDC)]);
        lane.checkpoint();
        let st = lane.state();
        assert_eq!(
            (st.checkpoint_seq, st.last_commitment.withdrawals_total),
            (1, 30 * USDC)
        );
        assert_eq!(lane.escape_leaves()[2], (pk(0x41), 70 * USDC));
        assert_eq!(lane.records.len(), 3);
        assert!(lane.pending().is_empty());
    }
}
