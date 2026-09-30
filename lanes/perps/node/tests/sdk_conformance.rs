//! P-08 conformance: the app SDK's standard pipeline behaves as the frozen
//! perps engine on everything they share (spec §20.4.3): deposits (created,
//! credited, bounced, slot reuse), forced withdrawals, WITHDRAW, session keys,
//! signer rules, nonces, expiry, rate limits and commitments.
//!
//! The same script runs through `caravel_perps::step` and through the SDK's
//! test app, with the same generic limits, keys and lane id. Every block must
//! give the same receipt codes and platform events, and the same frame: the
//! inbox fold, the totals, the checkpoint count and the whole commitment,
//! including the escape and withdrawal roots. Perps accounts here hold no
//! positions, so their escape equity is their collateral, as an SDK balance.

use caravel_app_sdk::crypto::native::DiagnosticCrypto as SdkCrypto;
use caravel_app_sdk::testapp::TestApp;
use caravel_app_sdk::{AccessMode, AppEngine, AppGenesisV1, SdkState};
use caravel_core::block::{block_hash_preimage, BlockInputV1, Entry};
use caravel_core::inbox::{InboxKind, InboxMsgV1};
use caravel_core::receipts::{ReceiptsV1, PSEUDO_BLOCK_START};
use caravel_core::state::StateFrameV1;
use caravel_core::tx::{sep53_preimage, sep53_tx_message, SigScheme, StandardBody, TxEnvelopeV1};
use caravel_perps::native::DiagnosticCrypto as PerpsCrypto;
use caravel_runtime::checkpoint::sha256;
use caravel_types::config::GenesisConfigV1;
use caravel_types::state::StateV1;
use caravel_types::vectors::{key, pk};
use ed25519_dalek::Signer;

const A: u8 = 0xA1;
const B: u8 = 0xB2;
const C: u8 = 0xC3;
const K: u8 = 0x5E;
const USDC: i128 = 10_000_000;
const T0: u64 = 1_790_000_000_000;

fn perps_config() -> GenesisConfigV1 {
    let mut c = caravel_types::vectors::config();
    c.max_accounts = 4; // two system accounts and two users
    c.max_pending_withdrawals = 4;
    c.max_session_keys = 2;
    c.max_txs_per_account_per_block = 3;
    c
}

/// The same generic settings as an `AppGenesisV1` for the test app.
fn sdk_config(p: &GenesisConfigV1) -> AppGenesisV1 {
    AppGenesisV1 {
        lane_id: p.lane_id,
        template: TestApp::TEMPLATE,
        template_version: 1,
        system_keys: vec![p.backstop_key, p.treasury_key],
        access_mode: match p.access_mode {
            caravel_types::config::AccessMode::Open => AccessMode::Open,
            caravel_types::config::AccessMode::Allowlist => AccessMode::Allowlist,
        },
        allowlist: p.allowlist.clone(),
        min_deposit: p.min_deposit,
        min_withdrawal: p.min_withdrawal,
        max_accounts: p.max_accounts,
        max_session_keys: p.max_session_keys,
        max_txs_per_account_per_block: p.max_txs_per_account_per_block,
        max_entries_per_block: p.max_entries_per_block,
        max_block_bytes: p.max_block_bytes,
        max_pending_withdrawals: p.max_pending_withdrawals,
        exec_cpu_limit: p.exec_cpu_limit,
        exec_mem_limit: p.exec_mem_limit,
        app_params: vec![],
    }
}

#[derive(Clone, Copy)]
enum Op {
    Deposit(u8, i128),
    Forced(u8, i128),
    /// `(account, signer, body)`, signed with the next nonce.
    Tx(u8, u8, StandardBody),
    /// The same, signed by the owner with SEP-53.
    Sep53(u8, StandardBody),
    /// A tx with a used nonce.
    StaleNonce(u8),
    /// A tx whose expiry is before the block.
    Expired(u8),
    /// A tx for another lane.
    WrongLane(u8),
}

struct Side {
    perps: bool,
    state: Vec<u8>,
    config_hash: [u8; 32],
    lane_id: [u8; 32],
    inbox_n: u64,
}

