//! Reading `#[contracttype]` values out of the settlement contract's storage
//! (soroban-sdk 28.0.0 encodings, docs/SOURCES.md 2026-09-29): an enum
//! variant is `Vec[Symbol(name), fields…]`, a struct is a `Map` keyed by
//! field-name symbols.

use anyhow::{anyhow, bail, Result};
use stellar_xdr::{ScAddress, ScMap, ScSymbol, ScVal, ScVec};

pub fn sym(s: &str) -> ScVal {
    ScVal::Symbol(ScSymbol(s.try_into().expect("short symbol")))
}

/// A `#[contracttype]` enum variant key: `Vec[Symbol(name), fields...]`.
pub fn variant(name: &str, fields: Vec<ScVal>) -> ScVal {
    let mut v = vec![sym(name)];
    v.extend(fields);
    ScVal::Vec(Some(ScVec(v.try_into().expect("small vec"))))
}

/// The value stored under `key` in a map (instance storage, or a struct).
pub fn entry<'a>(m: &'a ScMap, key: &ScVal) -> Option<&'a ScVal> {
    m.0.iter().find(|e| e.key == *key).map(|e| &e.val)
}

/// A struct's field.
pub fn field<'a>(m: &'a ScMap, name: &str) -> Result<&'a ScVal> {
    entry(m, &sym(name)).ok_or_else(|| anyhow!("no field {name}"))
}

pub fn map<'a>(v: &'a ScVal, what: &str) -> Result<&'a ScMap> {
    match v {
        ScVal::Map(Some(m)) => Ok(m),
        _ => bail!("{what} is not a struct"),
    }
}

pub fn bytes32(v: &ScVal, what: &str) -> Result<[u8; 32]> {
    match v {
        ScVal::Bytes(b) => {
            b.0.as_slice()
                .try_into()
                .map_err(|_| anyhow!("{what} is not 32 bytes"))
        }
        _ => bail!("{what} is not bytes"),
    }
}

pub fn u64_of(v: &ScVal, what: &str) -> Result<u64> {
    match v {
        ScVal::U64(x) => Ok(*x),
        _ => bail!("{what} is not a u64"),
    }
}

pub fn u32_of(v: &ScVal, what: &str) -> Result<u32> {
    match v {
        ScVal::U32(x) => Ok(*x),
        _ => bail!("{what} is not a u32"),
    }
}

pub fn i128_of(v: &ScVal, what: &str) -> Result<i128> {
    match v {
        ScVal::I128(x) => Ok(i128::from(x)),
        _ => bail!("{what} is not an i128"),
    }
}

/// An address's 32 bytes: an ed25519 account key or a contract ID.
pub fn address_of(v: &ScVal, what: &str) -> Result<[u8; 32]> {
    match v {
        ScVal::Address(ScAddress::Account(a)) => {
            let stellar_xdr::PublicKey::PublicKeyTypeEd25519(k) = &a.0;
            Ok(k.0)
        }
        ScVal::Address(ScAddress::Contract(c)) => Ok(c.0 .0),
        _ => bail!("{what} is not an account or contract address"),
    }
}

pub fn vec_of<'a>(v: &'a ScVal, what: &str) -> Result<&'a [ScVal]> {
    match v {
        ScVal::Vec(Some(items)) => Ok(items.0.as_slice()),
        _ => bail!("{what} is not a vec"),
    }
}
