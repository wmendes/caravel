//! Golden vectors for every format in spec §9 (spec §19.2).
//!
//! `cargo gen-vectors` writes these files to `test-vectors/`. The Rust tests,
//! the settlement contract tests and the TypeScript codec tests all load them.
//! A unit test regenerates them in memory and fails if the committed files are
//! stale, so a format cannot change silently (spec §0.3 rule 4).
//!
//! Every value is deterministic: keys come from fixed seeds and ed25519
//! signing is deterministic. Integers are decimal strings (safe for u64/i128
//! in JavaScript), bytes are lowercase hex.

use std::format;
use std::string::{String, ToString};
use std::vec;
use std::vec::Vec;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::batch::BatchV1;
use crate::block::{block_hash_preimage, BlockInputV1, BlockRecordV1, Entry};
use crate::checkpoint::CheckpointHeaderV1;
use crate::codes;
use crate::config::{AccessMode, GenesisConfigV1, MarketParamsV1};
use crate::inbox::{inbox_acc_preimage, InboxKind, InboxMsgV1};
use crate::oracle::OracleUpdateV1;
use crate::preimage::{
    account_leaf_preimage, engine_version_preimage, lane_id_preimage, rotate_message_preimage,
    signers_hash_preimage, withdrawal_leaf_preimage, WeightedSigner,
};
use crate::receipts::{
    CancelReason, DepositOutcome, Event, Receipt, Receipts, PSEUDO_BLOCK_END, PSEUDO_BLOCK_START,
};
use crate::state::{
    AccountV1, CommitmentV1, MarketStateV1, OrderV1, PendingWithdrawalV1, PositionV1, SessionKeyV1,
    StateV1,
};
use crate::step::StepEnvelope;
use crate::tx::{
    sep53_preimage, sep53_tx_message, LaneTxV1, PlaceOrder, Side, SigScheme, Tif, TxBody,
    ALL_MARKETS, PERM_ALL,
};
use crate::SPEC_VERSION;

/// Lane name used by the vectors (the M0 lane name, spec §9.1).
pub const LANE_NAME: &str = "caravel-perps-testnet-0";
/// `H("Test SDF Network ; September 2015")`, the testnet network id.
pub const TESTNET_PASSPHRASE: &str = "Test SDF Network ; September 2015";

/// One generated file.
pub struct VectorFile {
    pub name: &'static str,
    pub contents: String,
}

// ---------------------------------------------------------------------------
// Tiny deterministic JSON writer.

enum J {
    S(String),
    B(bool),
    A(Vec<J>),
    O(Vec<(String, J)>),
}

fn s(v: impl ToString) -> J {
    J::S(v.to_string())
}

fn h(bytes: &[u8]) -> J {
    J::S(hex(bytes))
}

fn obj(fields: Vec<(&str, J)>) -> J {
    J::O(
        fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

impl J {
    fn write(&self, out: &mut String, indent: usize) {
        let pad = |n: usize| " ".repeat(n);
        match self {
            J::S(v) => {
                out.push('"');
                for c in v.chars() {
                    match c {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        c => out.push(c),
                    }
                }
                out.push('"');
            }
            J::B(b) => out.push_str(if *b { "true" } else { "false" }),
            J::A(items) if items.is_empty() => out.push_str("[]"),
            J::A(items) => {
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    out.push_str(&pad(indent + 2));
                    item.write(out, indent + 2);
                    out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
                }
                out.push_str(&pad(indent));
                out.push(']');
            }
            J::O(fields) if fields.is_empty() => out.push_str("{}"),
            J::O(fields) => {
                out.push_str("{\n");
                for (i, (k, v)) in fields.iter().enumerate() {
                    out.push_str(&pad(indent + 2));
                    out.push('"');
                    out.push_str(k);
                    out.push_str("\": ");
                    v.write(out, indent + 2);
                    out.push_str(if i + 1 < fields.len() { ",\n" } else { "\n" });
                }
                out.push_str(&pad(indent));
                out.push('}');
            }
        }
    }

    fn render(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out.push('\n');
        out
    }
}

// ---------------------------------------------------------------------------
// Helpers.

pub fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(HEX[usize::from(b >> 4)]));
        out.push(char::from(HEX[usize::from(b & 0x0F)]));
    }
    out
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// A deterministic test key: the ed25519 key with seed `[n; 32]`.
pub fn key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

pub fn pk(n: u8) -> [u8; 32] {
    key(n).verifying_key().to_bytes()
}

fn sign(n: u8, msg: &[u8]) -> [u8; 64] {
    key(n).sign(msg).to_bytes()
}

/// Seeds of the fixture keys.
const BACKSTOP: u8 = 0x21;
const TREASURY: u8 = 0x22;
const ORACLE: u8 = 0x31;
const OWNER_A: u8 = 0x41;
const OWNER_B: u8 = 0x42;
const SESSION_A: u8 = 0x51;
const VALIDATORS: [u8; 3] = [0x61, 0x62, 0x63];

fn file(
    format: &str,
    spec: &str,
    hash_rule: &str,
    context: Vec<(&str, J)>,
    valid: Vec<J>,
    invalid: Vec<J>,
) -> String {
    obj(vec![
        ("format", s(format)),
        ("spec", s(spec)),
        ("hash_rule", s(hash_rule)),
        ("context", obj(context)),
        ("vectors", J::A(valid)),
        ("invalid", J::A(invalid)),
    ])
    .render()
}

fn vector(name: &str, fields: J, bytes: &[u8], hash: &[u8; 32]) -> J {
    obj(vec![
        ("name", s(name)),
        ("fields", fields),
        ("hex", h(bytes)),
        ("hash", h(hash)),
    ])
}

fn invalid(name: &str, bytes: &[u8], error: impl core::fmt::Debug) -> J {
    obj(vec![
        ("name", s(name)),
        ("hex", h(bytes)),
        ("error", s(format!("{error:?}"))),
    ])
}

fn with_byte(bytes: &[u8], at: usize, v: u8) -> Vec<u8> {
    let mut b = bytes.to_vec();
    b[at] = v;
    b
}

fn with_push(bytes: &[u8], v: u8) -> Vec<u8> {
    let mut b = bytes.to_vec();
    b.push(v);
    b
}

// ---------------------------------------------------------------------------
// Shared fixtures.

pub fn lane_id() -> [u8; 32] {
    sha256(&lane_id_preimage(LANE_NAME))
}

pub fn network_id() -> [u8; 32] {
    sha256(TESTNET_PASSPHRASE.as_bytes())
}

fn symbol(sym: &str) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..sym.len()].copy_from_slice(sym.as_bytes());
    out
}

