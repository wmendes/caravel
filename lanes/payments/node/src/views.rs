//! The payments lane's account view (spec §14.4): balance, nonce and session keys.

use caravel_app_sdk::SdkState;
use caravel_runtime::views::g_address;
use serde::Serialize;

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
    /// USDC stroops.
    pub balance: String,
    pub next_nonce: String,
    pub session_keys: Vec<SessionKeyView>,
}

pub fn account(st: &SdkState, key: &[u8; 32]) -> Option<AccountView> {
    let (i, a) = st
        .accounts
        .iter()
        .enumerate()
        .find(|(_, a)| a.key == *key)?;
    Some(AccountView {
        account: g_address(&a.key),
        index: i as u32,
        system: a.is_system(),
        balance: a.balance.to_string(),
        next_nonce: a.next_nonce.to_string(),
        session_keys: a
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
