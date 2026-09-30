//! Read-only JSON views for the node APIs (spec §14.4, §15): accounts,
//! markets, books, fills, blocks, checkpoints and proofs. JSON uses lowercase
//! hex for bytes, decimal strings for i128/u64 amounts and `G...` strkeys.
//! Nothing here is consensus.

use caravel_perps::margin::{account_margin, upnl};
use caravel_types::block::{BlockInputV1, BlockRecordV1, Entry};
use caravel_types::checkpoint::CheckpointHeaderV1;
use caravel_types::receipts::{Event, Receipts};
use caravel_types::state::StateV1;
use caravel_types::tx::Side;
use serde::Serialize;

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

fn side(s: Side) -> &'static str {
    match s {
        Side::Buy => "buy",
        Side::Sell => "sell",
    }
}

pub fn symbol(raw: &[u8; 16]) -> String {
    String::from_utf8_lossy(raw)
        .trim_end_matches('\0')
        .to_string()
}

#[derive(Serialize, Clone, Debug)]
pub struct PositionView {
    pub market_id: u16,
    pub symbol: String,
    pub lots: i64,
    pub cost_basis: String,
    /// `cost_basis / lots`, per lot, in stroops (floored).
    pub entry_price: Option<String>,
    pub mark_price: String,
    pub upnl: String,
    /// Estimate: the oracle price at which this position alone would bring
    /// equity down to maintenance margin, others held fixed. Display only.
    pub liq_price: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct OrderView {
    pub market_id: u16,
    pub order_id: String,
    pub side: &'static str,
    pub price: String,
    pub lots_remaining: i64,
    pub client_order_id: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct SessionKeyView {
    pub key: String,
    pub expires_at_ms: String,
    pub permissions: u8,
}

#[derive(Serialize, Clone, Debug)]
pub struct AccountView {
    pub account: String,
    pub index: u32,
    pub system: bool,
    pub collateral: String,
    pub equity: String,
    pub free_collateral: String,
    pub initial_margin: String,
    pub maintenance_margin: String,
    pub next_nonce: String,
    pub positions: Vec<PositionView>,
    pub open_orders: Vec<OrderView>,
    pub session_keys: Vec<SessionKeyView>,
}

pub fn account(st: &StateV1, key: &[u8; 32]) -> Option<AccountView> {
    let a = st.accounts.iter().position(|x| x.key == *key)?;
    let acct = &st.accounts[a];
    let margin = account_margin(st, a, None).ok()?;
    let mut positions = Vec::new();
    for (m, p) in acct.positions.iter().enumerate() {
        if p.lots == 0 {
            continue;
        }
        let params = &st.config.markets[m];
        let mark = st.markets[m].oracle_price;
        let u = upnl(st, a, m).ok()?;
        let mm_m = mm(p.lots, mark, params.mmf_bps);
        let liq = liq_price(
            margin.equity,
            margin.maintenance - mm_m,
            p.lots,
            mark,
            params.mmf_bps,
        );
        positions.push(PositionView {
            market_id: params.market_id,
            symbol: symbol(&params.symbol),
            lots: p.lots,
            cost_basis: p.cost_basis.to_string(),
            entry_price: (p.lots != 0)
                .then(|| (p.cost_basis.div_euclid(i128::from(p.lots))).to_string()),
            mark_price: mark.to_string(),
            upnl: u.to_string(),
            liq_price: liq.map(|x| x.to_string()),
        });
    }
    let mut open_orders = Vec::new();
    for (m, market) in st.markets.iter().enumerate() {
        let market_id = st.config.markets[m].market_id;
        for (orders, s) in [(&market.bids, Side::Buy), (&market.asks, Side::Sell)] {
            for o in orders.iter().filter(|o| o.account_index as usize == a) {
                open_orders.push(OrderView {
                    market_id,
                    order_id: o.order_id.to_string(),
                    side: side(s),
                    price: o.price.to_string(),
                    lots_remaining: o.lots_remaining,
                    client_order_id: o.client_order_id.to_string(),
                });
            }
        }
    }
    Some(AccountView {
        account: g_address(key),
        index: a as u32,
        system: acct.system,
        collateral: acct.collateral.to_string(),
        equity: margin.equity.to_string(),
        free_collateral: margin.free_collateral().ok()?.to_string(),
        initial_margin: margin.initial.to_string(),
        maintenance_margin: margin.maintenance.to_string(),
        next_nonce: acct.next_nonce.to_string(),
        positions,
        open_orders,
        session_keys: acct
            .session_keys
            .iter()
            .map(|k| SessionKeyView {
                key: g_address(&k.key),
                expires_at_ms: k.expires_at_ms.to_string(),
                permissions: k.permissions,
            })
            .collect(),
    })
}

fn mm(lots: i64, price: i64, mmf_bps: u16) -> i128 {
    (i128::from(lots).abs() * i128::from(price) * i128::from(mmf_bps) + 9_999) / 10_000
}

/// Solves `E + s·(P − M) = MM_other + |s|·P·mmf/10⁴` for `P`.
fn liq_price(equity: i128, mm_other: i128, lots: i64, mark: i64, mmf_bps: u16) -> Option<i128> {
    let s = i128::from(lots);
    let num = (mm_other - equity + s * i128::from(mark)).checked_mul(10_000)?;
    let den = s * 10_000 - s.abs() * i128::from(mmf_bps);
    if den == 0 {
        return None;
    }
    let p = num.div_euclid(den);
    (p > 0).then_some(p)
}

#[derive(Serialize, Clone, Debug)]
pub struct MarketView {
    pub market_id: u16,
    pub symbol: String,
    pub tick: String,
    pub imf_bps: u16,
    pub mmf_bps: u16,
    pub taker_fee_bps: u16,
    pub maker_fee_bps: u16,
    pub liq_fee_bps: u16,
    pub band_bps: u16,
    pub max_position_lots: i64,
    pub max_oi_lots: i64,
    pub impact_lots: i64,
    pub display_lot_base_units: i64,
    pub display_base_decimals: u8,
    pub oracle_price: String,
    pub oracle_time_ms: String,
    pub last_funding_time_ms: String,
    pub cumulative_funding_per_lot: String,
    pub open_interest_lots: i64,
    pub best_bid: Option<String>,
    pub best_ask: Option<String>,
}

pub fn markets(st: &StateV1) -> Vec<MarketView> {
    st.config
        .markets
        .iter()
        .zip(&st.markets)
        .map(|(p, m)| MarketView {
            market_id: p.market_id,
            symbol: symbol(&p.symbol),
            tick: p.tick.to_string(),
            imf_bps: p.imf_bps,
            mmf_bps: p.mmf_bps,
            taker_fee_bps: p.taker_fee_bps,
            maker_fee_bps: p.maker_fee_bps,
            liq_fee_bps: p.liq_fee_bps,
            band_bps: p.band_bps,
            max_position_lots: p.max_position_lots,
            max_oi_lots: p.max_oi_lots,
            impact_lots: p.impact_lots,
            display_lot_base_units: p.display_lot_base_units,
            display_base_decimals: p.display_base_decimals,
            oracle_price: m.oracle_price.to_string(),
            oracle_time_ms: m.oracle_time_ms.to_string(),
            last_funding_time_ms: m.last_funding_time_ms.to_string(),
            cumulative_funding_per_lot: m.cumulative_funding_per_lot.to_string(),
            open_interest_lots: m.open_interest_lots,
            best_bid: m.bids.first().map(|o| o.price.to_string()),
            best_ask: m.asks.first().map(|o| o.price.to_string()),
        })
        .collect()
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Level {
    pub price: String,
    pub lots: i64,
    pub orders: u32,
}

#[derive(Serialize, Clone, Debug)]
pub struct BookView {
    pub market_id: u16,
    pub height: String,
    pub bids: Vec<Level>,
    pub asks: Vec<Level>,
}

pub fn book(st: &StateV1, market_id: u16, depth: usize) -> Option<BookView> {
    let m = st
        .config
        .markets
        .iter()
        .position(|p| p.market_id == market_id)?;
    let levels = |orders: &[caravel_types::state::OrderV1]| {
        let mut out: Vec<Level> = Vec::new();
        for o in orders {
            match out.last_mut() {
                Some(l) if l.price == o.price.to_string() => {
                    l.lots += o.lots_remaining;
                    l.orders += 1;
                }
                _ => {
                    if out.len() == depth {
                        break;
                    }
                    out.push(Level {
                        price: o.price.to_string(),
                        lots: o.lots_remaining,
                        orders: 1,
                    });
                }
            }
        }
        out
    };
    Some(BookView {
        market_id,
        height: st.height.to_string(),
        bids: levels(&st.markets[m].bids),
        asks: levels(&st.markets[m].asks),
    })
}

#[derive(Serialize, Clone, Debug)]
pub struct FillView {
    pub height: String,
    pub timestamp_ms: String,
    pub market_id: u16,
    pub price: String,
    pub lots: i64,
    pub taker_side: &'static str,
    pub maker_order_id: String,
    pub maker: String,
    pub taker: String,
}

/// Fills in a block, in order. `st` is the state after it (for account keys).
pub fn fills(st: &StateV1, height: u64, timestamp_ms: u64, receipts: &Receipts) -> Vec<FillView> {
    let key = |i: u32| {
        st.accounts
            .get(i as usize)
            .map(|a| g_address(&a.key))
            .unwrap_or_default()
    };
    receipts
        .receipts
        .iter()
        .flat_map(|r| r.events.iter())
        .filter_map(|e| match e {
            Event::Fill {
                market,
                maker_order_id,
                maker_idx,
                taker_idx,
                price,
                lots,
                taker_side,
            } => Some(FillView {
                height: height.to_string(),
                timestamp_ms: timestamp_ms.to_string(),
                market_id: *market,
                price: price.to_string(),
                lots: *lots,
                taker_side: side(*taker_side),
                maker_order_id: maker_order_id.to_string(),
                maker: key(*maker_idx),
                taker: key(*taker_idx),
            }),
            _ => None,
        })
        .collect()
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

pub fn block(record: &BlockRecordV1, receipts_bytes: &[u8]) -> Option<BlockView> {
    let input = BlockInputV1::decode(&record.input).ok()?;
    let receipts = Receipts::decode(receipts_bytes).ok()?;
    let entries = input
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let rc = receipts.receipts.iter().find(|r| r.entry_index == i as u32);
            let (kind, hex_) = match e {
                Entry::Inbox(m) => ("inbox", hex(&m.encode())),
                Entry::Oracle(u) => ("oracle", hex(&u.encode())),
                Entry::User(tx) => ("user", hex(&tx.encode())),
            };
            EntryView {
                index: i as u32,
                kind,
                hex: hex_,
                code: rc.map(|r| r.code),
                events: rc
                    .map(|r| r.events.iter().map(|e| format!("{e:?}")).collect())
                    .unwrap_or_default(),
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
    leaves
        .iter()
        .filter(|l| l.key == *account)
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
