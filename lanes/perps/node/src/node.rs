//! Caravel Perps as a `NodeApp` (M0.5, P-06): its lane file, account view,
//! `/v1/markets*` routes, recent fills, candles and stream messages, and the
//! oracle feed route. The JSON is the M0 JSON, plus `candles` and the
//! per-block `tickers` message (H-15, DEC-103).

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::Router;
use caravel_core::block::BlockInputV1;
use caravel_node::api::{ok, ApiError, ApiResult};
use caravel_node::lane_toml::LaneFile;
use caravel_node::sequencer::SequencerNode;
use caravel_node::{FeedApi, NodeApp};
use caravel_runtime::sequencer::Produced;
use caravel_runtime::store::Store;
use caravel_types::config::GenesisConfigV1;
use caravel_types::state::StateV1;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::history::{History, FILLS_KEPT, INTERVALS};
use crate::views::{self, AccountView, FillView};
use crate::{lane_file, PerpsApp};

/// What one block adds to the stream: its fills and its time.
#[derive(Clone, Debug, Default)]
pub struct PerpsBlock {
    pub fills: Vec<FillView>,
    pub timestamp_ms: u64,
}

/// `markets`: the markets whose fills and books the subscriber wants;
/// `tickers`: every market's price line on every block.
#[derive(Deserialize, Default, Clone)]
pub struct PerpsSubscription {
    #[serde(default)]
    markets: Vec<u16>,
    #[serde(default)]
    tickers: bool,
}

/// A market's line in the per-block `tickers` message.
#[derive(Serialize, Clone, Debug)]
pub struct TickerView {
    pub market_id: u16,
    pub oracle_price: String,
    pub oracle_time_ms: String,
    pub best_bid: Option<String>,
    pub best_ask: Option<String>,
    pub open_interest_lots: i64,
}

/// Every market's ticker in `st`.
pub fn tickers(st: &StateV1) -> Vec<TickerView> {
    st.config
        .markets
        .iter()
        .zip(&st.markets)
        .map(|(p, m)| TickerView {
            market_id: p.market_id,
            oracle_price: m.oracle_price.to_string(),
            oracle_time_ms: m.oracle_time_ms.to_string(),
            best_bid: m.bids.first().map(|o| o.price.to_string()),
            best_ask: m.asks.first().map(|o| o.price.to_string()),
            open_interest_lots: m.open_interest_lots,
        })
        .collect()
}

type Node = State<Arc<SequencerNode<PerpsApp>>>;

impl NodeApp for PerpsApp {
    const TEMPLATE: &'static str = "perps";
    type Cache = History;
    type BlockView = PerpsBlock;
    type AccountView = AccountView;
    type Subscription = PerpsSubscription;

    fn genesis_config(&self, lane: &LaneFile) -> Result<Vec<u8>> {
        lane_file::genesis_config(lane)?
            .encode()
            .map_err(|_| anyhow!("config does not encode"))
    }

    fn exec_limits(&self, config: &[u8]) -> Result<(u64, u64)> {
        let g = GenesisConfigV1::decode(config).map_err(|_| anyhow!("config does not decode"))?;
        Ok((g.exec_cpu_limit, g.exec_mem_limit))
    }

    fn account(&self, st: &StateV1, key: &[u8; 32]) -> Option<AccountView> {
        views::account(st, key)
    }

    fn on_block(&self, cache: &mut History, produced: &Produced<StateV1>) -> PerpsBlock {
        let timestamp_ms =
            BlockInputV1::decode(&produced.record.input).map_or(0, |b| b.timestamp_ms);
        let fills = cache.index_block(
            &produced.state,
            produced.height,
            &produced.record.input,
            &produced.receipts_bytes,
        );
        PerpsBlock {
            fills,
            timestamp_ms,
        }
    }

    /// Loads the history file beside the store and replays the blocks since.
    fn warm(
        &self,
        cache: &mut History,
        db: &std::path::Path,
        store: &Store,
        state: &StateV1,
        height: u64,
    ) -> Result<()> {
        let mut h = History::open(&History::path_for(db))?;
        h.catch_up(store, state, height)?;
        *cache = h;
        Ok(())
    }

