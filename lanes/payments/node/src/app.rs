//! Caravel Payments as a `LaneApp` and a `NodeApp` (spec §20.4.4, DEC-053):
//! what the runtime and the node need from the payments engine, answered with
//! `caravel-payments` on the app SDK.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use caravel_app_sdk::crypto::native::{DiagnosticCrypto, NativeCrypto};
use caravel_app_sdk::{AppEngine, AppGenesisV1, SdkState};
use caravel_core::receipts::ReceiptsV1;
use caravel_core::tx::{kind, TxEnvelopeV1};
use caravel_node::lane_toml::LaneFile;
use caravel_node::NodeApp;
use caravel_payments::{Payments, TransferEvent, TRANSFER, TRANSFER_BODY_LEN};
use caravel_runtime::sequencer::Produced;
use caravel_runtime::{LaneApp, LaneLimits, NativeFatal, StepOutput};
use serde::Deserialize;
use serde_json::Value;

use crate::views::{self, AccountView};

#[derive(Clone, Copy, Debug, Default)]
pub struct PaymentsApp;

/// No stream fields beyond `blocks` and `account`.
#[derive(Deserialize, Default, Clone)]
pub struct PaymentsSubscription {}

fn fatal(f: caravel_app_sdk::Fatal) -> NativeFatal {
    NativeFatal {
        code: f.code,
        entry_index: f.entry_index,
    }
}

/// The event's text in block views and receipts.
pub fn event_text(e: &caravel_core::receipts::EventV1) -> Option<String> {
    if let Some(p) = e.platform() {
        return p.ok().map(|p| format!("{p:?}"));
    }
    TransferEvent::decode(e).map(|t| format!("{t:?}"))
}

impl LaneApp for PaymentsApp {
    type State = SdkState;

    fn decode_state(&self, bytes: &[u8]) -> Option<SdkState> {
        SdkState::decode(bytes, &Payments::STATE_MAGIC).ok()
    }

    fn limits(&self, st: &SdkState) -> LaneLimits {
        LaneLimits {
            max_block_bytes: st.config.max_block_bytes,
            max_entries_per_block: st.config.max_entries_per_block,
            max_txs_per_account_per_block: st.config.max_txs_per_account_per_block,
            max_pending_withdrawals: st.config.max_pending_withdrawals,
        }
    }

    fn next_nonces(&self, st: &SdkState) -> BTreeMap<[u8; 32], u64> {
        st.accounts.iter().map(|a| (a.key, a.next_nonce)).collect()
    }

    fn next_nonce(&self, st: &SdkState, key: &[u8; 32]) -> Option<u64> {
        st.accounts
            .iter()
            .find(|a| a.key == *key)
            .map(|a| a.next_nonce)
    }

    fn pending_withdrawals(&self, st: &SdkState) -> usize {
        st.pending.len()
    }

    fn tx_decodes(&self, tx: &TxEnvelopeV1) -> bool {
        if kind::is_platform(tx.kind) {
            matches!(tx.standard_body(), Some(Ok(_)))
        } else {
            tx.kind == TRANSFER && tx.body.len() == TRANSFER_BODY_LEN
        }
    }

    fn native_genesis(&self, config: &[u8]) -> Result<Vec<u8>, NativeFatal> {
        caravel_app_sdk::genesis::<Payments>(config, &NativeCrypto).map_err(fatal)
    }

    fn native_step(&self, state: &[u8], block: &[u8]) -> Result<StepOutput, NativeFatal> {
        caravel_app_sdk::step::<Payments, _>(state, block, &DiagnosticCrypto)
            .map(|o| StepOutput {
                state: o.state,
                receipts: o.receipts,
            })
            .map_err(fatal)
    }

    /// The container decodes and every event is one the payments engine emits.
    fn receipts_decode(&self, receipts: &[u8]) -> bool {
        ReceiptsV1::decode(receipts).is_ok_and(|r| {
            r.receipts
                .iter()
                .all(|rc| rc.events.iter().all(|e| event_text(e).is_some()))
        })
    }

    fn render_events(&self, receipts: &[u8]) -> Option<Vec<Vec<String>>> {
        let r = ReceiptsV1::decode(receipts).ok()?;
        r.receipts
            .iter()
            .map(|rc| rc.events.iter().map(event_text).collect::<Option<Vec<_>>>())
            .collect()
    }

    fn escape_leaves(&self, st: &SdkState) -> Option<Vec<([u8; 32], i128)>> {
        Some(
            st.accounts
                .iter()
                .map(|a| (a.key, a.balance.max(0)))
                .collect(),
        )
    }
}

impl NodeApp for PaymentsApp {
    const TEMPLATE: &'static str = "payments";
    type Cache = ();
    type BlockView = ();
    type AccountView = AccountView;
    type Subscription = PaymentsSubscription;

    fn genesis_config(&self, lane: &LaneFile) -> Result<Vec<u8>> {
        crate::lane_file::genesis_config(lane)?
            .encode()
            .map_err(|_| anyhow!("config does not encode"))
    }

    fn exec_limits(&self, config: &[u8]) -> Result<(u64, u64)> {
        let g = AppGenesisV1::decode(config).map_err(|_| anyhow!("config does not decode"))?;
        Ok((g.exec_cpu_limit, g.exec_mem_limit))
    }

    fn account(&self, st: &SdkState, key: &[u8; 32]) -> Option<AccountView> {
        views::account(st, key)
    }

    fn on_block(&self, _cache: &mut (), _produced: &Produced<SdkState>) {}

    /// The account's `receipt`s, then `account`.
    fn stream(
        &self,
        p: &Produced<SdkState>,
        _view: &(),
        _sub: &PaymentsSubscription,
        account: Option<&[u8; 32]>,
        receipts: Vec<Value>,
    ) -> Vec<Value> {
        let mut out = receipts;
        if let Some(a) = account.and_then(|key| views::account(&p.state, key)) {
            let mut v = serde_json::to_value(a).unwrap_or_default();
            v["type"] = "account".into();
            out.push(v);
        }
        out
    }
}