/// The §10.3 lane settings with fixture keys (not the deployed testnet keys).
pub fn config() -> GenesisConfigV1 {
    let market = |id, sym: &str, lot, dec, imf, mmf, pos, oi, impact| MarketParamsV1 {
        market_id: id,
        symbol: symbol(sym),
        tick: 1000,
        imf_bps: imf,
        mmf_bps: mmf,
        taker_fee_bps: 5,
        maker_fee_bps: 0,
        liq_fee_bps: 100,
        band_bps: 500,
        max_position_lots: pos,
        max_oi_lots: oi,
        impact_lots: impact,
        display_lot_base_units: lot,
        display_base_decimals: dec,
    };
    GenesisConfigV1 {
        lane_id: lane_id(),
        backstop_key: pk(BACKSTOP),
        treasury_key: pk(TREASURY),
        access_mode: AccessMode::Open,
        allowlist: Vec::new(),
        oracle_keys: vec![pk(ORACLE)],
        oracle_max_staleness_ms: 30_000,
        oracle_max_future_ms: 5_000,
        oracle_circuit_breaker_bps: 1_000,
        oracle_breaker_bps_per_sec: 10,
        funding_interval_ms: 3_600_000,
        funding_damping: 8,
        funding_max_rate_ppm: 500,
        insurance_fee_share_bps: 3_000,
        min_deposit: 10_000_000,
        min_withdrawal: 10_000_000,
        max_accounts: 1024,
        max_orders_per_side: 256,
        max_open_orders_per_account: 32,
        max_session_keys: 4,
        max_txs_per_account_per_block: 50,
        max_entries_per_block: 256,
        max_block_bytes: 24_000,
        max_pending_withdrawals: 512,
        exec_cpu_limit: 400_000_000,
        exec_mem_limit: 41_943_040,
        markets: vec![
            market(1, "BTC-PERP", 10_000, 8, 1000, 500, 20_000, 200_000, 1_000),
            market(2, "ETH-PERP", 100_000, 8, 1000, 500, 50_000, 500_000, 2_000),
            market(
                3,
                "XLM-PERP",
                100_000_000,
                7,
                2000,
                1000,
                100_000,
                1_000_000,
                1_000,
            ),
        ],
    }
}

pub fn config_hash() -> [u8; 32] {
    sha256(&config().encode().expect("fixture config encodes"))
}

/// Signs `tx` per spec §9.2 with the key of seed `signer_seed`.
fn signed(mut tx: LaneTxV1, signer_seed: u8) -> LaneTxV1 {
    let tx_hash = sha256(&tx.tx_hash_preimage(&config_hash()));
    tx.signature = match tx.sig_scheme {
        SigScheme::RawEd25519 => sign(signer_seed, &tx_hash),
        SigScheme::Sep53 => sign(
            signer_seed,
            &sha256(&sep53_preimage(&sep53_tx_message(&tx_hash))),
        ),
    };
    tx
}

fn base_tx(signer: u8, nonce: u64, sig_scheme: SigScheme, body: TxBody) -> LaneTxV1 {
    signed(
        LaneTxV1 {
            lane_id: lane_id(),
            account: pk(OWNER_A),
            signer: pk(signer),
            nonce,
            expiry_ms: 1_790_000_060_000,
            sig_scheme,
            body,
            signature: [0; 64],
        },
        signer,
    )
}

fn place_order() -> LaneTxV1 {
    base_tx(
        SESSION_A,
        1_790_000_000_000,
        SigScheme::RawEd25519,
        TxBody::PlaceOrder(PlaceOrder {
            market_id: 1,
            side: Side::Buy,
            tif: Tif::Gtc,
            reduce_only: false,
            price: 6_500_000_000,
            lots: 250,
            client_order_id: 7,
        }),
    )
}

fn inbox_messages() -> [InboxMsgV1; 3] {
    [
        InboxMsgV1 {
            kind: InboxKind::Deposit,
            index: 0,
            lane_account: pk(OWNER_A),
            amount: 10_000_000_000,
            enqueued_at: 1_789_999_990,
        },
        InboxMsgV1 {
            kind: InboxKind::Deposit,
            index: 1,
            lane_account: pk(OWNER_B),
            amount: 10_000_000_000,
            enqueued_at: 1_789_999_995,
        },
        InboxMsgV1 {
            kind: InboxKind::ForcedWithdrawal,
            index: 2,
            lane_account: pk(OWNER_A),
            amount: 500_000_000,
            enqueued_at: 1_790_000_000,
        },
    ]
}

fn oracle_update() -> OracleUpdateV1 {
    let mut u = OracleUpdateV1 {
        market_id: 1,
        price: 6_500_000_000,
        publish_time_ms: 1_790_000_000_500,
        oracle_key: pk(ORACLE),
        signature: [0; 64],
    };
    u.signature = sign(ORACLE, &sha256(&u.signing_preimage(&lane_id())));
    u
}

fn block(height: u64, prev: [u8; 32], checkpoint_end: bool, entries: Vec<Entry>) -> BlockInputV1 {
    BlockInputV1 {
        lane_id: lane_id(),
        height,
        timestamp_ms: 1_790_000_000_000 + 1000 * height,
        prev_block_hash: prev,
        checkpoint_end,
        entries,
    }
}

fn block_1() -> BlockInputV1 {
    let m = inbox_messages();
    block(
        1,
        [0; 32],
        false,
        vec![
            Entry::Inbox(m[0]),
            Entry::Inbox(m[1]),
            Entry::Oracle(oracle_update()),
        ],
    )
}

fn block_1_record() -> BlockRecordV1 {
    BlockRecordV1 {
        input: block_1().encode().unwrap(),
        state_hash_after: sha256(b"fixture state after block 1"),
    }
}

fn block_hash(rec: &BlockRecordV1) -> [u8; 32] {
    sha256(&block_hash_preimage(
        &sha256(&rec.input),
        &rec.state_hash_after,
    ))
}

fn block_2() -> BlockInputV1 {
    block(
        2,
        block_hash(&block_1_record()),
        true,
        vec![
            Entry::Inbox(inbox_messages()[2]),
            Entry::User(place_order()),
        ],
    )
}

fn block_2_record() -> BlockRecordV1 {
    BlockRecordV1 {
        input: block_2().encode().unwrap(),
        state_hash_after: sha256(b"fixture state after block 2"),
    }
}

fn batch() -> BatchV1 {
    BatchV1 {
        lane_id: lane_id(),
        checkpoint_seq: 1,
        blocks: vec![block_1_record(), block_2_record()],
    }
}

fn signers() -> Vec<WeightedSigner> {
    let mut s: Vec<WeightedSigner> = VALIDATORS
        .iter()
        .map(|n| WeightedSigner {
            key: pk(*n),
            weight: 1,
        })
        .collect();
    s.sort_by(|a, b| a.key.cmp(&b.key));
    s
}

