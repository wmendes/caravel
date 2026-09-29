//! Profiling only: each function runs one engine phase so the host can meter it.
#![no_std]
extern crate alloc;

use caravel_types::state::StateV1;
use soroban_sdk::{contract, contractimpl, Bytes, Env};

#[contract]
pub struct EngineProfile;

#[contractimpl]
impl EngineProfile {
    pub fn copy_in(_env: Env, state: Bytes) -> u32 {
        state.to_alloc_vec().len() as u32
    }

    pub fn decode(_env: Env, state: Bytes) -> u32 {
        StateV1::decode(&state.to_alloc_vec())
            .map(|s| s.accounts.len() as u32)
            .unwrap_or(0)
    }

    pub fn dec_enc(_env: Env, state: Bytes) -> u32 {
        let st = StateV1::decode(&state.to_alloc_vec()).unwrap();
        st.encode().unwrap().len() as u32
    }

    pub fn equity(_env: Env, state: Bytes) -> u32 {
        let st = StateV1::decode(&state.to_alloc_vec()).unwrap();
        (0..st.accounts.len())
            .filter(|a| caravel_perps::margin::equity(&st, *a).unwrap() > 0)
            .count() as u32
    }

    pub fn margin(_env: Env, state: Bytes) -> u32 {
        let st = StateV1::decode(&state.to_alloc_vec()).unwrap();
        (0..st.accounts.len())
            .filter(|a| {
                caravel_perps::margin::account_margin(&st, *a, None)
                    .unwrap()
                    .equity
                    > 0
            })
            .count() as u32
    }
}
