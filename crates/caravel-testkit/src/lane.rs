//! A lane simulator: builds signed blocks, executes them through an
//! [`Executor`], and checks the §11.9 invariants after every block.

use std::collections::BTreeMap;

use caravel_perps::native::DiagnosticCrypto;
use caravel_perps::{Fatal, StepOutput};
use caravel_types::block::{block_hash_preimage, BlockInputV1, BlockRecordV1, Entry};
use caravel_types::codes;
use caravel_types::config::GenesisConfigV1;
use caravel_types::inbox::{inbox_acc_preimage, InboxKind, InboxMsgV1};
use caravel_types::oracle::OracleUpdateV1;
use caravel_types::receipts::{Event, Receipt, Receipts};
use caravel_types::state::{AccountV1, StateV1};
use caravel_types::tx::{
    sep53_preimage, sep53_tx_message, LaneTxV1, PlaceOrder, Side, SigScheme, Tif, TxBody,
};
use caravel_types::vectors::{key, pk, sha256};
use ed25519_dalek::Signer;

/// Runs `genesis` and `step`. Native here; the Wasm executor (T-005) implements
/// the same trait so every scenario runs through both paths.
pub trait Executor {
    fn genesis(&self, config: &[u8]) -> Result<Vec<u8>, Fatal>;
    fn step(&self, state: &[u8], block: &[u8]) -> Result<StepOutput, Fatal>;
    /// Whether a fatal carries its entry index. The native diagnostic path does;
    /// the Wasm path loses it (spec §11).
    fn reports_entry_index(&self) -> bool;
}

/// The native engine with [`DiagnosticCrypto`], so a bad signature is
/// `Fatal { BAD_SIGNATURE, entry_index }` instead of a panic.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeExecutor;

impl Executor for NativeExecutor {
    fn genesis(&self, config: &[u8]) -> Result<Vec<u8>, Fatal> {
        caravel_perps::genesis(config, &DiagnosticCrypto)
    }

    fn step(&self, state: &[u8], block: &[u8]) -> Result<StepOutput, Fatal> {
        caravel_perps::step(state, block, &DiagnosticCrypto)
    }

    fn reports_entry_index(&self) -> bool {
        true
    }
}

/// Fixture key seeds (the key is ed25519 with seed `[n; 32]`).
pub mod seeds {
    pub const BACKSTOP: u8 = 0x21;
    pub const TREASURY: u8 = 0x22;
    pub const ORACLE: u8 = 0x31;
    pub const A: u8 = 0x41;
    pub const B: u8 = 0x42;
    pub const C: u8 = 0x43;
    pub const D: u8 = 0x44;
    pub const E: u8 = 0x45;
    pub const S: u8 = 0x51;
    pub const T: u8 = 0x52;
    pub const U: u8 = 0x53;
    pub const V: u8 = 0x54;
    pub const W: u8 = 0x55;
}

/// 1 USDC in stroops.
pub const USDC: i128 = 10_000_000;
pub const BTC: u16 = 1;
pub const ETH: u16 = 2;
pub const XLM: u16 = 3;
/// $65,000 per BTC is $6.50 per 0.0001-BTC lot.
pub const BTC_PRICE: i64 = 65_000_000;
/// $3,500 per ETH is $3.50 per 0.001-ETH lot.
pub const ETH_PRICE: i64 = 35_000_000;
/// $0.40 per XLM is $4.00 per 10-XLM lot.
pub const XLM_PRICE: i64 = 40_000_000;
pub const TICK: i64 = 1000;
/// First block time: 2026-09-21T13:46:40Z.
pub const T0: u64 = 1_790_000_000_000;
pub const BLOCK_MS: u64 = 1000;

/// The §10.3 lane with fixture keys.
pub fn config() -> GenesisConfigV1 {
    caravel_types::vectors::config()
}

pub fn market_index(config: &GenesisConfigV1, market_id: u16) -> usize {
    config
        .markets
        .iter()
        .position(|m| m.market_id == market_id)
        .expect("configured market")
}

pub fn sign(seed: u8, msg: &[u8]) -> [u8; 64] {
    key(seed).sign(msg).to_bytes()
}