fn header() -> CheckpointHeaderV1 {
    let b2 = block_2_record();
    CheckpointHeaderV1 {
        lane_id: lane_id(),
        network_id: network_id(),
        settlement_addr_hash: sha256(b"fixture settlement address XDR"),
        engine_wasm_hash: sha256(b"fixture engine wasm"),
        seq: 1,
        prev_header_hash: [0; 32],
        first_block_height: 1,
        last_block_height: 2,
        last_block_timestamp_ms: block_2().timestamp_ms,
        last_block_hash: block_hash(&b2),
        batch_hash: sha256(&batch().encode().unwrap()),
        state_hash: b2.state_hash_after,
        accounts_root: sha256(b"fixture accounts root"),
        account_count: 4,
        escape_total: 19_500_000_000,
        withdrawals_root: sha256(b"fixture withdrawals root"),
        withdrawal_count: 1,
        withdrawals_total: 500_000_000,
        inbox_through: 3,
        inbox_acc: inbox_accs()[2],
    }
}

fn inbox_accs() -> [[u8; 32]; 3] {
    let mut acc = [0u8; 32];
    let mut out = [[0u8; 32]; 3];
    for (i, m) in inbox_messages().iter().enumerate() {
        acc = sha256(&inbox_acc_preimage(&acc, &m.encode()));
        out[i] = acc;
    }
    out
}

// ---------------------------------------------------------------------------
// Field projections.

fn j_tx(t: &LaneTxV1) -> J {
    let body = match t.body {
        TxBody::PlaceOrder(o) => obj(vec![
            ("market_id", s(o.market_id)),
            ("side", s(o.side as u8)),
            ("tif", s(o.tif as u8)),
            ("reduce_only", J::B(o.reduce_only)),
            ("price", s(o.price)),
            ("lots", s(o.lots)),
            ("client_order_id", s(o.client_order_id)),
        ]),
        TxBody::CancelOrder {
            market_id,
            order_id,
        } => obj(vec![("market_id", s(market_id)), ("order_id", s(order_id))]),
        TxBody::CancelAll { market_id } => obj(vec![("market_id", s(market_id))]),
        TxBody::Withdraw { amount } => obj(vec![("amount", s(amount))]),
        TxBody::AddSessionKey {
            session_key,
            expires_at_ms,
            permissions,
        } => obj(vec![
            ("session_key", h(&session_key)),
            ("expires_at_ms", s(expires_at_ms)),
            ("permissions", s(permissions)),
        ]),
        TxBody::RevokeSessionKey { session_key } => obj(vec![("session_key", h(&session_key))]),
    };
    let tx_hash = sha256(&t.tx_hash_preimage(&config_hash()));
    obj(vec![
        ("version", s(1)),
        ("lane_id", h(&t.lane_id)),
        ("account", h(&t.account)),
        ("signer", h(&t.signer)),
        ("nonce", s(t.nonce)),
        ("expiry_ms", s(t.expiry_ms)),
        ("kind", s(t.kind() as u8)),
        ("kind_name", s(t.kind().name())),
        ("sig_scheme", s(t.sig_scheme as u8)),
        ("body_len", s(t.kind().body_len())),
        ("body", body),
        ("signature", h(&t.signature)),
        ("signing_bytes", h(&t.signing_bytes())),
        ("tx_hash_preimage", h(&t.tx_hash_preimage(&config_hash()))),
        (
            "sep53_message",
            s(String::from_utf8(sep53_tx_message(&tx_hash).to_vec()).unwrap()),
        ),
        (
            "sep53_hash",
            h(&sha256(&sep53_preimage(&sep53_tx_message(&tx_hash)))),
        ),
    ])
}

fn j_inbox(m: &InboxMsgV1) -> J {
    obj(vec![
        ("kind", s(m.kind as u8)),
        ("index", s(m.index)),
        ("lane_account", h(&m.lane_account)),
        ("amount", s(m.amount)),
        ("enqueued_at", s(m.enqueued_at)),
    ])
}

fn j_oracle(u: &OracleUpdateV1) -> J {
    obj(vec![
        ("market_id", s(u.market_id)),
        ("price", s(u.price)),
        ("publish_time_ms", s(u.publish_time_ms)),
        ("oracle_key", h(&u.oracle_key)),
        ("signature", h(&u.signature)),
        ("signing_preimage", h(&u.signing_preimage(&lane_id()))),
    ])
}

fn j_entry(e: &Entry) -> J {
    let (t, f) = match e {
        Entry::Inbox(m) => ("1", j_inbox(m)),
        Entry::Oracle(u) => ("2", j_oracle(u)),
        Entry::User(tx) => (
            "3",
            obj(vec![("hex", h(&tx.encode())), ("kind", s(tx.kind() as u8))]),
        ),
    };
    obj(vec![
        ("entry_type", s(t)),
        ("len", s(e.payload_len())),
        ("payload", f),
    ])
}

fn j_block(b: &BlockInputV1) -> J {
    obj(vec![
        ("lane_id", h(&b.lane_id)),
        ("height", s(b.height)),
        ("timestamp_ms", s(b.timestamp_ms)),
        ("prev_block_hash", h(&b.prev_block_hash)),
        ("flags", s(b.flags())),
        ("entry_count", s(b.entries.len())),
        ("entries", J::A(b.entries.iter().map(j_entry).collect())),
    ])
}

fn j_header(c: &CheckpointHeaderV1) -> J {
    obj(vec![
        ("magic", s("CVCKPT01")),
        ("version", s(1)),
        ("lane_id", h(&c.lane_id)),
        ("network_id", h(&c.network_id)),
        ("settlement_addr_hash", h(&c.settlement_addr_hash)),
        ("engine_wasm_hash", h(&c.engine_wasm_hash)),
        ("seq", s(c.seq)),
        ("prev_header_hash", h(&c.prev_header_hash)),
        ("first_block_height", s(c.first_block_height)),
        ("last_block_height", s(c.last_block_height)),
        ("last_block_timestamp_ms", s(c.last_block_timestamp_ms)),
        ("last_block_hash", h(&c.last_block_hash)),
        ("batch_hash", h(&c.batch_hash)),
        ("state_hash", h(&c.state_hash)),
        ("accounts_root", h(&c.accounts_root)),
        ("account_count", s(c.account_count)),
        ("escape_total", s(c.escape_total)),
        ("withdrawals_root", h(&c.withdrawals_root)),
        ("withdrawal_count", s(c.withdrawal_count)),
        ("withdrawals_total", s(c.withdrawals_total)),
        ("inbox_through", s(c.inbox_through)),
        ("inbox_acc", h(&c.inbox_acc)),
    ])
}