    fn routes(&self) -> Router<Arc<SequencerNode<Self>>> {
        Router::new()
            .route("/v1/markets", get(markets))
            .route("/v1/markets/{id}/book", get(book))
            .route("/v1/markets/{id}/trades", get(trades))
            .route("/v1/markets/{id}/candles", get(candles))
    }

    /// `tickers` (when asked), `fill`, `book`, the account's `receipt`s,
    /// then `account` (the M0 order after `tickers`).
    fn stream(
        &self,
        p: &Produced<StateV1>,
        block: &PerpsBlock,
        sub: &PerpsSubscription,
        account: Option<&[u8; 32]>,
        receipts: Vec<Value>,
    ) -> Vec<Value> {
        let mut out = Vec::new();
        if sub.tickers {
            out.push(serde_json::json!({
                "type": "tickers",
                "height": p.height.to_string(),
                "timestamp_ms": block.timestamp_ms.to_string(),
                "markets": tickers(&p.state),
            }));
        }
        let markets: HashMap<u16, ()> = sub.markets.iter().map(|m| (*m, ())).collect();
        for f in block
            .fills
            .iter()
            .filter(|f| markets.contains_key(&f.market_id))
        {
            let mut v = serde_json::to_value(f).unwrap_or_default();
            v["type"] = "fill".into();
            out.push(v);
        }
        for m in &sub.markets {
            if let Some(b) = views::book(&p.state, *m, 20) {
                let mut v = serde_json::to_value(b).unwrap_or_default();
                v["type"] = "book".into();
                out.push(v);
            }
        }
        out.extend(receipts);
        if let Some(a) = account.and_then(|key| views::account(&p.state, key)) {
            let mut v = serde_json::to_value(a).unwrap_or_default();
            v["type"] = "account".into();
            out.push(v);
        }
        out
    }

    fn subscription_hint(&self) -> &'static str {
        "expected {blocks, markets, tickers, account}"
    }

    fn feed_api(&self) -> Option<FeedApi> {
        Some(FeedApi {
            route: "oracle",
            not_decoded: "update is not an OracleUpdateV1",
            refused_code: "BAD_ORACLE",
            refused: "unknown oracle key or bad signature",
        })
    }

    /// Collateral, prices and margins are in units of 10^-7 (USDC stroops, §10).
    fn token_decimals(&self) -> Option<u32> {
        Some(7)
    }
}

async fn markets(State(node): Node) -> ApiResult {
    ok(views::markets(&node.state()))
}

#[derive(Deserialize)]
struct Depth {
    depth: Option<usize>,
}

async fn book(State(node): Node, Path(id): Path<u16>, Query(q): Query<Depth>) -> ApiResult {
    let depth = q.depth.unwrap_or(50).clamp(1, 500);
    ok(views::book(&node.state(), id, depth)
        .ok_or_else(|| ApiError::not_found("no such market"))?)
}

#[derive(Deserialize)]
struct Limit {
    limit: Option<usize>,
}

async fn trades(State(node): Node, Path(id): Path<u16>, Query(q): Query<Limit>) -> ApiResult {
    let limit = q.limit.unwrap_or(100).clamp(1, FILLS_KEPT);
    ok(node.cache().fills(id, limit))
}

#[derive(Deserialize)]
struct CandleQuery {
    interval: Option<String>,
    limit: Option<usize>,
}

/// `GET /v1/markets/{id}/candles?interval=1m|5m|15m|1h&limit=`: candles of
/// the oracle price (the mark), oldest first, the last one still open.
async fn candles(
    State(node): Node,
    Path(id): Path<u16>,
    Query(q): Query<CandleQuery>,
) -> ApiResult {
    if views::book(&node.state(), id, 0).is_none() {
        return Err(ApiError::not_found("no such market"));
    }
    let name = q.interval.as_deref().unwrap_or("1m");
    let minutes = INTERVALS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, m)| *m)
        .ok_or_else(|| ApiError::bad_request("INTERVAL", "interval is one of 1m, 5m, 15m, 1h"))?;
    let limit = q.limit.unwrap_or(500).clamp(1, 5000);
    ok(node.cache().candles(id, minutes, limit))
}
