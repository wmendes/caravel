//! Caravel Perps as a `NodeApp` (M0.5, P-06): its lane file, account view,
//! `/v1/markets*` routes, recent fills and stream messages, and the oracle
//! feed route. The JSON is the M0 JSON.

use std::collections::{BTreeMap, HashMap, VecDeque};
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
use caravel_types::config::GenesisConfigV1;
use caravel_types::receipts::Receipts;
use caravel_types::state::StateV1;
use serde::Deserialize;
use serde_json::Value;

use crate::views::{self, AccountView, FillView};
use crate::{lane_file, PerpsApp};

/// Fills kept per market for `/v1/markets/{id}/trades`.
pub const FILLS_KEPT: usize = 1000;

/// The latest fills per market, newest first.
#[derive(Default)]
pub struct PerpsCache {
    fills: BTreeMap<u16, VecDeque<FillView>>,
}

/// `markets`: the markets whose fills and books the subscriber wants.
#[derive(Deserialize, Default, Clone)]
pub struct PerpsSubscription {
    #[serde(default)]
    markets: Vec<u16>,
}

type Node = State<Arc<SequencerNode<PerpsApp>>>;

impl NodeApp for PerpsApp {
    const TEMPLATE: &'static str = "perps";
    type Cache = PerpsCache;
    type BlockView = Vec<FillView>;
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

    fn on_block(&self, cache: &mut PerpsCache, produced: &Produced<StateV1>) -> Vec<FillView> {
        let ts = BlockInputV1::decode(&produced.record.input).map_or(0, |b| b.timestamp_ms);
        let fills = Receipts::decode(&produced.receipts_bytes).map_or_else(
            |_| Vec::new(),
            |r| views::fills(&produced.state, produced.height, ts, &r),
        );
        for f in &fills {
            let q = cache.fills.entry(f.market_id).or_default();
            q.push_front(f.clone());
            q.truncate(FILLS_KEPT);
        }
        fills
    }

    fn routes(&self) -> Router<Arc<SequencerNode<Self>>> {
        Router::new()
            .route("/v1/markets", get(markets))
            .route("/v1/markets/{id}/book", get(book))
            .route("/v1/markets/{id}/trades", get(trades))
    }

    /// `fill`, `book`, the account's `receipt`s, then `account` (the M0 order).
    fn stream(
        &self,
        p: &Produced<StateV1>,
        fills: &Vec<FillView>,
        sub: &PerpsSubscription,
        account: Option<&[u8; 32]>,
        receipts: Vec<Value>,
    ) -> Vec<Value> {
        let mut out = Vec::new();
        let markets: HashMap<u16, ()> = sub.markets.iter().map(|m| (*m, ())).collect();
        for f in fills.iter().filter(|f| markets.contains_key(&f.market_id)) {
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
        "expected {blocks, markets, account}"
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
    let list: Vec<FillView> = node
        .cache()
        .fills
        .get(&id)
        .map(|q| q.iter().take(limit).cloned().collect())
        .unwrap_or_default();
    ok(list)
}