fn j_market(m: &MarketParamsV1) -> J {
    obj(vec![
        ("market_id", s(m.market_id)),
        ("symbol", s(m.symbol_str())),
        ("tick", s(m.tick)),
        ("imf_bps", s(m.imf_bps)),
        ("mmf_bps", s(m.mmf_bps)),
        ("taker_fee_bps", s(m.taker_fee_bps)),
        ("maker_fee_bps", s(m.maker_fee_bps)),
        ("liq_fee_bps", s(m.liq_fee_bps)),
        ("band_bps", s(m.band_bps)),
        ("max_position_lots", s(m.max_position_lots)),
        ("max_oi_lots", s(m.max_oi_lots)),
        ("impact_lots", s(m.impact_lots)),
        ("display_lot_base_units", s(m.display_lot_base_units)),
        ("display_base_decimals", s(m.display_base_decimals)),
    ])
}

fn j_config(c: &GenesisConfigV1) -> J {
    obj(vec![
        ("lane_id", h(&c.lane_id)),
        ("backstop_key", h(&c.backstop_key)),
        ("treasury_key", h(&c.treasury_key)),
        ("access_mode", s(c.access_mode as u8)),
        (
            "allowlist",
            J::A(c.allowlist.iter().map(|k| h(k)).collect()),
        ),
        (
            "oracle_keys",
            J::A(c.oracle_keys.iter().map(|k| h(k)).collect()),
        ),
        ("oracle_max_staleness_ms", s(c.oracle_max_staleness_ms)),
        ("oracle_max_future_ms", s(c.oracle_max_future_ms)),
        (
            "oracle_circuit_breaker_bps",
            s(c.oracle_circuit_breaker_bps),
        ),
        (
            "oracle_breaker_bps_per_sec",
            s(c.oracle_breaker_bps_per_sec),
        ),
        ("funding_interval_ms", s(c.funding_interval_ms)),
        ("funding_damping", s(c.funding_damping)),
        ("funding_max_rate_ppm", s(c.funding_max_rate_ppm)),
        ("insurance_fee_share_bps", s(c.insurance_fee_share_bps)),
        ("min_deposit", s(c.min_deposit)),
        ("min_withdrawal", s(c.min_withdrawal)),
        ("max_accounts", s(c.max_accounts)),
        ("max_orders_per_side", s(c.max_orders_per_side)),
        (
            "max_open_orders_per_account",
            s(c.max_open_orders_per_account),
        ),
        ("max_session_keys", s(c.max_session_keys)),
        (
            "max_txs_per_account_per_block",
            s(c.max_txs_per_account_per_block),
        ),
        ("max_entries_per_block", s(c.max_entries_per_block)),
        ("max_block_bytes", s(c.max_block_bytes)),
        ("max_pending_withdrawals", s(c.max_pending_withdrawals)),
        ("exec_cpu_limit", s(c.exec_cpu_limit)),
        ("exec_mem_limit", s(c.exec_mem_limit)),
        ("markets", J::A(c.markets.iter().map(j_market).collect())),
    ])
}

fn j_event(e: &Event) -> J {
    let t = s(e.type_id());
    let f = match *e {
        Event::Fill {
            market,
            maker_order_id,
            maker_idx,
            taker_idx,
            price,
            lots,
            taker_side,
        } => obj(vec![
            ("market", s(market)),
            ("maker_order_id", s(maker_order_id)),
            ("maker_idx", s(maker_idx)),
            ("taker_idx", s(taker_idx)),
            ("price", s(price)),
            ("lots", s(lots)),
            ("taker_side", s(taker_side as u8)),
        ]),
        Event::OrderRested {
            market,
            order_id,
            account_idx,
            side,
            price,
            lots,
        } => obj(vec![
            ("market", s(market)),
            ("order_id", s(order_id)),
            ("account_idx", s(account_idx)),
            ("side", s(side as u8)),
            ("price", s(price)),
            ("lots", s(lots)),
        ]),
        Event::OrderCanceled {
            market,
            order_id,
            reason,
        } => obj(vec![
            ("market", s(market)),
            ("order_id", s(order_id)),
            ("reason", s(reason as u8)),
        ]),
        Event::Funding {
            market,
            rate_ppm,
            fpl,
        } => obj(vec![
            ("market", s(market)),
            ("rate_ppm", s(rate_ppm)),
            ("fpl", s(fpl)),
        ]),
        Event::Liquidation {
            account_idx,
            fee,
            deficit,
        } => obj(vec![
            ("account_idx", s(account_idx)),
            ("fee", s(fee)),
            ("deficit", s(deficit)),
        ]),
        Event::Deposit {
            key,
            amount,
            outcome,
        } => obj(vec![
            ("key", h(&key)),
            ("amount", s(amount)),
            ("outcome", s(outcome as u8)),
        ]),
        Event::ForcedWithdrawalProcessed { key, amount } => {
            obj(vec![("key", h(&key)), ("amount", s(amount))])
        }
        Event::Oracle {
            market,
            price,
            accepted,
        } => obj(vec![
            ("market", s(market)),
            ("price", s(price)),
            ("accepted", J::B(accepted)),
        ]),
        Event::BackstopDeficit {
            active,
            backstop_equity,
        } => obj(vec![
            ("active", J::B(active)),
            ("backstop_equity", s(backstop_equity)),
        ]),
        Event::Commitment {
            seq,
            withdrawals_total,
            escape_total,
        } => obj(vec![
            ("seq", s(seq)),
            ("withdrawals_total", s(withdrawals_total)),
            ("escape_total", s(escape_total)),
        ]),
    };
    obj(vec![("type", t), ("fields", f)])
}

// ---------------------------------------------------------------------------
// Files.

