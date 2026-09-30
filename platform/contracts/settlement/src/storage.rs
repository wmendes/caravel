//! Storage helpers with the TTL policy (spec §13.1): instance storage is
//! extended on every call; a persistent entry is extended when it is read or
//! written and its TTL is below 30 days, toward 120 days. Both are clamped to
//! the network's current maximum TTL, which is a live network setting.

use soroban_sdk::{Env, IntoVal, TryFromVal, Val};

use crate::types::{Config, DataKey, Error, LastCheckpoint};

/// Ledgers per day at ~5 s per ledger.
const DAY: u32 = 17_280;
const THRESHOLD: u32 = 30 * DAY;
const TARGET: u32 = 120 * DAY;

fn ttl(env: &Env) -> (u32, u32) {
    let extend_to = TARGET.min(env.storage().max_ttl());
    (THRESHOLD.min(extend_to), extend_to)
}

pub fn bump_instance(env: &Env) {
    let (threshold, extend_to) = ttl(env);
    env.storage().instance().extend_ttl(threshold, extend_to);
}

pub fn get<V: TryFromVal<Env, Val>>(env: &Env, key: &DataKey) -> Option<V> {
    let v = env.storage().persistent().get(key);
    if v.is_some() {
        let (threshold, extend_to) = ttl(env);
        env.storage()
            .persistent()
            .extend_ttl(key, threshold, extend_to);
    }
    v
}

pub fn has(env: &Env, key: &DataKey) -> bool {
    let present = env.storage().persistent().has(key);
    if present {
        let (threshold, extend_to) = ttl(env);
        env.storage()
            .persistent()
            .extend_ttl(key, threshold, extend_to);
    }
    present
}

pub fn set<V: IntoVal<Env, Val>>(env: &Env, key: &DataKey, value: &V) {
    env.storage().persistent().set(key, value);
    let (threshold, extend_to) = ttl(env);
    env.storage()
        .persistent()
        .extend_ttl(key, threshold, extend_to);
}

// --- Instance entries ---------------------------------------------------------

pub fn instance<V: TryFromVal<Env, Val>>(env: &Env, key: &DataKey) -> Option<V> {
    env.storage().instance().get(key)
}

pub fn set_instance<V: IntoVal<Env, Val>>(env: &Env, key: &DataKey, value: &V) {
    env.storage().instance().set(key, value);
}

pub fn config(env: &Env) -> Config {
    instance(env, &DataKey::Config).expect("constructed")
}

pub fn last(env: &Env) -> LastCheckpoint {
    instance(env, &DataKey::LastCkpt).expect("constructed")
}

pub fn u64_of(env: &Env, key: &DataKey) -> u64 {
    instance(env, key).unwrap_or(0)
}

pub fn i128_of(env: &Env, key: &DataKey) -> i128 {
    instance(env, key).unwrap_or(0)
}

pub fn is_frozen(env: &Env) -> bool {
    env.storage().instance().has(&DataKey::Frozen)
}

pub fn require_not_frozen(env: &Env) -> Result<(), Error> {
    if is_frozen(env) {
        Err(Error::Frozen)
    } else {
        Ok(())
    }
}
