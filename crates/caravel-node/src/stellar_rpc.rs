//! A small Stellar RPC client (JSON-RPC over HTTP). Validators use it to read
//! the settlement contract's `LastCkpt` straight from its instance storage
//! with `getLedgerEntries`, so what they know about acceptance comes from
//! Stellar, not from the sequencer. Replay (T-010) builds on it.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use stellar_xdr::{
    ContractDataDurability, ContractId, Hash, LedgerEntryData, LedgerKey, LedgerKeyContractData,
    Limits, ReadXdr, ScAddress, ScMap, ScSymbol, ScVal, ScVec, WriteXdr,
};

pub struct Rpc {
    http: reqwest::Client,
    url: String,
}

/// `LastCkpt.seq` and `LastCkpt.header_hash` as the contract stores them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LastCheckpoint {
    pub seq: u64,
    pub header_hash: [u8; 32],
}

/// Bounds for XDR that comes from Stellar RPC, which nodes do not trust: a
/// transaction is at most 132,096 bytes on testnet, and the settlement
/// contract's entries are far smaller.
pub fn read_limits() -> Limits {
    Limits {
        depth: 500,
        len: 1 << 20,
    }
}

impl Rpc {
    pub fn new(url: &str) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()?,
            url: url.to_string(),
        })
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let resp: Value = self
            .http
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("{method}: {}", self.url))?
            .json()
            .await?;
        if let Some(err) = resp.get("error") {
            bail!("{method}: {err}");
        }
        resp.get("result")
            .cloned()
            .ok_or_else(|| anyhow!("{method}: no result"))
    }

    /// `getLedgerEntries` for one key: the entry data, if the entry exists.
    pub async fn ledger_entry(&self, key: &LedgerKey) -> Result<Option<LedgerEntryData>> {
        let key_b64 = key.to_xdr_base64(Limits::none())?;
        let result = self
            .call("getLedgerEntries", json!({ "keys": [key_b64] }))
            .await?;
        let Some(entry) = result["entries"].as_array().and_then(|e| e.first()) else {
            return Ok(None);
        };
        let xdr = entry["xdr"]
            .as_str()
            .ok_or_else(|| anyhow!("getLedgerEntries: entry without xdr"))?;
        Ok(Some(LedgerEntryData::from_xdr_base64(xdr, read_limits())?))
    }

    /// The contract's instance storage map.
    pub async fn instance_storage(&self, contract: &[u8; 32]) -> Result<Option<ScMap>> {
        let Some(LedgerEntryData::ContractData(d)) =
            self.ledger_entry(&instance_key(contract)).await?
        else {
            return Ok(None);
        };
        match d.val {
            ScVal::ContractInstance(i) => Ok(i.storage),
            _ => bail!("the instance entry does not hold a contract instance"),
        }
    }

    pub async fn last_checkpoint(&self, contract: &[u8; 32]) -> Result<Option<LastCheckpoint>> {
        Ok(self
            .instance_storage(contract)
            .await?
            .as_ref()
            .and_then(last_checkpoint_in))
    }
}

/// The ledger key of a contract's instance entry.
pub fn instance_key(contract: &[u8; 32]) -> LedgerKey {
    LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::Contract(ContractId(Hash(*contract))),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
    })
}

/// `DataKey::LastCkpt` as a `#[contracttype]` unit variant: `Vec[Symbol("LastCkpt")]`.
pub fn last_ckpt_key() -> ScVal {
    let sym = ScVal::Symbol(ScSymbol("LastCkpt".try_into().expect("short symbol")));
    ScVal::Vec(Some(ScVec(vec![sym].try_into().expect("one element"))))
}

fn field<'a>(map: &'a ScMap, name: &str) -> Option<&'a ScVal> {
    map.0
        .iter()
        .find(|e| matches!(&e.key, ScVal::Symbol(s) if s.0.as_slice() == name.as_bytes()))
        .map(|e| &e.val)
}

/// Reads `LastCkpt` from an instance storage map.
pub fn last_checkpoint_in(storage: &ScMap) -> Option<LastCheckpoint> {
    let key = last_ckpt_key();
    let ScVal::Map(Some(m)) = &storage.0.iter().find(|e| e.key == key)?.val else {
        return None;
    };
    let ScVal::U64(seq) = field(m, "seq")? else {
        return None;
    };
    let ScVal::Bytes(h) = field(m, "header_hash")? else {
        return None;
    };
    Some(LastCheckpoint {
        seq: *seq,
        header_hash: h.0.as_slice().try_into().ok()?,
    })
}