impl Side {
    fn nonce(&self, seed: u8) -> u64 {
        if self.perps {
            StateV1::decode(&self.state)
                .unwrap()
                .accounts
                .iter()
                .find(|a| a.key == pk(seed))
                .map_or(0, |a| a.next_nonce)
        } else {
            SdkState::decode(&self.state, &TestApp::STATE_MAGIC)
                .unwrap()
                .accounts
                .iter()
                .find(|a| a.key == pk(seed))
                .map_or(0, |a| a.next_nonce)
        }
    }

    /// Every account's key, nonce, balance (perps: collateral, no positions here)
    /// and session keys, then the pending queue.
    fn accounts(&self) -> String {
        if self.perps {
            let st = StateV1::decode(&self.state).unwrap();
            let accounts: Vec<String> = st
                .accounts
                .iter()
                .map(|a| {
                    let keys: Vec<_> = a
                        .session_keys
                        .iter()
                        .map(|k| (k.key, k.expires_at_ms, k.permissions))
                        .collect();
                    format!("{:?}/{}/{}/{:?}", a.key, a.next_nonce, a.collateral, keys)
                })
                .collect();
            let pending: Vec<_> = st.pending.iter().map(|p| (p.key, p.amount)).collect();
            format!("{accounts:?} {pending:?}")
        } else {
            let st = SdkState::decode(&self.state, &TestApp::STATE_MAGIC).unwrap();
            let accounts: Vec<String> = st
                .accounts
                .iter()
                .map(|a| {
                    let keys: Vec<_> = a
                        .session_keys
                        .iter()
                        .map(|k| (k.key, k.expires_at_ms, k.permissions))
                        .collect();
                    format!("{:?}/{}/{}/{:?}", a.key, a.next_nonce, a.balance, keys)
                })
                .collect();
            let pending: Vec<_> = st.pending.iter().map(|p| (p.key, p.amount)).collect();
            format!("{accounts:?} {pending:?}")
        }
    }

    fn tx(
        &self,
        account: u8,
        signer: u8,
        scheme: SigScheme,
        nonce: u64,
        expiry: u64,
        body: StandardBody,
    ) -> TxEnvelopeV1 {
        let mut tx = TxEnvelopeV1 {
            lane_id: self.lane_id,
            account: pk(account),
            signer: pk(signer),
            nonce,
            expiry_ms: expiry,
            kind: body.kind(),
            sig_scheme: scheme,
            body: body.encode(),
            signature: [0; 64],
        };
        let hash = sha256(&tx.tx_hash_preimage(&self.config_hash));
        let msg = match scheme {
            SigScheme::RawEd25519 => hash.to_vec(),
            SigScheme::Sep53 => sha256(&sep53_preimage(&sep53_tx_message(&hash))).to_vec(),
        };
        tx.signature = key(signer).sign(&msg).to_bytes();
        tx
    }

