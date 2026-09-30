//! Read-only JSON views for the node APIs (spec §14.4, §15), for any app:
//! blocks, checkpoint headers and proofs. JSON uses lowercase hex for bytes,
//! decimal strings for i128/u64 amounts and `G...` strkeys. An app adds its
//! own views (perps: accounts, markets, books, fills). Nothing here is
//! consensus.

use caravel_core::block::{BlockInputV1, BlockRecordV1, Entry};
use caravel_core::checkpoint::CheckpointHeaderV1;
use caravel_core::receipts::ReceiptsV1;
use serde::Serialize;

use crate::app::LaneApp;
use crate::checkpoint::{self, Leaf};
use crate::sequencer::hex;

/// `G...` for a raw ed25519 key.
pub fn g_address(key: &[u8; 32]) -> String {
    stellar_strkey::ed25519::PublicKey(*key)
        .to_string()
        .as_str()
        .to_owned()
}

/// A raw key from `G...`.
pub fn parse_g(s: &str) -> Option<[u8; 32]> {
    stellar_strkey::ed25519::PublicKey::from_string(s)
        .ok()
        .map(|k| k.0)
}

#[derive(Serialize, Clone, Debug)]
pub struct EntryView {
    pub index: u32,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub hex: String,
    pub code: Option<u16>,
    pub events: Vec<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct BlockView {
    pub height: String,
    pub timestamp_ms: String,
    pub checkpoint_end: bool,
    pub prev_block_hash: String,
    pub block_hash: String,
    pub state_hash_after: String,
    pub record_hex: String,
    pub receipts_hex: String,
    pub entries: Vec<EntryView>,
}

pub fn block<A: LaneApp>(
    app: &A,
    record: &BlockRecordV1,
    receipts_bytes: &[u8],
) -> Option<BlockView> {
    let input = BlockInputV1::decode(&record.input).ok()?;
    if !app.receipts_decode(receipts_bytes) {
        return None;
    }
    let receipts = ReceiptsV1::decode(receipts_bytes).ok()?;
    let events = app.render_events(receipts_bytes)?;
    let entries = input
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let rc = receipts
                .receipts
                .iter()
                .position(|r| r.entry_index == i as u32);
            let kind = match e {
                Entry::Inbox(_) => "inbox",
                Entry::Feed(_) => app.feed_label(),
                Entry::User(_) => "user",
            };
            EntryView {
                index: i as u32,
                kind,
                hex: hex(&e.payload().unwrap_or_default()),
                code: rc.map(|r| receipts.receipts[r].code),
                events: rc.and_then(|r| events.get(r).cloned()).unwrap_or_default(),
            }
        })
        .collect();
    let record_bytes = record.encode();
    Some(BlockView {
        height: input.height.to_string(),
        timestamp_ms: input.timestamp_ms.to_string(),
        checkpoint_end: input.checkpoint_end,
        prev_block_hash: hex(&input.prev_block_hash),
        block_hash: hex(&checkpoint::block_hash(record)),
        state_hash_after: hex(&record.state_hash_after),
        record_hex: hex(&record_bytes),
        receipts_hex: hex(receipts_bytes),
        entries,
    })
}

#[derive(Serialize, Clone, Debug)]
pub struct HeaderView {
    pub seq: String,
    pub lane_id: String,
    pub network_id: String,
    pub settlement_addr_hash: String,
    pub engine_wasm_hash: String,
    pub prev_header_hash: String,
    pub first_block_height: String,
    pub last_block_height: String,
    pub last_block_timestamp_ms: String,
    pub last_block_hash: String,
    pub batch_hash: String,
    pub state_hash: String,
    pub accounts_root: String,
    pub account_count: u32,
    pub escape_total: String,
    pub withdrawals_root: String,
    pub withdrawal_count: u32,
    pub withdrawals_total: String,
    pub inbox_through: String,
    pub inbox_acc: String,
}

pub fn header(h: &CheckpointHeaderV1) -> HeaderView {
    HeaderView {
        seq: h.seq.to_string(),
        lane_id: hex(&h.lane_id),
        network_id: hex(&h.network_id),
        settlement_addr_hash: hex(&h.settlement_addr_hash),
        engine_wasm_hash: hex(&h.engine_wasm_hash),
        prev_header_hash: hex(&h.prev_header_hash),
        first_block_height: h.first_block_height.to_string(),
        last_block_height: h.last_block_height.to_string(),
        last_block_timestamp_ms: h.last_block_timestamp_ms.to_string(),
        last_block_hash: hex(&h.last_block_hash),
        batch_hash: hex(&h.batch_hash),
        state_hash: hex(&h.state_hash),
        accounts_root: hex(&h.accounts_root),
        account_count: h.account_count,
        escape_total: h.escape_total.to_string(),
        withdrawals_root: hex(&h.withdrawals_root),
        withdrawal_count: h.withdrawal_count,
        withdrawals_total: h.withdrawals_total.to_string(),
        inbox_through: h.inbox_through.to_string(),
        inbox_acc: hex(&h.inbox_acc),
    }
}

/// One claimable leaf with its proof (`/v1/proofs/*`).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ProofView {
    pub seq: String,
    pub index: u32,
    pub account: String,
    /// The withdrawal amount, or the escape equity.
    pub amount: String,
    pub proof: Vec<String>,
}

/// Proofs for every leaf of `account` among `leaves` of checkpoint `header`.
pub fn proofs_for(
    header: &CheckpointHeaderV1,
    leaves: &[Leaf],
    hashes: &[[u8; 32]],
    account: &[u8; 32],
) -> Vec<ProofView> {
    proofs_where(header, leaves, hashes, |l| l.key == *account)
}

/// Proofs for every leaf of checkpoint `header`.
pub fn all_proofs(
    header: &CheckpointHeaderV1,
    leaves: &[Leaf],
    hashes: &[[u8; 32]],
) -> Vec<ProofView> {
    proofs_where(header, leaves, hashes, |_| true)
}

fn proofs_where(
    header: &CheckpointHeaderV1,
    leaves: &[Leaf],
    hashes: &[[u8; 32]],
    keep: impl Fn(&Leaf) -> bool,
) -> Vec<ProofView> {
    leaves
        .iter()
        .filter(|l| keep(l))
        .filter_map(|l| {
            let proof = checkpoint::proof(hashes, l.index)?;
            Some(ProofView {
                seq: header.seq.to_string(),
                index: l.index,
                account: g_address(&l.key),
                amount: l.amount.to_string(),
                proof: proof.iter().map(|p| hex(p)).collect(),
            })
        })
        .collect()
}