fn lane_tx_file() -> String {
    let owner = OWNER_A;
    let txs = [
        ("place_order_session_key", place_order()),
        (
            "cancel_order_session_key",
            base_tx(
                SESSION_A,
                1_790_000_000_001,
                SigScheme::RawEd25519,
                TxBody::CancelOrder {
                    market_id: 1,
                    order_id: 12,
                },
            ),
        ),
        (
            "cancel_all_markets",
            base_tx(
                SESSION_A,
                1_790_000_000_002,
                SigScheme::RawEd25519,
                TxBody::CancelAll {
                    market_id: ALL_MARKETS,
                },
            ),
        ),
        (
            "withdraw_sep53_owner",
            base_tx(
                owner,
                1_790_000_000_003,
                SigScheme::Sep53,
                TxBody::Withdraw {
                    amount: 100_000_000,
                },
            ),
        ),
        (
            "add_session_key_sep53_owner",
            base_tx(
                owner,
                1_790_000_000_004,
                SigScheme::Sep53,
                TxBody::AddSessionKey {
                    session_key: pk(SESSION_A),
                    expires_at_ms: 1_790_086_400_000,
                    permissions: PERM_ALL,
                },
            ),
        ),
        (
            "revoke_session_key_raw_owner",
            base_tx(
                owner,
                1_790_000_000_005,
                SigScheme::RawEd25519,
                TxBody::RevokeSessionKey {
                    session_key: pk(SESSION_A),
                },
            ),
        ),
    ];
    let mut valid = Vec::new();
    for (name, t) in &txs {
        let bytes = t.encode();
        assert_eq!(
            LaneTxV1::decode(&bytes).as_ref(),
            Ok(t),
            "{name} round-trips"
        );
        let tx_hash = sha256(&t.tx_hash_preimage(&config_hash()));
        let vk = VerifyingKey::from_bytes(&t.signer).unwrap();
        let signed_msg = match t.sig_scheme {
            SigScheme::RawEd25519 => tx_hash,
            SigScheme::Sep53 => sha256(&sep53_preimage(&sep53_tx_message(&tx_hash))),
        };
        assert!(
            vk.verify_strict(&signed_msg, &Signature::from_bytes(&t.signature))
                .is_ok(),
            "{name} signature"
        );
        valid.push(vector(name, j_tx(t), &bytes, &tx_hash));
    }

    let good = place_order().encode();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("bad_version", with_byte(&good, 0, 2)),
        ("unknown_kind", with_byte(&good, 113, 9)),
        ("bad_sig_scheme", with_byte(&good, 114, 2)),
        ("body_len_mismatch", with_byte(&good, 115, 28)),
        ("bad_side", with_byte(&good, 119, 2)),
        ("bad_tif", with_byte(&good, 120, 3)),
        ("bad_reduce_only", with_byte(&good, 121, 2)),
        ("trailing_byte", with_push(&good, 0)),
        ("truncated_signature", good[..good.len() - 1].to_vec()),
    ];
    let invalid_vectors = cases
        .iter()
        .map(|(name, bytes)| invalid(name, bytes, LaneTxV1::decode(bytes).expect_err("must fail")))
        .collect();

    file(
        "LaneTxV1",
        "§9.2, §9.3",
        "hash = tx_hash = H(TAG_TX || config_hash || bytes[0 .. 117+N]). sep53_hash is what a SEP-53 wallet signs for sig_scheme 1.",
        vec![("lane_name", s(LANE_NAME)), ("lane_id", h(&lane_id())), ("config_hash", h(&config_hash()))],
        valid,
        invalid_vectors,
    )
}

fn inbox_file() -> String {
    let msgs = inbox_messages();
    let accs = inbox_accs();
    let mut valid = Vec::new();
    let mut prev = [0u8; 32];
    for (i, m) in msgs.iter().enumerate() {
        let bytes = m.encode();
        assert_eq!(InboxMsgV1::decode(&bytes), Ok(*m));
        let mut fields = j_inbox(m);
        if let J::O(f) = &mut fields {
            f.push(("acc_before".to_string(), h(&prev)));
            f.push((
                "acc_preimage".to_string(),
                h(&inbox_acc_preimage(&prev, &bytes)),
            ));
            f.push(("acc_after".to_string(), h(&accs[i])));
        }
        valid.push(vector(&format!("msg_{i}"), fields, &bytes, &accs[i]));
        prev = accs[i];
    }
    let good = msgs[0].encode();
    let invalid_vectors = vec![
        invalid(
            "bad_kind",
            &with_byte(&good, 0, 2),
            InboxMsgV1::decode(&with_byte(&good, 0, 2)).unwrap_err(),
        ),
        invalid(
            "short",
            &good[..64],
            InboxMsgV1::decode(&good[..64]).unwrap_err(),
        ),
        invalid(
            "long",
            &with_push(&good, 0),
            InboxMsgV1::decode(&with_push(&good, 0)).unwrap_err(),
        ),
    ];
    file(
        "InboxMsgV1",
        "§9.4",
        "hash = acc_after = H(TAG_INBOX || acc_before || msg); acc_0 is 32 zero bytes. The messages form one chain.",
        vec![],
        valid,
        invalid_vectors,
    )
}

fn oracle_file() -> String {
    let u = oracle_update();
    let bytes = u.encode();
    assert_eq!(OracleUpdateV1::decode(&bytes), Ok(u));
    let msg = sha256(&u.signing_preimage(&lane_id()));
    assert!(VerifyingKey::from_bytes(&u.oracle_key)
        .unwrap()
        .verify_strict(&msg, &Signature::from_bytes(&u.signature))
        .is_ok());
    file(
        "OracleUpdateV1",
        "§9.5",
        "hash = H(TAG_ORACLE || lane_id || bytes[0..18]), the message the oracle key signs.",
        vec![("lane_id", h(&lane_id()))],
        vec![vector("btc_price", j_oracle(&u), &bytes, &msg)],
        vec![invalid(
            "short",
            &bytes[..113],
            OracleUpdateV1::decode(&bytes[..113]).unwrap_err(),
        )],
    )
}

fn block_file() -> String {
    let empty = block(1, [0; 32], false, Vec::new());
    let blocks = [
        ("empty_height_1", empty),
        ("inbox_and_oracle_height_1", block_1()),
        ("forced_withdrawal_and_order_checkpoint_end", block_2()),
    ];
    let mut valid = Vec::new();
    for (name, b) in &blocks {
        let bytes = b.encode().unwrap();
        assert_eq!(BlockInputV1::decode(&bytes).as_ref(), Ok(b));
        valid.push(vector(name, j_block(b), &bytes, &sha256(&bytes)));
    }
    let good = block_2().encode().unwrap();
    let oracle_block = block_1().encode().unwrap();
    // block_1: entries at 93: inbox (70), inbox (70), oracle at 233 (type) / 234 (len).
    let mut short_oracle = oracle_block.clone();
    short_oracle[234..238].copy_from_slice(&113u32.to_le_bytes());
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("bad_magic", with_byte(&good, 0, b'X')),
        ("reserved_flag_bit", with_byte(&good, 88, 0x02)),
        ("trailing_byte", with_push(&good, 0)),
        ("entry_2_len_mismatch", short_oracle),
        ("entry_1_unknown_type", with_byte(&good, 93 + 70, 9)),
        ("entry_count_too_high", {
            let mut b = good.clone();
            b[89..93].copy_from_slice(&3u32.to_le_bytes());
            b
        }),
    ];
    let invalid_vectors = cases
        .iter()
        .map(|(name, bytes)| {
            invalid(
                name,
                bytes,
                BlockInputV1::decode(bytes).expect_err("must fail"),
            )
        })
        .collect();
    file(
        "BlockInputV1",
        "§9.6",
        "hash = input_hash = H(BlockInputV1 bytes). Header errors map to fatal BAD_BLOCK_ENCODING, Entry errors to BAD_ENTRY_ENCODING with that entry index.",
        vec![("lane_id", h(&lane_id()))],
        valid,
        invalid_vectors,
    )
}