    /// Runs one block; returns the entry receipts and the frame.
    fn run(&mut self, ops: &[Op], now: u64, checkpoint_end: bool) -> (ReceiptsV1, StateFrameV1) {
        let mut next = std::collections::BTreeMap::<u8, u64>::new();
        let mut nonce = |s: &Side, seed: u8| {
            let n = *next.entry(seed).or_insert_with(|| s.nonce(seed));
            next.insert(seed, n + 1);
            n
        };
        let later = now + 60_000;
        let mut entries = Vec::new();
        for op in ops {
            let e = match *op {
                Op::Deposit(seed, amount) | Op::Forced(seed, amount) => {
                    let kind = if matches!(op, Op::Deposit(..)) {
                        InboxKind::Deposit
                    } else {
                        InboxKind::ForcedWithdrawal
                    };
                    let m = InboxMsgV1 {
                        kind,
                        index: self.inbox_n,
                        lane_account: pk(seed),
                        amount,
                        enqueued_at: now / 1000,
                    };
                    self.inbox_n += 1;
                    Entry::Inbox(m)
                }
                Op::Tx(account, signer, body) => {
                    let n = nonce(self, account);
                    Entry::User(self.tx(account, signer, SigScheme::RawEd25519, n, later, body))
                }
                Op::Sep53(account, body) => {
                    let n = nonce(self, account);
                    Entry::User(self.tx(account, account, SigScheme::Sep53, n, later, body))
                }
                Op::StaleNonce(account) => {
                    let n = self.nonce(account).saturating_sub(1);
                    Entry::User(self.tx(
                        account,
                        account,
                        SigScheme::RawEd25519,
                        n,
                        later,
                        StandardBody::Withdraw { amount: USDC },
                    ))
                }
                Op::Expired(account) => {
                    let n = self.nonce(account);
                    Entry::User(self.tx(
                        account,
                        account,
                        SigScheme::RawEd25519,
                        n,
                        now - 1,
                        StandardBody::Withdraw { amount: USDC },
                    ))
                }
                Op::WrongLane(account) => {
                    let n = self.nonce(account);
                    let mut tx = self.tx(
                        account,
                        account,
                        SigScheme::RawEd25519,
                        n,
                        later,
                        StandardBody::Withdraw { amount: USDC },
                    );
                    tx.lane_id = [9; 32];
                    let hash = sha256(&tx.tx_hash_preimage(&self.config_hash));
                    tx.signature = key(account).sign(&hash).to_bytes();
                    Entry::User(tx)
                }
            };
            entries.push(e);
        }
        let frame = StateFrameV1::read(&self.state).unwrap();
        let prev = if frame.height == 0 {
            [0; 32]
        } else {
            sha256(&block_hash_preimage(
                &frame.last_block_input_hash,
                &sha256(&self.state),
            ))
        };
        let block = BlockInputV1 {
            lane_id: frame.lane_id,
            height: frame.height + 1,
            timestamp_ms: now,
            prev_block_hash: prev,
            checkpoint_end,
            entries,
        }
        .encode()
        .unwrap();
        let (state, receipts) = if self.perps {
            let o = caravel_perps::step(&self.state, &block, &PerpsCrypto).unwrap();
            (o.state, o.receipts)
        } else {
            let o = caravel_app_sdk::step::<TestApp, _>(&self.state, &block, &SdkCrypto).unwrap();
            (o.state, o.receipts)
        };
        self.state = state;
        (
            ReceiptsV1::decode(&receipts).unwrap(),
            StateFrameV1::read(&self.state).unwrap(),
        )
    }
}

/// What must match: entry codes and platform events, and the frame without
/// the app's words and the hashes that include app bytes.
fn comparable(r: &ReceiptsV1, f: &StateFrameV1) -> String {
    let receipts: Vec<String> = r
        .receipts
        .iter()
        .map(|rc| {
            let events: Vec<String> = rc
                .events
                .iter()
                .filter_map(|e| e.platform().map(|p| format!("{:?}", p.unwrap())))
                .collect();
            let at = if rc.entry_index >= PSEUDO_BLOCK_START {
                "block".to_string()
            } else {
                rc.entry_index.to_string()
            };
            format!("{at}:{}:{}", rc.code, events.join(","))
        })
        .collect();
    format!(
        "{receipts:?} height={} ts={} seq={} inbox={} {:?} deposits={} committed={} {:?}",
        f.height,
        f.last_timestamp_ms,
        f.checkpoint_seq,
        f.inbox_through,
        f.inbox_acc,
        f.deposits_credited_total,
        f.withdrawals_committed_total,
        f.last_commitment
    )
}

