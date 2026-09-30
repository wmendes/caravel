//! A test lane for the SDK's test app: config, signed envelopes and blocks.

#![allow(dead_code)]

use caravel_app_sdk::crypto::native::{DiagnosticCrypto, NativeCrypto};
use caravel_app_sdk::testapp::TestApp;
use caravel_app_sdk::{
    genesis, step, AccessMode, AppEngine, AppGenesisV1, Fatal, SdkState, StepOutput,
};
use caravel_core::block::{block_hash_preimage, BlockInputV1, Entry};
use caravel_core::inbox::{InboxKind, InboxMsgV1};
use caravel_core::receipts::ReceiptsV1;
use caravel_core::tx::{sep53_preimage, sep53_tx_message, SigScheme, StandardBody, TxEnvelopeV1};
use ed25519_dalek::{Signer, SigningKey};
use sha2::Digest;

pub const T0: u64 = 1_790_000_000_000;
pub const USDC: i128 = 10_000_000;

pub fn sha256(b: &[u8]) -> [u8; 32] {
    sha2::Sha256::digest(b).into()
}

pub fn sk(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

pub fn pk(seed: u8) -> [u8; 32] {
    sk(seed).verifying_key().to_bytes()
}

pub fn config() -> AppGenesisV1 {
    AppGenesisV1 {
        lane_id: sha256(b"sdk-test-lane"),
        template: TestApp::TEMPLATE,
        template_version: 1,
        system_keys: vec![pk(0x21), pk(0x22)],
        access_mode: AccessMode::Open,
        allowlist: vec![],
        min_deposit: USDC,
        min_withdrawal: USDC,
        max_accounts: 8,
        max_session_keys: 4,
        max_txs_per_account_per_block: 50,
        max_entries_per_block: 256,
        max_block_bytes: 12_000,
        max_pending_withdrawals: 16,
        exec_cpu_limit: 200_000_000,
        exec_mem_limit: 41_943_040,
        app_params: vec![],
    }
}

/// A lane driven block by block, with the native crypto.
pub struct Lane {
    pub config_bytes: Vec<u8>,
    pub config_hash: [u8; 32],
    pub state: Vec<u8>,
    pub now: u64,
    pub inbox_n: u64,
    pub last: Option<StepOutput>,
}

impl Lane {
    pub fn new(config: AppGenesisV1) -> Self {
        let config_bytes = config.encode().unwrap();
        let state = genesis::<TestApp>(&config_bytes, &NativeCrypto).unwrap();
        Self {
            config_hash: sha256(&config_bytes),
            config_bytes,
            state,
            now: T0,
            inbox_n: 0,
            last: None,
        }
    }

    pub fn st(&self) -> SdkState {
        SdkState::decode(&self.state, &TestApp::STATE_MAGIC).unwrap()
    }

    pub fn nonce(&self, seed: u8) -> u64 {
        self.st()
            .accounts
            .iter()
            .find(|a| a.key == pk(seed))
            .map_or(0, |a| a.next_nonce)
    }

    pub fn inbox(&mut self, kind: InboxKind, seed: u8, amount: i128) -> Entry {
        let m = InboxMsgV1 {
            kind,
            index: self.inbox_n,
            lane_account: pk(seed),
            amount,
            enqueued_at: self.now / 1000,
        };
        self.inbox_n += 1;
        Entry::Inbox(m)
    }

    pub fn deposit(&mut self, seed: u8, amount: i128) -> Entry {
        self.inbox(InboxKind::Deposit, seed, amount)
    }

    /// An envelope signed by `signer_seed` for `account_seed`.
    pub fn tx(
        &self,
        account_seed: u8,
        signer_seed: u8,
        scheme: SigScheme,
        nonce: u64,
        kind: u8,
        body: Vec<u8>,
    ) -> TxEnvelopeV1 {
        let mut tx = TxEnvelopeV1 {
            lane_id: config().lane_id,
            account: pk(account_seed),
            signer: pk(signer_seed),
            nonce,
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
        tx.signature = sk(signer_seed).sign(&msg).to_bytes();
        tx
    }

    /// Signs `tx` again after a field changed (raw ed25519 by its signer's seed is not known here,
    /// so the signer must be one of the test seeds: the account's own).
    pub fn tx_resigned(&self, mut tx: TxEnvelopeV1) -> TxEnvelopeV1 {
        let seed = (0u8..=255)
            .find(|s| pk(*s) == tx.signer)
            .expect("a test seed");
        let hash = sha256(&tx.tx_hash_preimage(&self.config_hash));
        tx.signature = sk(seed).sign(&hash).to_bytes();
        tx
    }

    pub fn owner(&self, seed: u8, body: StandardBody) -> Entry {
        Entry::User(self.tx(
            seed,
            seed,
            SigScheme::RawEd25519,
            self.nonce(seed),
            body.kind(),
            body.encode(),
        ))
    }

    pub fn owner_with_nonce(&self, seed: u8, nonce: u64, body: StandardBody) -> Entry {
        Entry::User(self.tx(
            seed,
            seed,
            SigScheme::RawEd25519,
            nonce,
            body.kind(),
            body.encode(),
        ))
    }

    pub fn count(&self, account: u8, signer: u8, nonce: u64, by: u64) -> Entry {
        Entry::User(self.tx(
            account,
            signer,
            SigScheme::RawEd25519,
            nonce,
            caravel_app_sdk::testapp::COUNT,
            by.to_le_bytes().to_vec(),
        ))
    }

    pub fn block_bytes(&self, entries: Vec<Entry>, checkpoint_end: bool) -> Vec<u8> {
        let st = self.st();
        let prev = if st.height == 0 {
            [0; 32]
        } else {
            sha256(&block_hash_preimage(
                &st.last_block_input_hash,
                &sha256(&self.state),
            ))
        };
        BlockInputV1 {
            lane_id: st.lane_id,
            height: st.height + 1,
            timestamp_ms: self.now,
            prev_block_hash: prev,
            checkpoint_end,
            entries,
        }
        .encode()
        .unwrap()
    }

    /// Runs a block; on success advances the lane and the clock.
    pub fn run(&mut self, entries: Vec<Entry>, checkpoint_end: bool) -> Result<ReceiptsV1, Fatal> {
        let bytes = self.block_bytes(entries, checkpoint_end);
        let out = step::<TestApp, _>(&self.state, &bytes, &DiagnosticCrypto)?;
        // The trapping crypto gives the same bytes (it only differs on a bad signature).
        assert_eq!(
            step::<TestApp, _>(&self.state, &bytes, &NativeCrypto).unwrap(),
            out
        );
        self.state = out.state.clone();
        let receipts = ReceiptsV1::decode(&out.receipts).unwrap();
        self.last = Some(out);
        self.now += 1_000;
        Ok(receipts)
    }

    pub fn codes(&mut self, entries: Vec<Entry>) -> Vec<u16> {
        let r = self.run(entries, false).unwrap();
        // Entry receipts only, not the block's pseudo entries.
        r.receipts
            .iter()
            .filter(|r| r.entry_index < caravel_core::receipts::PSEUDO_BLOCK_START)
            .map(|r| r.code)
            .collect()
    }
}
