//! What the node needs from a lane's app on top of the runtime's `LaneApp`
//! (M0.5, P-06, DEC-053).
//!
//! The node serves every lane the same way: `/v1/tx`, `/v1/status`,
//! `/v1/accounts/{account}`, blocks, checkpoints, proofs and the stream, and
//! the internal inbox, feed and checkpoint routes. This trait covers the rest:
//! - turning a lane file into the app's genesis config bytes;
//! - the account view, and the app's own routes and stream messages;
//! - the view cache the node keeps between blocks;
//! - the feed route, when the app has feeds.
//!
//! An app's node binary is a thin `main` over [`crate::cli`] (DEC-053).

use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use caravel_runtime::sequencer::Produced;
use caravel_runtime::LaneApp;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use crate::lane_toml::LaneFile;
use crate::sequencer::SequencerNode;

/// The internal feed route of an app with feeds: `POST /internal/{route}`
/// with `{"update": "<hex>"}`.
#[derive(Clone, Copy, Debug)]
pub struct FeedApi {
    /// The route name (perps: "oracle").
    pub route: &'static str,
    /// The 400 `DECODE` message for bytes that are not a feed update.
    pub not_decoded: &'static str,
    /// The 400 code and message for an update the app does not admit.
    pub refused_code: &'static str,
    pub refused: &'static str,
}

pub trait NodeApp: LaneApp + Clone {
    /// The `[app] template` of the lane files this app runs.
    const TEMPLATE: &'static str;

    /// What the node keeps between blocks for the app's views (perps: the
    /// latest fills per market).
    type Cache: Default + Send + 'static;

    /// What one block adds to the stream (perps: its fills).
    type BlockView: Clone + Send + Sync + 'static;

    /// `GET /v1/accounts/{account}` and the stream's `account` message.
    type AccountView: Serialize;

    /// Stream subscription fields besides `blocks` and `account` (perps: `markets`).
    type Subscription: DeserializeOwned + Default + Clone + Send + Sync + 'static;

    /// The genesis config bytes of a lane file: its generic sections and the
    /// app's own. A lane file without `[app]` (an M0 file) is the app's to
    /// accept or refuse.
    fn genesis_config(&self, lane: &LaneFile) -> Result<Vec<u8>>;

    /// The consensus execution limits in a genesis config: `(cpu, mem)`.
    fn exec_limits(&self, config: &[u8]) -> Result<(u64, u64)>;

    fn account(&self, state: &Self::State, key: &[u8; 32]) -> Option<Self::AccountView>;

    /// After each block: updates the cache and returns what the stream sends for it.
    fn on_block(
        &self,
        cache: &mut Self::Cache,
        produced: &Produced<Self::State>,
    ) -> Self::BlockView;

    /// The app's public routes (perps: `/v1/markets*`).
    fn routes(&self) -> Router<Arc<SequencerNode<Self>>> {
        Router::new()
    }

    /// The stream messages after `block` for one subscriber, in order.
    /// `receipts` are the account's `receipt` messages, built by the node.
    fn stream(
        &self,
        produced: &Produced<Self::State>,
        view: &Self::BlockView,
        sub: &Self::Subscription,
        account: Option<&[u8; 32]>,
        receipts: Vec<Value>,
    ) -> Vec<Value>;

    /// The error text for a subscription that does not parse.
    fn subscription_hint(&self) -> &'static str {
        "expected {blocks, account}"
    }

    /// The feed route, for an app with feeds.
    fn feed_api(&self) -> Option<FeedApi> {
        None
    }
}