#[test]
fn the_sdk_s_standard_paths_match_perps() {
    let pc = perps_config();
    let sc = sdk_config(&pc);
    let perps_bytes = pc.encode().unwrap();
    let sdk_bytes = sc.encode().unwrap();
    let mut perps = Side {
        perps: true,
        state: caravel_perps::genesis(&perps_bytes, &caravel_perps::native::NativeCrypto).unwrap(),
        config_hash: sha256(&perps_bytes),
        lane_id: pc.lane_id,
        inbox_n: 0,
    };
    let mut sdk = Side {
        perps: false,
        state: caravel_app_sdk::genesis::<TestApp>(
            &sdk_bytes,
            &caravel_app_sdk::crypto::native::NativeCrypto,
        )
        .unwrap(),
        config_hash: sha256(&sdk_bytes),
        lane_id: sc.lane_id,
        inbox_n: 0,
    };
    let add = |key_seed: u8, exp: u64, perms: u8| StandardBody::AddSessionKey {
        session_key: pk(key_seed),
        expires_at_ms: exp,
        permissions: perms,
    };
    let revoke = |key_seed: u8| StandardBody::RevokeSessionKey {
        session_key: pk(key_seed),
    };
    let w = |amount: i128| StandardBody::Withdraw { amount };
    let week = 7 * 24 * 3_600_000;
    let mut now = T0;
    let script: Vec<(Vec<Op>, bool)> = vec![
        // Created, created, then no slot: bounced.
        (
            vec![
                Op::Deposit(A, 100 * USDC),
                Op::Deposit(B, 50 * USDC),
                Op::Deposit(C, 20 * USDC),
            ],
            false,
        ),
        // Session keys, withdrawals, signer rules, SEP-53, nonces, expiry, lane.
        (
            vec![
                Op::Tx(A, A, add(K, T0 + 1_000 + 3_600_000, 0x01)),
                Op::Tx(A, A, add(0x5F, T0 + 1_000 + 3_600_000, 0x04)),
                Op::Tx(A, A, w(USDC - 1)),
                Op::Tx(B, B, w(51 * USDC)),
                Op::Sep53(B, w(10 * USDC)),
                Op::StaleNonce(A),
                Op::Expired(B),
                Op::WrongLane(A),
            ],
            false,
        ),
        // A session key can't withdraw or manage keys; a forced withdrawal takes
        // what is free; an unknown account's is a no-op.
        (
            vec![
                Op::Forced(B, 70 * USDC),
                Op::Forced(0x77, USDC),
                Op::Tx(A, K, w(USDC)),
                Op::Tx(A, K, revoke(K)),
                Op::Tx(A, A, add(0x60, T0 + 2_000 + week + 1, 0x01)),
                Op::Tx(A, A, add(0x61, T0 + 2_000, 0x01)),
            ],
            false,
        ),
        // Rate limit (3 per block), revoke twice, too many keys.
        (
            vec![
                Op::Tx(A, A, add(0x62, T0 + 3_000 + 1, 0x01)),
                Op::Tx(A, A, add(0x63, T0 + 3_000 + 1, 0x01)),
                Op::Tx(A, A, revoke(K)),
                Op::Tx(A, A, revoke(K)),
            ],
            false,
        ),
        (
            vec![
                Op::Tx(A, A, revoke(K)),
                Op::Tx(A, A, add(0x64, T0 + 4_000 + 1, 0x01)),
            ],
            false,
        ),
        // The commitment: escape and withdrawal roots.
        (vec![Op::Tx(A, A, w(10 * USDC))], true),
        // B is empty and has no pending withdrawal: C takes its slot.
        (
            vec![Op::Deposit(C, 30 * USDC), Op::Deposit(B, 5 * USDC)],
            false,
        ),
        // The queue holds 4: the fifth withdrawal is refused; then lane liquidity.
        (
            vec![
                Op::Tx(A, A, w(USDC)),
                Op::Tx(A, A, w(USDC)),
                Op::Tx(A, A, w(USDC)),
            ],
            false,
        ),
        (
            vec![
                Op::Tx(C, C, w(USDC)),
                Op::Tx(C, C, w(USDC)),
                Op::Tx(C, C, w(29 * USDC)),
            ],
            true,
        ),
        (
            vec![Op::Forced(A, 1_000 * USDC), Op::Tx(C, C, w(29 * USDC))],
            true,
        ),
    ];
    let mut codes_seen = std::collections::BTreeSet::new();
    for (i, (ops, cp)) in script.iter().enumerate() {
        let (pr, pf) = perps.run(ops, now, *cp);
        let (sr, sf) = sdk.run(ops, now, *cp);
        assert_eq!(
            comparable(&sr, &sf),
            comparable(&pr, &pf),
            "block {}",
            i + 1
        );
        assert_eq!(
            sdk.accounts(),
            perps.accounts(),
            "accounts after block {}",
            i + 1
        );
        codes_seen.extend(pr.receipts.iter().map(|r| r.code));
        now += 1_000;
    }
    // The script reaches every standard code it is meant to.
    use caravel_core::codes::receipt as c;
    for code in [
        c::OK,
        c::WRONG_LANE,
        c::UNAUTHORIZED_SIGNER,
        c::EXPIRED,
        c::BAD_NONCE,
        c::RATE_LIMITED,
        c::BELOW_MIN_WITHDRAWAL,
        c::INSUFFICIENT_FREE_BALANCE,
        c::WITHDRAWAL_QUEUE_FULL,
        c::TOO_MANY_SESSION_KEYS,
        c::BAD_SESSION_KEY,
    ] {
        assert!(
            codes_seen.contains(&code),
            "code {code} not exercised: {codes_seen:?}"
        );
    }
}
