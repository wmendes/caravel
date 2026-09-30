//! `AppGenesisV1`, the genesis config of an SDK app (spec §20.4.1, DEC-060).

use alloc::vec::Vec;

use caravel_core::batch::MAX_BATCH_BYTES;
use caravel_core::codec::{DecodeError, EncodeError, Reader, Writer};

pub const APP_GENESIS_MAGIC: &[u8; 8] = b"CVAPPGN1";
pub const MAX_SYSTEM_KEYS: usize = 4;
/// The §10.2 bounds, the same for every lane.
pub const MAX_BLOCK_BYTES: u32 = 48_000;
pub const MAX_EXEC_CPU: u64 = 400_000_000;
pub const MAX_EXEC_MEM: u64 = 41_943_040;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessMode {
    Open = 0,
    Allowlist = 1,
}

impl AccessMode {
    fn from_u8(v: u8) -> Result<Self, DecodeError> {
        match v {
            0 => Ok(Self::Open),
            1 => Ok(Self::Allowlist),
            _ => Err(DecodeError::BadEnum),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppGenesisV1 {
    pub lane_id: [u8; 32],
    /// ASCII, zero-padded.
    pub template: [u8; 16],
    pub template_version: u16,
    /// Accounts `0..n`, flag SYSTEM; key 0 is the treasury.
    pub system_keys: Vec<[u8; 32]>,
    pub access_mode: AccessMode,
    /// Strictly ascending.
    pub allowlist: Vec<[u8; 32]>,
    pub min_deposit: i128,
    pub min_withdrawal: i128,
    pub max_accounts: u32,
    pub max_session_keys: u8,
    pub max_txs_per_account_per_block: u16,
    pub max_entries_per_block: u32,
    pub max_block_bytes: u32,
    pub max_pending_withdrawals: u32,
    pub exec_cpu_limit: u64,
    pub exec_mem_limit: u64,
    /// The app's own parameters; the app decodes them strictly.
    pub app_params: Vec<u8>,
}

/// Why a config breaks a genesis rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    Template,
    SystemKeys,
    Allowlist,
    Minimums,
    Limits,
}

/// `name` as a zero-padded 16-byte template id.
pub const fn template_id(name: &str) -> [u8; 16] {
    let b = name.as_bytes();
    let mut out = [0u8; 16];
    let mut i = 0;
    while i < b.len() && i < 16 {
        out[i] = b[i];
        i += 1;
    }
    out
}

impl AppGenesisV1 {
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let mut w = Writer::new();
        self.encode_to(&mut w)?;
        Ok(w.into_vec())
    }

    pub(crate) fn encode_to(&self, w: &mut Writer) -> Result<(), EncodeError> {
        w.bytes(APP_GENESIS_MAGIC);
        w.bytes(&self.lane_id);
        w.bytes(&self.template);
        w.u16(self.template_version);
        w.count_u8(self.system_keys.len())?;
        for k in &self.system_keys {
            w.bytes(k);
        }
        w.u8(self.access_mode as u8);
        w.count_u16(self.allowlist.len())?;
        for k in &self.allowlist {
            w.bytes(k);
        }
        w.i128(self.min_deposit);
        w.i128(self.min_withdrawal);
        w.u32(self.max_accounts);
        w.u8(self.max_session_keys);
        w.u16(self.max_txs_per_account_per_block);
        w.u32(self.max_entries_per_block);
        w.u32(self.max_block_bytes);
        w.u32(self.max_pending_withdrawals);
        w.u64(self.exec_cpu_limit);
        w.u64(self.exec_mem_limit);
        w.count_u32(self.app_params.len())?;
        w.bytes(&self.app_params);
        Ok(())
    }

    /// Strict: exactly these bytes, nothing after them.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let g = Self::decode_from(&mut r)?;
        r.finish()?;
        Ok(g)
    }

    pub(crate) fn decode_from(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.magic(APP_GENESIS_MAGIC)?;
        let lane_id = r.array()?;
        let template = r.array()?;
        let template_version = r.u16()?;
        let n = r.u8()?;
        let mut system_keys = Vec::new();
        for _ in 0..n {
            system_keys.push(r.array()?);
        }
        let access_mode = AccessMode::from_u8(r.u8()?)?;
        let n = r.u16()?;
        let mut allowlist = Vec::new();
        for _ in 0..n {
            allowlist.push(r.array()?);
        }
        let min_deposit = r.i128()?;
        let min_withdrawal = r.i128()?;
        let max_accounts = r.u32()?;
        let max_session_keys = r.u8()?;
        let max_txs_per_account_per_block = r.u16()?;
        let max_entries_per_block = r.u32()?;
        let max_block_bytes = r.u32()?;
        let max_pending_withdrawals = r.u32()?;
        let exec_cpu_limit = r.u64()?;
        let exec_mem_limit = r.u64()?;
        let len = usize::try_from(r.u32()?).map_err(|_| DecodeError::UnexpectedEnd)?;
        let app_params = r.take(len)?.to_vec();
        Ok(Self {
            lane_id,
            template,
            template_version,
            system_keys,
            access_mode,
            allowlist,
            min_deposit,
            min_withdrawal,
            max_accounts,
            max_session_keys,
            max_txs_per_account_per_block,
            max_entries_per_block,
            max_block_bytes,
            max_pending_withdrawals,
            exec_cpu_limit,
            exec_mem_limit,
            app_params,
        })
    }

    /// The generic genesis rules (spec §20.4.1). The template and the app
    /// parameters are the engine's to check.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let t = &self.template;
        let len = t.iter().position(|b| *b == 0).unwrap_or(16);
        if len == 0
            || !t[..len].iter().all(|b| b.is_ascii_graphic())
            || t[len..].iter().any(|b| *b != 0)
        {
            return Err(ConfigError::Template);
        }
        let n = self.system_keys.len();
        if n == 0 || n > MAX_SYSTEM_KEYS {
            return Err(ConfigError::SystemKeys);
        }
        for (i, k) in self.system_keys.iter().enumerate() {
            if self.system_keys[..i].contains(k) || self.allowlist.binary_search(k).is_ok() {
                return Err(ConfigError::SystemKeys);
            }
        }
        if self.allowlist.windows(2).any(|w| w[0] >= w[1]) {
            return Err(ConfigError::Allowlist);
        }
        if self.min_deposit < 1 || self.min_withdrawal < 1 {
            return Err(ConfigError::Minimums);
        }
        let positive = self.max_accounts > 0
            && self.max_session_keys > 0
            && self.max_txs_per_account_per_block > 0
            && self.max_entries_per_block > 0
            && self.max_block_bytes > 0
            && self.max_pending_withdrawals > 0
            && self.exec_cpu_limit > 0
            && self.exec_mem_limit > 0;
        let bounded = self.max_accounts as usize > n
            && self.max_block_bytes <= MAX_BLOCK_BYTES
            && self.max_block_bytes as usize + 1_024 <= MAX_BATCH_BYTES
            && self.exec_cpu_limit <= MAX_EXEC_CPU
            && self.exec_mem_limit <= MAX_EXEC_MEM;
        if !positive || !bounded {
            return Err(ConfigError::Limits);
        }
        Ok(())
    }
}
