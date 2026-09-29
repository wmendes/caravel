//! A sequencer core on a fake clock, for the sequencer and validator tests.

use caravel_lane::checkpoint::{network_id, settlement_addr_hash, sha256, HeaderIds};
use caravel_lane::sequencer::{self, Core, Executor, InboxReport, SequencerConfig};
use caravel_lane::store::Store;
use caravel_testkit::lane::{
    config, seeds, BTC, BTC_PRICE, ETH, ETH_PRICE, T0, TICK, USDC, XLM, XLM_PRICE,
};
use caravel_testkit::Lane;
use caravel_types::inbox::{inbox_acc_preimage, InboxKind, InboxMsgV1};
use caravel_types::tx::{LaneTxV1, PlaceOrder, Side, SigScheme, Tif, TxBody};
use caravel_types::vectors::pk;

pub fn ids() -> HeaderIds {
    HeaderIds {
        network_id: network_id("Test SDF Network ; September 2015"),
        settlement_addr_hash: settlement_addr_hash(&[7; 32]),
        engine_wasm_hash: super::engine_hash(),
    }
}

pub fn seq_config() -> SequencerConfig {
    SequencerConfig {
        ids: ids(),
        checkpoint_every_blocks: 10,
        max_batch_bytes: 96_000,
        mempool_max: 10_000,
        mempool_max_per_account: 256,
    }
}

/// A core plus a testkit lane used only to sign transactions and oracle updates.
pub struct T {
    pub core: Core,
    pub signer: Lane,
    pub now: u64,
    pub inbox_n: u64,
    pub inbox_acc: [u8; 32],
}

pub fn genesis() -> (Vec<u8>, [u8; 32]) {
    let bytes = config().encode().unwrap();
    let state = caravel_perps::genesis(&bytes, &caravel_perps::native::NativeCrypto).unwrap();
    (state, sha256(&bytes))
}

pub fn store_in_memory() -> Store {
    let (state, config_hash) = genesis();
    Store::open_in_memory(&config().lane_id, &config_hash, &state).unwrap()
}

impl T {
    pub fn with(exec: Executor, store: Store) -> Self {
        let (_, config_hash) = genesis();
        let core = Core::open(exec, store, config_hash, seq_config()).unwrap();
        let inbox_n = core.inbox_reported();
        let inbox_acc = match inbox_n {
            0 => [0; 32],
            n => core.store().inbox_acc(n - 1).unwrap().unwrap(),
        };
        let now = core.state().last_timestamp_ms.max(T0) + 1000;
        Self {
            core,
            signer: Lane::native(config()),
            now,
            inbox_n,
            inbox_acc,
        }
    }

    pub fn native() -> Self {
        Self::with(Executor::Native, store_in_memory())
    }

    pub fn inbox(&mut self, kind: InboxKind, seed: u8, amount: i128) {
        let msg = InboxMsgV1 {
            kind,
            index: self.inbox_n,
            lane_account: pk(seed),
            amount,
            enqueued_at: self.now / 1000,
        };
        let acc = sha256(&inbox_acc_preimage(&self.inbox_acc, &msg.encode()));
        assert_eq!(
            self.core.report_inbox(msg, acc).unwrap(),
            InboxReport::Added
        );
        self.inbox_n += 1;
        self.inbox_acc = acc;
    }

    pub fn deposit(&mut self, seed: u8, amount: i128) {
        self.inbox(InboxKind::Deposit, seed, amount);
    }

    pub fn prices(&mut self) {
        for (m, p) in [(BTC, BTC_PRICE), (ETH, ETH_PRICE), (XLM, XLM_PRICE)] {
            let u = self.signer.oracle_update(seeds::ORACLE, m, p, self.now);
            assert!(self.core.report_oracle(u).unwrap());
        }
    }

    pub fn next_nonce(&self, seed: u8) -> u64 {
        let base = self
            .core
            .state()
            .accounts
            .iter()
            .find(|a| a.key == pk(seed))
            .map_or(0, |a| a.next_nonce);
        base + self
            .core
            .mempool
            .iter()
            .filter(|q| q.tx.account == pk(seed))
            .count() as u64
    }

    pub fn signed(&self, seed: u8, nonce: u64, body: TxBody) -> LaneTxV1 {
        self.signer.signed(
            seed,
            seed,
            SigScheme::RawEd25519,
            nonce,
            self.now + 60_000,
            body,
        )
    }

    pub fn tx(&mut self, seed: u8, body: TxBody) -> [u8; 32] {
        let tx = self.signed(seed, self.next_nonce(seed), body);
        self.core.submit_tx(&tx.encode(), self.now).unwrap()
    }

    pub fn order(
        &mut self,
        seed: u8,
        market_id: u16,
        side: Side,
        price: i64,
        lots: i64,
    ) -> [u8; 32] {
        self.tx(
            seed,
            TxBody::PlaceOrder(PlaceOrder {
                market_id,
                side,
                tif: Tif::Gtc,
                reduce_only: false,
                price,
                lots,
                client_order_id: 0,
            }),
        )
    }

    pub fn block(&mut self) -> sequencer::Produced {
        let p = self.core.produce_block(self.now).unwrap();
        self.now += 1000;
        p
    }
}

/// Deposits, prices and a cross, then blocks up to `height`.
pub fn busy_lane(t: &mut T, height: u64) {
    t.deposit(seeds::A, 10_000 * USDC);
    t.deposit(seeds::B, 10_000 * USDC);
    t.prices();
    t.block();
    t.order(seeds::A, BTC, Side::Buy, BTC_PRICE, 10);
    t.order(seeds::B, BTC, Side::Sell, BTC_PRICE, 4);
    t.order(seeds::B, ETH, Side::Sell, ETH_PRICE + 10 * TICK, 3);
    t.tx(seeds::A, TxBody::Withdraw { amount: 100 * USDC });
    while t.core.height() < height {
        if t.core.height().is_multiple_of(3) {
            t.prices();
        }
        t.block();
    }
}