fn record_file() -> String {
    let mut valid = Vec::new();
    for (name, rec) in [("block_1", block_1_record()), ("block_2", block_2_record())] {
        let bytes = rec.encode();
        assert_eq!(BlockRecordV1::decode(&bytes).as_ref(), Ok(&rec));
        let input_hash = sha256(&rec.input);
        let fields = obj(vec![
            ("input_hash", h(&input_hash)),
            ("state_hash_after", h(&rec.state_hash_after)),
            (
                "block_hash_preimage",
                h(&block_hash_preimage(&input_hash, &rec.state_hash_after)),
            ),
        ]);
        valid.push(vector(name, fields, &bytes, &block_hash(&rec)));
    }
    file(
        "BlockRecordV1",
        "§9.6",
        "hash = block_hash = H(input_hash || state_hash_after). block_2.prev_block_hash equals block_1's block_hash.",
        vec![],
        valid,
        vec![],
    )
}

fn batch_file() -> String {
    let b = batch();
    let bytes = b.encode().unwrap();
    assert_eq!(BatchV1::decode(&bytes).as_ref(), Ok(&b));
    let fields = obj(vec![
        ("lane_id", h(&b.lane_id)),
        ("checkpoint_seq", s(b.checkpoint_seq)),
        ("block_count", s(b.blocks.len())),
        (
            "records",
            J::A(b.blocks.iter().map(|r| h(&r.encode())).collect()),
        ),
    ]);
    file(
        "BatchV1",
        "§9.7",
        "hash = batch_hash = H(BatchV1 bytes).",
        vec![],
        vec![vector("seq_1_two_blocks", fields, &bytes, &sha256(&bytes))],
        vec![invalid(
            "trailing_byte",
            &with_push(&bytes, 0),
            BatchV1::decode(&with_push(&bytes, 0)).unwrap_err(),
        )],
    )
}

fn header_file() -> String {
    let c = header();
    let bytes = c.encode();
    assert_eq!(CheckpointHeaderV1::decode(&bytes), Ok(c));
    let header_hash = sha256(&bytes);
    let signers = signers();
    let sigs: Vec<J> = signers
        .iter()
        .enumerate()
        .map(|(i, sgn)| {
            let seed = VALIDATORS
                .iter()
                .copied()
                .find(|n| pk(*n) == sgn.key)
                .unwrap();
            obj(vec![
                ("signer_index", s(i)),
                ("key", h(&sgn.key)),
                ("signature", h(&sign(seed, &header_hash))),
            ])
        })
        .collect();
    let mut fields = j_header(&c);
    if let J::O(f) = &mut fields {
        f.push(("signatures".to_string(), J::A(sigs)));
    }
    let mut bad_len = bytes.to_vec();
    bad_len.pop();
    file(
        "CheckpointHeaderV1",
        "§9.8",
        "hash = header_hash = H(header bytes). Validators sign the raw 32-byte header_hash with ed25519. signer_index follows the signer set sorted by key (§13.1).",
        vec![("signers_threshold", s(2))],
        vec![vector("seq_1", fields, &bytes, &header_hash)],
        vec![
            invalid("version_2", &with_byte(&bytes, 8, 2), CheckpointHeaderV1::decode(&with_byte(&bytes, 8, 2)).unwrap_err()),
            invalid("441_bytes", &bad_len, CheckpointHeaderV1::decode(&bad_len).unwrap_err()),
        ],
    )
}

fn config_file() -> String {
    let c = config();
    assert_eq!(c.validate(), Ok(()));
    let bytes = c.encode().unwrap();
    assert_eq!(GenesisConfigV1::decode(&bytes).as_ref(), Ok(&c));
    file(
        "GenesisConfigV1",
        "§10.2",
        "hash = config_hash = H(GenesisConfigV1 bytes). Keys are fixture keys, not the deployed testnet keys.",
        vec![("lane_name", s(LANE_NAME))],
        vec![vector("section_10_3_markets", j_config(&c), &bytes, &sha256(&bytes))],
        vec![
            invalid("bad_access_mode", &with_byte(&bytes, 104, 2), GenesisConfigV1::decode(&with_byte(&bytes, 104, 2)).unwrap_err()),
            invalid("trailing_byte", &with_push(&bytes, 0), GenesisConfigV1::decode(&with_push(&bytes, 0)).unwrap_err()),
        ],
    )
}

fn state() -> StateV1 {
    let cfg = config();
    let n = cfg.markets.len();
    let acct = |key: [u8; 32], system: bool, collateral: i128| AccountV1 {
        key,
        system,
        next_nonce: 0,
        collateral,
        open_order_count: 0,
        session_keys: Vec::new(),
        positions: vec![PositionV1::default(); n],
        txs_this_block: 0,
    };
    let mut a = acct(pk(OWNER_A), false, 9_500_000_000);
    a.next_nonce = 1_790_000_000_001;
    a.open_order_count = 1;
    a.session_keys = vec![SessionKeyV1 {
        key: pk(SESSION_A),
        expires_at_ms: 1_790_086_400_000,
        permissions: PERM_ALL,
    }];
    let mut markets = vec![MarketStateV1::default(); n];
    markets[0].oracle_price = 6_500_000_000;
    markets[0].oracle_time_ms = 1_790_000_000_500;
    markets[0].bids = vec![OrderV1 {
        order_id: 1,
        account_index: 2,
        price: 6_500_000_000,
        lots_remaining: 250,
        client_order_id: 7,
    }];
    StateV1 {
        lane_id: lane_id(),
        config_hash: config_hash(),
        height: 2,
        last_block_input_hash: sha256(&block_2().encode().unwrap()),
        last_timestamp_ms: block_2().timestamp_ms,
        checkpoint_seq: 1,
        inbox_through: 3,
        inbox_acc: inbox_accs()[2],
        next_order_id: 2,
        deposits_credited_total: 20_000_000_000,
        withdrawals_committed_total: 500_000_000,
        backstop_deficit: false,
        accounts: vec![
            acct(pk(BACKSTOP), true, 0),
            acct(pk(TREASURY), true, 0),
            a,
            acct(pk(OWNER_B), false, 10_000_000_000),
        ],
        markets,
        pending: vec![PendingWithdrawalV1 {
            key: pk(OWNER_A),
            amount: 100_000_000,
        }],
        last_commitment: CommitmentV1 {
            seq: 1,
            last_block_height: 2,
            account_count: 4,
            inbox_through: 3,
            inbox_acc: inbox_accs()[2],
            ..CommitmentV1::default()
        },
        config: cfg,
    }
}

