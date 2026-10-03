//! What Stellar has for a deployment, read straight from the ledger with
//! `getLedgerEntries`: the accounts, USDC, the settlement Wasm and the
//! settlement contract's instance and signer entries. No transaction is
//! simulated, so reading costs nothing and needs no key.

use anyhow::{anyhow, bail, Result};
use caravel_core::preimage::{signers_hash_preimage, WeightedSigner};
use caravel_node::scval::{
    self, address_of, bytes32, field, i128_of, map, u32_of, u64_of, variant,
};
use caravel_node::stellar_rpc::{account_key, code_key, instance_key, persistent_key, Entry, Rpc};
use caravel_runtime::checkpoint::sha256;
use stellar_xdr::{ContractExecutable, LedgerEntryData, ScVal};

use crate::plan::{Chain, Desired, Key, OnChain, Params, SignerSet};

/// What `status` shows beyond the plan's inputs.
#[derive(Clone, Debug, Default)]
pub struct Extra {
    pub latest_ledger: u32,
    /// `LastCkpt`: seq and the ledger time it was accepted.
    pub last_checkpoint: Option<(u64, u64)>,
    /// `LastCkpt.inbox_through`: inbox messages the lane has processed.
    pub inbox_through: u64,
    pub inbox_count: u64,
    /// When the oldest inbox message the lane hasn't processed was enqueued.
    pub oldest_unprocessed_at: Option<u64>,
    /// The relayer account's XLM, in stroops.
    pub relayer_balance: Option<i64>,
    /// Entries and the last ledger each lives until.
    pub ttl: Vec<(&'static str, u32)>,
}

/// The hash the contract keys a signer set by (`SignersEpoch(hash)`).
pub fn signers_hash(set: &SignerSet) -> Key {
    let raw: Vec<WeightedSigner> = set
        .signers
        .iter()
        .map(|(key, weight)| WeightedSigner {
            key: *key,
            weight: *weight,
        })
        .collect();
    sha256(&signers_hash_preimage(&raw, set.threshold).expect("a signer set encodes"))
}

fn signer_set(v: &ScVal) -> Result<SignerSet> {
    let m = map(v, "WeightedSigners")?;
    let mut signers = Vec::new();
    for s in scval::vec_of(field(m, "signers")?, "signers")? {
        let s = map(s, "WeightedSigner")?;
        signers.push((
            bytes32(field(s, "key")?, "key")?,
            u32_of(field(s, "weight")?, "weight")?,
        ));
    }
    Ok(SignerSet {
        signers,
        threshold: u32_of(field(m, "threshold")?, "threshold")?,
    })
}

fn contract_data(e: &Option<Entry>) -> Option<&ScVal> {
    match e.as_ref().map(|e| &e.data) {
        Some(LedgerEntryData::ContractData(d)) => Some(&d.val),
        _ => None,
    }
}

/// The declared contracts (C-20), tokens (C-19) and accounts (C-18): which
/// contracts (and Wasm) and accounts exist, and the accounts' trustlines
/// with balances, in batched reads.
async fn read_declared(rpc: &Rpc, d: &Desired, chain: &mut Chain) -> Result<()> {
    // Declared contracts (C-20): the Wasm each runs, and which Wasm is
    // uploaded.
    // Each key once: the RPC refuses a batch that asks for one twice (two
    // contracts can share their Wasm).
    if !d.contracts.is_empty() {
        let wasms: Vec<Key> = d
            .contracts
            .iter()
            .map(|c| c.wasm)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut keys: Vec<_> = d
            .contracts
            .iter()
            .map(|c| instance_key(&c.address))
            .collect();
        keys.extend(wasms.iter().map(code_key));
        let (entries, _) = rpc.ledger_entries(&keys).await?;
        let (instances, codes) = entries.split_at(d.contracts.len());
        for (c, e) in d.contracts.iter().zip(instances) {
            if let Some(ScVal::ContractInstance(i)) = contract_data(e) {
                if let ContractExecutable::Wasm(h) = &i.executable {
                    chain.contracts.insert(c.address, h.0);
                }
            }
        }
        for (w, e) in wasms.iter().zip(codes) {
            if e.is_some() {
                chain.wasms.insert(*w);
            }
        }
    }
    // Declared tokens' contracts.
    if !d.tokens.is_empty() {
        let contracts: Vec<Key> = d
            .tokens
            .iter()
            .map(|t| t.contract)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let keys: Vec<_> = contracts.iter().map(instance_key).collect();
        let (entries, _) = rpc.ledger_entries(&keys).await?;
        for (c, e) in contracts.iter().zip(entries) {
            if e.is_some() {
                chain.tokens.insert(*c);
            }
        }
    }
    let mut keys = Vec::new();
    let mut what: Vec<(Key, Option<(String, Key)>)> = Vec::new();
    for a in &d.accounts {
        keys.push(account_key(&a.key));
        what.push((a.key, None));
        // An issuer has no trustline to its own asset (and the ledger has no
        // such key to ask for).
        for (code, issuer) in a.trustlines.iter().filter(|(_, issuer)| *issuer != a.key) {
            keys.push(crate::address::trustline_key(&a.key, code, issuer));
            what.push((a.key, Some((code.clone(), *issuer))));
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    let (keys, what): (Vec<_>, Vec<_>) = keys
        .into_iter()
        .zip(what)
        .filter(|(_, w)| seen.insert(w.clone()))
        .unzip();
    for chunk in keys.chunks(100).zip(what.chunks(100)) {
        let (entries, _) = rpc.ledger_entries(chunk.0).await?;
        for (entry, (account, line)) in entries.iter().zip(chunk.1) {
            match (entry.as_ref().map(|e| &e.data), line) {
                (Some(LedgerEntryData::Account(_)), None) => {
                    chain.accounts.insert(*account);
                }
                (Some(LedgerEntryData::Trustline(t)), Some((code, issuer))) => {
                    chain
                        .trustlines
                        .insert((*account, code.clone(), *issuer), i128::from(t.balance));
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Reads the chain side of deployment `d`.
pub async fn read(rpc: &Rpc, d: &Desired) -> Result<(Chain, Extra)> {
    let set_hash = signers_hash(&d.signers);
    let keys = [
        account_key(&d.admin),
        account_key(&d.relayer),
        instance_key(&d.token),
        code_key(&d.settlement_wasm),
        instance_key(&d.settlement),
        persistent_key(
            &d.settlement,
            variant(
                "SignersEpoch",
                vec![ScVal::Bytes(set_hash.to_vec().try_into()?)],
            ),
        ),
    ];
    let (e, latest_ledger) = rpc.ledger_entries(&keys).await?;
    let mut chain = Chain::default();
    read_declared(rpc, d, &mut chain).await?;
    let mut extra = Extra {
        latest_ledger,
        ..Extra::default()
    };
    for (i, key) in [d.admin, d.relayer].iter().enumerate() {
        if e[i].is_some() {
            chain.accounts.insert(*key);
        }
    }
    if let Some(LedgerEntryData::Account(a)) = e[1].as_ref().map(|e| &e.data) {
        extra.relayer_balance = Some(a.balance);
    }
    chain.token_exists = e[2].is_some();
    chain.settlement_wasm_uploaded = e[3].is_some();
    if let Some(entry) = &e[3] {
        extra
            .ttl
            .extend(entry.live_until.map(|l| ("settlement Wasm", l)));
    }
    let Some(ScVal::ContractInstance(instance)) = contract_data(&e[4]) else {
        return Ok((chain, extra));
    };
    if let Some(l) = e[4].as_ref().and_then(|e| e.live_until) {
        extra.ttl.push(("settlement instance", l));
    }
    let code = match &instance.executable {
        ContractExecutable::Wasm(h) => h.0,
        ContractExecutable::StellarAsset => bail!(
            "{} is an asset contract, not a settlement contract",
            crate::address::strkey(&d.settlement)
        ),
        // CAP-85: code owned by another contract. Settlement contracts run their own Wasm.
        ContractExecutable::ExternalRef(_) => bail!(
            "{}'s code is managed by another contract (CAP-85); a settlement contract runs its own Wasm",
            crate::address::strkey(&d.settlement)
        ),
    };
    let storage = instance
        .storage
        .as_ref()
        .ok_or_else(|| anyhow!("the settlement contract has no instance storage"))?;
    let get = |name: &str| scval::entry(storage, &variant(name, vec![]));
    let config = map(get("Config").ok_or_else(|| anyhow!("no Config"))?, "Config")?;
    let params = map(field(config, "params")?, "params")?;
    let epoch = u64_of(get("Epoch").ok_or_else(|| anyhow!("no Epoch"))?, "Epoch")?;
    if let Some(v) = get("LastCkpt") {
        let lc = map(v, "LastCkpt")?;
        extra.last_checkpoint = Some((
            u64_of(field(lc, "seq")?, "seq")?,
            u64_of(field(lc, "accepted_at")?, "accepted_at")?,
        ));
        extra.inbox_through = u64_of(field(lc, "inbox_through")?, "inbox_through")?;
    }
    extra.inbox_count = get("InboxCount")
        .map(|v| u64_of(v, "InboxCount"))
        .transpose()?
        .unwrap_or(0);
    if extra.inbox_count > extra.inbox_through {
        let key = variant("Inbox", vec![ScVal::U64(extra.inbox_through)]);
        let (m, _) = rpc
            .ledger_entries(&[persistent_key(&d.settlement, key)])
            .await?;
        if let Some(v) = contract_data(&m[0]) {
            let msg = map(v, "InboxMsg")?;
            extra.oldest_unprocessed_at = Some(u64_of(field(msg, "enqueued_at")?, "enqueued_at")?);
        }
    }
    // The current signer set, a second read.
    let (s, _) = rpc
        .ledger_entries(&[persistent_key(
            &d.settlement,
            variant("Signers", vec![ScVal::U64(epoch)]),
        )])
        .await?;
    let signers = signer_set(contract_data(&s[0]).ok_or_else(|| anyhow!("no Signers({epoch})"))?)?;
    if let Some(l) = s[0].as_ref().and_then(|e| e.live_until) {
        extra.ttl.push(("current signer set", l));
    }
    let desired_set_epoch = contract_data(&e[5])
        .map(|v| u64_of(v, "SignersEpoch"))
        .transpose()?;
    chain.settlement = Some(OnChain {
        code,
        admin: address_of(field(config, "admin")?, "admin")?,
        // The settlement token (the M0 ABI names it `usdc`).
        token: address_of(field(config, "usdc")?, "usdc")?,
        lane_id: bytes32(field(config, "lane_id")?, "lane_id")?,
        engine_wasm_hash: bytes32(field(config, "engine_wasm_hash")?, "engine_wasm_hash")?,
        genesis_state_hash: bytes32(field(config, "genesis_state_hash")?, "genesis_state_hash")?,
        config_hash: bytes32(field(config, "config_hash")?, "config_hash")?,
        params: Params {
            force_inclusion_window_secs: u64_of(
                field(params, "force_inclusion_window_secs")?,
                "force_inclusion_window_secs",
            )?,
            escape_timeout_secs: u64_of(
                field(params, "escape_timeout_secs")?,
                "escape_timeout_secs",
            )?,
            min_rotation_delay_secs: u64_of(
                field(params, "min_rotation_delay_secs")?,
                "min_rotation_delay_secs",
            )?,
            signer_retention_epochs: u32_of(
                field(params, "signer_retention_epochs")?,
                "signer_retention_epochs",
            )?,
            min_deposit: i128_of(field(params, "min_deposit")?, "min_deposit")?,
        },
        epoch,
        signers,
        desired_set_epoch,
        frozen: get("Frozen").is_some(),
    });
    Ok((chain, extra))
}
