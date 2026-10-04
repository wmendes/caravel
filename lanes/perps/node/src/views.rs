//! The perps lane's own views (spec §14.4): accounts, markets, books and fills.
//! Moved from the runtime (P-05); the output is the M0 JSON.

use caravel_perps::margin::{account_margin, upnl};
use caravel_types::receipts::{Event, Receipts};
use caravel_types::state::StateV1;
use caravel_types::tx::Side;
use serde::{Deserialize, Serialize};

use caravel_runtime::views::g_address;

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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FillView {
    pub height: String,
    pub timestamp_ms: String,
    pub market_id: u16,
    pub price: String,
    pub lots: i64,
    pub taker_side: String,
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
                taker_side: side(*taker_side).to_string(),
                maker_order_id: maker_order_id.to_string(),
                maker: key(*maker_idx),
                taker: key(*taker_idx),
            }),
            _ => None,
        })
        .collect()
}