fn state_file() -> String {
    let st = state();
    let bytes = st.encode().unwrap();
    assert_eq!(StateV1::decode(&bytes).as_ref(), Ok(&st));
    let flags_at = 8 + 32 + 32 + 8 + 32 + 8 + 8 + 8 + 32 + 8 + 16 + 16;
    let fields = obj(vec![
        ("height", s(st.height)),
        ("account_count", s(st.accounts.len())),
        ("market_count", s(st.markets.len())),
        ("pending_count", s(st.pending.len())),
        ("flags_offset", s(flags_at)),
    ]);
    file(
        "StateV1",
        "§9.10",
        "hash = state_hash = H(StateV1 bytes). A synthetic state for codec tests; engine genesis vectors come with T-003.",
        vec![],
        vec![vector("synthetic_two_users", fields, &bytes, &sha256(&bytes))],
        vec![invalid("reserved_flag_bit", &with_byte(&bytes, flags_at, 0x02), StateV1::decode(&with_byte(&bytes, flags_at, 0x02)).unwrap_err())],
    )
}

fn receipts() -> Receipts {
    let ev = |e| vec![e];
    Receipts {
        receipts: vec![
            Receipt {
                entry_index: PSEUDO_BLOCK_START,
                code: codes::OK,
                events: ev(Event::Funding {
                    market: 1,
                    rate_ppm: -62,
                    fpl: -403,
                }),
            },
            Receipt {
                entry_index: 0,
                code: codes::OK,
                events: ev(Event::Deposit {
                    key: pk(OWNER_A),
                    amount: 10_000_000_000,
                    outcome: DepositOutcome::Created,
                }),
            },
            Receipt {
                entry_index: 1,
                code: codes::OK,
                events: ev(Event::ForcedWithdrawalProcessed {
                    key: pk(OWNER_A),
                    amount: 500_000_000,
                }),
            },
            Receipt {
                entry_index: 2,
                code: codes::OK,
                events: ev(Event::Oracle {
                    market: 1,
                    price: 6_500_000_000,
                    accepted: true,
                }),
            },
            Receipt {
                entry_index: 3,
                code: codes::OK,
                events: vec![
                    Event::Fill {
                        market: 1,
                        maker_order_id: 1,
                        maker_idx: 3,
                        taker_idx: 2,
                        price: 6_500_000_000,
                        lots: 100,
                        taker_side: Side::Buy,
                    },
                    Event::OrderRested {
                        market: 1,
                        order_id: 2,
                        account_idx: 2,
                        side: Side::Buy,
                        price: 6_500_000_000,
                        lots: 150,
                    },
                    Event::OrderCanceled {
                        market: 1,
                        order_id: 1,
                        reason: CancelReason::SelfTrade,
                    },
                ],
            },
            Receipt {
                entry_index: 4,
                code: codes::INSUFFICIENT_MARGIN,
                events: Vec::new(),
            },
            Receipt {
                entry_index: PSEUDO_BLOCK_END,
                code: codes::OK,
                events: vec![
                    Event::Liquidation {
                        account_idx: 3,
                        fee: 650_000,
                        deficit: 0,
                    },
                    Event::BackstopDeficit {
                        active: false,
                        backstop_equity: 1,
                    },
                    Event::Commitment {
                        seq: 1,
                        withdrawals_total: 500_000_000,
                        escape_total: 19_500_000_000,
                    },
                ],
            },
        ],
    }
}

fn receipts_file() -> String {
    let rs = receipts();
    let bytes = rs.encode().unwrap();
    assert_eq!(Receipts::decode(&bytes).as_ref(), Ok(&rs));
    let fields = J::A(
        rs.receipts
            .iter()
            .map(|r| {
                obj(vec![
                    ("entry_index", s(r.entry_index)),
                    ("status", s(u8::from(r.rejected()))),
                    ("code", s(r.code)),
                    ("code_name", s(codes::name(r.code).unwrap())),
                    ("events", J::A(r.events.iter().map(j_event).collect())),
                ])
            })
            .collect(),
    );
    let mismatch = {
        let one = Receipts {
            receipts: vec![Receipt {
                entry_index: 0,
                code: codes::BAD_NONCE,
                events: Vec::new(),
            }],
        }
        .encode()
        .unwrap();
        with_byte(&one, 16, 0)
    };
    file(
        "Receipts",
        "§11.10",
        "hash = H(receipts bytes). Receipts are informational: part of the parity gate, not of checkpoints.",
        vec![],
        vec![vector("every_event_type", fields, &bytes, &sha256(&bytes))],
        vec![invalid("status_ok_with_reject_code", &mismatch, Receipts::decode(&mismatch).unwrap_err())],
    )
}

fn step_file() -> String {
    let env = StepEnvelope {
        state: state().encode().unwrap(),
        receipts: receipts().encode().unwrap(),
    };
    let bytes = env.encode().unwrap();
    assert_eq!(StepEnvelope::decode(&bytes).as_ref(), Ok(&env));
    let fields = obj(vec![
        ("state_len", s(env.state.len())),
        ("state_hash", h(&sha256(&env.state))),
        ("receipts_len", s(env.receipts.len())),
        ("receipts_hash", h(&sha256(&env.receipts))),
    ]);
    file(
        "StepEnvelope",
        "§12.1",
        "hash = H(envelope bytes). The engine contract's step returns this: CVSTEP01 · state_len · state · receipts_len · receipts.",
        vec![],
        vec![vector("state_and_receipts", fields, &bytes, &sha256(&bytes))],
        vec![],
    )
}