pub struct Lane<E: Executor = NativeExecutor> {
    pub exec: E,
    pub config: GenesisConfigV1,
    pub config_hash: [u8; 32],
    pub state_bytes: Vec<u8>,
    /// The next block's timestamp.
    pub now: u64,
    pub prev_block_hash: [u8; 32],
    next_inbox: u64,
    /// INV-P4: the fold of every inbox message in an executed block.
    inbox_acc: [u8; 32],
    inbox_through: u64,
    queued_inbox: Vec<InboxMsgV1>,
    queued_oracle: Vec<OracleUpdateV1>,
    queued_user: Vec<LaneTxV1>,
    /// Receipts of the last executed block.
    pub receipts: Receipts,
    /// Every executed block, in order.
    pub records: Vec<BlockRecordV1>,
}

impl Lane<NativeExecutor> {
    pub fn native(config: GenesisConfigV1) -> Self {
        Self::new(NativeExecutor, config)
    }
}

impl<E: Executor> Lane<E> {
    pub fn new(exec: E, config: GenesisConfigV1) -> Self {
        let bytes = config.encode().expect("config encodes");
        let state_bytes = exec.genesis(&bytes).expect("genesis succeeds");
        Self {
            exec,
            config_hash: sha256(&bytes),
            config,
            state_bytes,
            now: T0,
            prev_block_hash: [0; 32],
            next_inbox: 0,
            inbox_acc: [0; 32],
            inbox_through: 0,
            queued_inbox: Vec::new(),
            queued_oracle: Vec::new(),
            queued_user: Vec::new(),
            receipts: Receipts::default(),
            records: Vec::new(),
        }
    }