fn hashes_file() -> String {
    let lane = lane_id_preimage(LANE_NAME);
    let engine = engine_version_preimage(SPEC_VERSION);
    let acct = account_leaf_preimage(&lane_id(), 1, 2, &pk(OWNER_A), 9_500_000_000);
    let wdl = withdrawal_leaf_preimage(&lane_id(), 1, 0, &pk(OWNER_A), 500_000_000);
    let signers = signers();
    let signers_pre = signers_hash_preimage(&signers, 2).unwrap();
    let signers_hash = sha256(&signers_pre);
    let rotate = rotate_message_preimage(
        &lane_id(),
        &network_id(),
        &header().settlement_addr_hash,
        2,
        &signers_hash,
    );
    let rec = block_1_record();
    let bh = block_hash_preimage(&sha256(&rec.input), &rec.state_hash_after);
    let t = place_order();
    let tx_hash = sha256(&t.tx_hash_preimage(&config_hash()));
    let sep53_msg = sep53_tx_message(&tx_hash);
    let sep53 = sep53_preimage(&sep53_msg);
    let net = TESTNET_PASSPHRASE.as_bytes();
    let v = |name: &str, fields: J, pre: &[u8]| vector(name, fields, pre, &sha256(pre));
    file(
        "hash preimages",
        "§9.1, §9.2, §9.6, §11.8, §12.1, §13.4",
        "hash = H(hex): each hex is the exact preimage.",
        vec![],
        vec![
            v("lane_id", obj(vec![("lane_name", s(LANE_NAME))]), &lane),
            v(
                "network_id_testnet",
                obj(vec![("passphrase", s(TESTNET_PASSPHRASE))]),
                net,
            ),
            v(
                "engine_version",
                obj(vec![("spec_version", s(SPEC_VERSION))]),
                &engine,
            ),
            v(
                "account_leaf",
                obj(vec![
                    ("lane_id", h(&lane_id())),
                    ("seq", s(1)),
                    ("index", s(2)),
                    ("key", h(&pk(OWNER_A))),
                    ("escape_equity", s(9_500_000_000i64)),
                ]),
                &acct,
            ),
            v(
                "withdrawal_leaf",
                obj(vec![
                    ("lane_id", h(&lane_id())),
                    ("seq", s(1)),
                    ("index", s(0)),
                    ("key", h(&pk(OWNER_A))),
                    ("amount", s(500_000_000)),
                ]),
                &wdl,
            ),
            v(
                "signers_hash_3_of_weight_1_threshold_2",
                obj(vec![
                    ("keys", J::A(signers.iter().map(|x| h(&x.key)).collect())),
                    (
                        "weights",
                        J::A(signers.iter().map(|x| s(x.weight)).collect()),
                    ),
                    ("threshold", s(2)),
                ]),
                &signers_pre,
            ),
            v(
                "rotate_message_epoch_2",
                obj(vec![
                    ("lane_id", h(&lane_id())),
                    ("network_id", h(&network_id())),
                    ("settlement_addr_hash", h(&header().settlement_addr_hash)),
                    ("new_epoch", s(2)),
                    ("signers_hash", h(&signers_hash)),
                ]),
                &rotate,
            ),
            v(
                "block_hash",
                obj(vec![
                    ("input_hash", h(&sha256(&rec.input))),
                    ("state_hash_after", h(&rec.state_hash_after)),
                ]),
                &bh,
            ),
            v(
                "sep53_lane_tx",
                obj(vec![
                    ("tx_hash", h(&tx_hash)),
                    ("message", s(String::from_utf8(sep53_msg.to_vec()).unwrap())),
                ]),
                &sep53,
            ),
        ],
        vec![],
    )
}

/// ed25519 edge cases for the §8.4 parity check: the native `Crypto` and the
/// host MUST agree on every one of these.
fn signatures_file() -> String {
    const L: [u8; 32] = [
        0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde,
        0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
    ];
    let msg = sha256(b"caravel ed25519 edge cases");
    let valid_sig = sign(OWNER_A, &msg);

    // S' = S + L (little-endian add). S < L, so S' < 2L < 2^254 fits in 32 bytes.
    let mut malleable = valid_sig;
    let mut carry = 0u16;
    for i in 0..32 {
        let sum = u16::from(malleable[32 + i]) + u16::from(L[i]) + carry;
        malleable[32 + i] = (sum & 0xFF) as u8;
        carry = sum >> 8;
    }
    assert_eq!(carry, 0);

    // The identity point (y = 1) as key and R, with S = 0: passes a cofactorless
    // check, fails `verify_strict` (small-order key and R).
    let mut identity = [0u8; 32];
    identity[0] = 1;
    let mut small_order_sig = [0u8; 64];
    small_order_sig[0] = 1;

    let cases = [
        ("valid", pk(OWNER_A), valid_sig),
        ("non_canonical_s_plus_l", pk(OWNER_A), malleable),
        ("small_order_key_identity", identity, small_order_sig),
    ];
    let vectors = cases
        .iter()
        .map(|(name, key, sig)| {
            let vk = VerifyingKey::from_bytes(key);
            let sig_obj = Signature::from_bytes(sig);
            let strict = vk
                .as_ref()
                .map(|k| k.verify_strict(&msg, &sig_obj).is_ok())
                .unwrap_or(false);
            let lax = vk
                .as_ref()
                .map(|k| k.verify(&msg, &sig_obj).is_ok())
                .unwrap_or(false);
            let fields = obj(vec![
                ("public_key", h(key)),
                ("message", h(&msg)),
                ("signature", h(sig)),
                ("verify_strict", J::B(strict)),
                ("verify", J::B(lax)),
            ]);
            let mut all = Vec::new();
            all.extend_from_slice(key);
            all.extend_from_slice(&msg);
            all.extend_from_slice(sig);
            vector(name, fields, &all, &sha256(&all))
        })
        .collect();
    file(
        "ed25519 edge cases",
        "§8.4",
        "hex = public_key || message || signature; hash = H(hex). verify_strict is what soroban-env-host 28.0.2 uses; the native Crypto MUST match it on every case.",
        vec![("dalek", s("ed25519-dalek 2.2.0"))],
        vectors,
        vec![],
    )
}

/// Every vector file, in a fixed order.
pub fn all() -> Vec<VectorFile> {
    vec![
        VectorFile {
            name: "hashes.json",
            contents: hashes_file(),
        },
        VectorFile {
            name: "lane_tx.json",
            contents: lane_tx_file(),
        },
        VectorFile {
            name: "inbox_msg.json",
            contents: inbox_file(),
        },
        VectorFile {
            name: "oracle_update.json",
            contents: oracle_file(),
        },
        VectorFile {
            name: "block_input.json",
            contents: block_file(),
        },
        VectorFile {
            name: "block_record.json",
            contents: record_file(),
        },
        VectorFile {
            name: "batch.json",
            contents: batch_file(),
        },
        VectorFile {
            name: "checkpoint_header.json",
            contents: header_file(),
        },
        VectorFile {
            name: "genesis_config.json",
            contents: config_file(),
        },
        VectorFile {
            name: "state.json",
            contents: state_file(),
        },
        VectorFile {
            name: "receipts.json",
            contents: receipts_file(),
        },
        VectorFile {
            name: "step_envelope.json",
            contents: step_file(),
        },
        VectorFile {
            name: "signatures.json",
            contents: signatures_file(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-vectors")
    }

    #[test]
    fn committed_vectors_are_current() {
        for f in all() {
            let path = dir().join(f.name);
            let on_disk = std::fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("missing {}: run `cargo gen-vectors`", path.display()));
            assert!(on_disk == f.contents, "{} is stale: run `cargo gen-vectors` (and record a DEC if a frozen format changed)", f.name);
        }
    }

    #[test]
    fn ed25519_edge_cases_behave_as_documented() {
        let text = signatures_file();
        // valid passes strict; the other two fail it.
        assert_eq!(text.matches("\"verify_strict\": true").count(), 1);
        assert_eq!(text.matches("\"verify_strict\": false").count(), 2);
    }

    #[test]
    fn lane_id_and_network_id_are_the_m0_values() {
        assert_eq!(
            hex(&lane_id()),
            hex(&sha256(b"CARAVEL/LANE/V1caravel-perps-testnet-0"))
        );
        assert_eq!(network_id(), sha256(b"Test SDF Network ; September 2015"));
    }
}