    pub fn state(&self) -> StateV1 {
        StateV1::decode(&self.state_bytes).expect("engine state decodes")
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

    pub fn account(&self, seed: u8) -> Option<AccountV1> {
        self.state()
            .accounts
            .into_iter()
            .find(|a| a.key == pk(seed))
    }

    pub fn account_index(&self, seed: u8) -> Option<usize> {
        self.state().accounts.iter().position(|a| a.key == pk(seed))
    }

    pub fn collateral(&self, seed: u8) -> i128 {
        self.account(seed).map_or(0, |a| a.collateral)
    }

    /// `(lots, cost_basis)` of `seed` on `market_id`.
    pub fn position(&self, seed: u8, market_id: u16) -> (i64, i128) {
        let m = market_index(&self.config, market_id);
        self.account(seed)
            .map_or((0, 0), |a| (a.positions[m].lots, a.positions[m].cost_basis))
    }

    // --- Queueing -------------------------------------------------------------

    fn inbox(&mut self, kind: InboxKind, lane_account: [u8; 32], amount: i128) -> InboxMsgV1 {
        let msg = InboxMsgV1 {
            kind,
            index: self.next_inbox,
            lane_account,
            amount,
            enqueued_at: self.now / 1000,
        };
        self.next_inbox += 1;
        self.queued_inbox.push(msg);
        msg
    }

    pub fn deposit(&mut self, seed: u8, amount: i128) -> InboxMsgV1 {
        self.inbox(InboxKind::Deposit, pk(seed), amount)
    }

    pub fn deposit_key(&mut self, key: [u8; 32], amount: i128) -> InboxMsgV1 {
        self.inbox(InboxKind::Deposit, key, amount)
    }

    pub fn forced_withdrawal(&mut self, seed: u8, amount: i128) -> InboxMsgV1 {
        self.inbox(InboxKind::ForcedWithdrawal, pk(seed), amount)
    }

    /// A signed oracle update published at `publish_time_ms`.
    pub fn oracle_update(
        &self,
        oracle_seed: u8,
        market_id: u16,
        price: i64,
        publish_time_ms: u64,
    ) -> OracleUpdateV1 {
        let mut u = OracleUpdateV1 {
            market_id,
            price,
            publish_time_ms,
            oracle_key: pk(oracle_seed),
            signature: [0; 64],
        };
        u.signature = sign(
            oracle_seed,
            &sha256(&u.signing_preimage(&self.config.lane_id)),
        );
        u
    }

    /// Queues an oracle update published at the next block's time.
    pub fn oracle(&mut self, market_id: u16, price: i64) {
        let u = self.oracle_update(seeds::ORACLE, market_id, price, self.now);
        self.queued_oracle.push(u);
    }

    pub fn push_oracle(&mut self, u: OracleUpdateV1) {
        self.queued_oracle.push(u);
    }

    /// The nonce the next queued transaction of `seed` needs.
    pub fn next_nonce(&self, account_seed: u8) -> u64 {
        let queued = self
            .queued_user
            .iter()
            .filter(|t| t.account == pk(account_seed))
            .count() as u64;
        self.account(account_seed)
            .map_or(self.now, |a| a.next_nonce)
            + queued
    }

    /// A signed transaction (not queued).
    pub fn signed(
        &self,
        signer: u8,
        account: u8,
        scheme: SigScheme,
        nonce: u64,
        expiry_ms: u64,
        body: TxBody,
    ) -> LaneTxV1 {
        let mut tx = LaneTxV1 {
            lane_id: self.config.lane_id,
            account: pk(account),
            signer: pk(signer),
            nonce,
            expiry_ms,
            sig_scheme: scheme,
            body,
            signature: [0; 64],
        };
        let tx_hash = sha256(&tx.tx_hash_preimage(&self.config_hash));
        tx.signature = match scheme {
            SigScheme::RawEd25519 => sign(signer, &tx_hash),
            SigScheme::Sep53 => sign(
                signer,
                &sha256(&sep53_preimage(&sep53_tx_message(&tx_hash))),
            ),
        };
        tx
    }

    pub fn push_tx(&mut self, tx: LaneTxV1) -> LaneTxV1 {
        self.queued_user.push(tx);
        tx
    }

    /// Queues a transaction with the next nonce and a 60 s expiry.
    pub fn tx_as(&mut self, signer: u8, account: u8, scheme: SigScheme, body: TxBody) -> LaneTxV1 {
        let tx = self.signed(
            signer,
            account,
            scheme,
            self.next_nonce(account),
            self.now + 60_000,
            body,
        );
        self.push_tx(tx)
    }

    /// Queues a transaction signed by the owner (raw ed25519).
    pub fn tx(&mut self, seed: u8, body: TxBody) -> LaneTxV1 {
        self.tx_as(seed, seed, SigScheme::RawEd25519, body)
    }

    pub fn order(
        &mut self,
        seed: u8,
        market_id: u16,
        side: Side,
        tif: Tif,
        price: i64,
        lots: i64,
    ) -> LaneTxV1 {
        self.tx(
            seed,
            TxBody::PlaceOrder(PlaceOrder {
                market_id,
                side,
                tif,
                reduce_only: false,
                price,
                lots,
                client_order_id: 0,
            }),
        )
    }

    pub fn reduce_only(
        &mut self,
        seed: u8,
        market_id: u16,
        side: Side,
        price: i64,
        lots: i64,
    ) -> LaneTxV1 {
        self.tx(
            seed,
            TxBody::PlaceOrder(PlaceOrder {
                market_id,
                side,
                tif: Tif::Ioc,
                reduce_only: true,
                price,
                lots,
                client_order_id: 0,
            }),
        )
    }

    pub fn withdraw(&mut self, seed: u8, amount: i128) -> LaneTxV1 {
        self.tx_as(seed, seed, SigScheme::Sep53, TxBody::Withdraw { amount })
    }

    // --- Execution --------------------------------------------------------------

    /// Builds the next block from the queues (INBOX, then ORACLE, then USER).
    pub fn build(&mut self, checkpoint_end: bool) -> BlockInputV1 {
        let mut entries: Vec<Entry> = self.queued_inbox.drain(..).map(Entry::Inbox).collect();
        entries.extend(self.queued_oracle.drain(..).map(Entry::Oracle));
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

    /// Executes encoded block bytes. On success the lane advances and every
    /// invariant is checked; on a fatal nothing changes.
    pub fn execute_bytes(&mut self, bytes: &[u8]) -> Result<&Receipts, Fatal> {
        let out = self.exec.step(&self.state_bytes, bytes)?;
        let block = BlockInputV1::decode(bytes).expect("an executed block decodes");
        for e in &block.entries {
            if let Entry::Inbox(m) = e {
                self.inbox_acc = sha256(&inbox_acc_preimage(&self.inbox_acc, &m.encode()));
                self.inbox_through += 1;
            }
        }
        let state_hash_after = sha256(&out.state);
        self.prev_block_hash = sha256(&block_hash_preimage(&sha256(bytes), &state_hash_after));
        self.records.push(BlockRecordV1 {
            input: bytes.to_vec(),
            state_hash_after,
        });
        self.state_bytes = out.state;
        self.receipts = Receipts::decode(&out.receipts).expect("receipts decode");
        self.now = self.now.max(block.timestamp_ms) + BLOCK_MS;
        self.check_invariants();
        Ok(&self.receipts)
    }

    pub fn execute(&mut self, block: &BlockInputV1) -> Result<&Receipts, Fatal> {
        let bytes = block.encode().expect("block encodes");
        self.execute_bytes(&bytes)
    }

    pub fn try_block(&mut self, checkpoint_end: bool) -> Result<&Receipts, Fatal> {
        let b = self.build(checkpoint_end);
        self.execute(&b)
    }

    /// Executes the queued entries; panics on a fatal.
    pub fn block(&mut self) -> &Receipts {
        self.run(false)
    }

    /// Executes the queued entries as a `CHECKPOINT_END` block; panics on a fatal.
    pub fn checkpoint(&mut self) -> &Receipts {
        self.run(true)
    }

    fn run(&mut self, checkpoint_end: bool) -> &Receipts {
        let height = self.height() + 1;
        match self.try_block(checkpoint_end) {
            Ok(r) => r,
            Err(f) => panic!("block {height} is fatal: {f:?}"),
        }
    }

    /// Asserts a fatal (pass `result.err()`) with `code`, and `entry` when the
    /// executor reports indexes.
    pub fn expect_fatal(&self, fatal: Option<Fatal>, code: u16, entry: u32) {
        let f = fatal.unwrap_or_else(|| panic!("expected fatal {code}"));
        assert_eq!(f.code, code, "fatal code");
        if self.exec.reports_entry_index() {
            assert_eq!(f.entry_index, entry, "fatal entry index");
        }
    }

    pub fn check_invariants(&self) {
        let st = self.state();
        if let Err(v) = caravel_perps::invariants::check(&st) {
            panic!("invariant broken after block {}: {v:?}", self.height());
        }
        assert_eq!(st.inbox_acc, self.inbox_acc, "INV-P4: inbox accumulator");
        assert_eq!(
            st.inbox_through, self.inbox_through,
            "INV-P4: inbox_through"
        );
    }

    // --- Receipts ---------------------------------------------------------------

    pub fn receipt(&self, entry: u32) -> &Receipt {
        self.receipts
            .receipts
            .iter()
            .find(|r| r.entry_index == entry)
            .unwrap_or_else(|| panic!("no receipt for entry {entry}"))
    }

    pub fn code(&self, entry: u32) -> u16 {
        self.receipt(entry).code
    }

    pub fn events(&self, entry: u32) -> &[Event] {
        &self.receipt(entry).events
    }

    /// Entry indexes of the USER entries in the last block.
    pub fn user_entries(&self) -> Vec<u32> {
        let Some(rec) = self.records.last() else {
            return Vec::new();
        };
        let block = BlockInputV1::decode(&rec.input).expect("recorded block decodes");
        (0..block.entries.len() as u32)
            .filter(|i| matches!(block.entries[*i as usize], Entry::User(_)))
            .collect()
    }

    /// The receipt of the `k`-th USER entry of the last block.
    pub fn user(&self, k: usize) -> &Receipt {
        let entry = *self
            .user_entries()
            .get(k)
            .unwrap_or_else(|| panic!("no user entry {k}"));
        self.receipt(entry)
    }

    /// Reason codes of the USER entries of the last block, in order.
    pub fn user_codes(&self) -> Vec<u16> {
        self.user_entries()
            .into_iter()
            .map(|e| self.code(e))
            .collect()
    }

    /// Asserts that every user entry in the last block succeeded.
    pub fn all_ok(&self) {
        for r in &self.receipts.receipts {
            assert_eq!(
                r.code,
                codes::OK,
                "entry {} rejected with {:?}",
                r.entry_index,
                codes::name(r.code)
            );
        }
    }

    /// The order id of the ORDER_RESTED event of `entry`.
    pub fn rested_id(&self, entry: u32) -> u64 {
        self.events(entry)
            .iter()
            .find_map(|e| match e {
                Event::OrderRested { order_id, .. } => Some(*order_id),
                _ => None,
            })
            .unwrap_or_else(|| panic!("entry {entry} did not rest"))
    }

    pub fn nonces(&self) -> BTreeMap<[u8; 32], u64> {
        self.state()
            .accounts
            .iter()
            .map(|a| (a.key, a.next_nonce))
            .collect()
    }
}
