//! The node's SQLite store (spec §14.1, §15): blocks, receipts, the current
//! state, a snapshot at every checkpoint, checkpoints and their signatures,
//! the inbox as Stellar recorded it, the latest feed updates, and (for
//! validators) every header signed.
//!
//! A block, its receipts, the new state and (at a `CHECKPOINT_END`) the
//! snapshot are written in one transaction, so a restart always resumes from
//! a consistent height.

use std::path::Path;

use caravel_core::block::BlockRecordV1;
use caravel_core::inbox::InboxMsgV1;
use rusqlite::{params, Connection, OptionalExtension};

pub use rusqlite::Error as SqlError;

const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS blocks (
    height INTEGER PRIMARY KEY,
    input BLOB NOT NULL,
    state_hash_after BLOB NOT NULL,
    receipts BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS head (
    id INTEGER PRIMARY KEY CHECK (id = 0),
    height INTEGER NOT NULL,
    state BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS snapshots (
    seq INTEGER PRIMARY KEY,
    height INTEGER NOT NULL,
    state BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS checkpoints (
    seq INTEGER PRIMARY KEY,
    header BLOB NOT NULL,
    batch BLOB NOT NULL,
    first_height INTEGER NOT NULL,
    last_height INTEGER NOT NULL,
    withdrawals TEXT NOT NULL,
    status TEXT NOT NULL,
    epoch INTEGER,
    sigs TEXT,
    stellar_tx_hash TEXT,
    stellar_ledger INTEGER,
    batch_len INTEGER
);
CREATE TABLE IF NOT EXISTS inbox (
    idx INTEGER PRIMARY KEY,
    msg BLOB NOT NULL,
    acc_after BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS oracle (market_id INTEGER PRIMARY KEY, update_bytes BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS signed (seq INTEGER PRIMARY KEY, header_hash BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS flags (height INTEGER PRIMARY KEY, reason TEXT NOT NULL, cleared INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS block_archive (
    seq INTEGER PRIMARY KEY,
    first_height INTEGER NOT NULL,
    last_height INTEGER NOT NULL,
    blob BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS block_archive_first ON block_archive (first_height);
";

#[derive(Debug)]
pub enum StoreError {
    Sql(rusqlite::Error),
    /// The database belongs to another lane or config.
    WrongLane,
    /// A stored value does not decode.
    Corrupt(&'static str),
    /// A write would change something already recorded.
    Conflict(&'static str),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sql(e) => write!(f, "sqlite: {e}"),
            Self::WrongLane => write!(f, "the database belongs to another lane or config"),
            Self::Corrupt(what) => write!(f, "corrupt store: {what}"),
            Self::Conflict(what) => write!(f, "store conflict: {what}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Where a checkpoint is (spec §14.4 `/v1/status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckpointStatus {
    /// The `CHECKPOINT_END` block exists and the header is built.
    Sequenced,
    /// Validator signatures reach the threshold; queued for Stellar.
    Signed,
    /// Accepted by the settlement contract.
    Accepted,
}

impl CheckpointStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sequenced => "sequenced",
            Self::Signed => "signed",
            Self::Accepted => "accepted",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "sequenced" => Self::Sequenced,
            "signed" => Self::Signed,
            "accepted" => Self::Accepted,
            _ => return Err(StoreError::Corrupt("checkpoint status")),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointRow {
    pub seq: u64,
    pub header: Vec<u8>,
    /// Empty once pruned (DEC-105): the batch is on Stellar and can be
    /// rebuilt from the stored blocks. `batch_len` keeps its size.
    pub batch: Vec<u8>,
    pub batch_len: usize,
    pub first_height: u64,
    pub last_height: u64,
    /// JSON `[[key_hex, amount], ...]` in leaf order.
    pub withdrawals: String,
    pub status: CheckpointStatus,
    pub epoch: Option<u64>,
    /// JSON `[{signer_index, signature}]`.
    pub sigs: Option<String>,
    pub stellar_tx_hash: Option<String>,
    pub stellar_ledger: Option<u32>,
}

/// Snapshots kept below the last accepted checkpoint (spec §14.4, §15, DEC-105).
pub const KEEP_ACCEPTED_SNAPSHOTS: u64 = 3;

/// What the WAL file is truncated to after SQLite checkpoints it.
const WAL_LIMIT_BYTES: i64 = 4 * 1024 * 1024;

/// What one [`Store::prune`] call dropped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    pub snapshots: usize,
    pub batches: usize,
}

/// What happens to the blocks of old accepted checkpoints (F-08).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum History {
    /// The sequencer keeps every block: a checkpoint's blocks become one
    /// deflated blob in `block_archive`, read back transparently.
    Archive,
    /// A validator drops them: what it needs is the snapshots it keeps, the
    /// blocks after them, and Stellar.
    Drop,
}

/// An archive blob: this version byte, then the deflated blocks, each as
/// `u32 LE input length, input, state_hash_after, u32 LE receipts length,
/// receipts`. A node-local format, never hashed or signed.
const ARCHIVE_VERSION: u8 = 1;
const ARCHIVE_LEVEL: u8 = 6;
/// Inflating refuses more than this: a checkpoint's blocks are well under it.
const ARCHIVE_MAX_BYTES: usize = 256 * 1024 * 1024;

type StoredBlock = (BlockRecordV1, Vec<u8>);

fn pack(blocks: &[StoredBlock]) -> Vec<u8> {
    let mut raw = Vec::new();
    for (record, receipts) in blocks {
        raw.extend_from_slice(&(record.input.len() as u32).to_le_bytes());
        raw.extend_from_slice(&record.input);
        raw.extend_from_slice(&record.state_hash_after);
        raw.extend_from_slice(&(receipts.len() as u32).to_le_bytes());
        raw.extend_from_slice(receipts);
    }
    let mut out = vec![ARCHIVE_VERSION];
    out.extend(miniz_oxide::deflate::compress_to_vec(&raw, ARCHIVE_LEVEL));
    out
}

fn unpack(blob: &[u8]) -> Result<Vec<StoredBlock>> {
    const BAD: StoreError = StoreError::Corrupt("block archive");
    let Some((&ARCHIVE_VERSION, deflated)) = blob.split_first() else {
        return Err(BAD);
    };
    let raw = miniz_oxide::inflate::decompress_to_vec_with_limit(deflated, ARCHIVE_MAX_BYTES)
        .map_err(|_| BAD)?;
    let mut at = 0usize;
    let mut take = |n: usize| -> Result<&[u8]> {
        let end = at.checked_add(n).filter(|&e| e <= raw.len()).ok_or(BAD)?;
        let out = &raw[at..end];
        at = end;
        Ok(out)
    };
    let mut out = Vec::new();
    loop {
        let Ok(len) = take(4) else { break };
        let input = take(u32::from_le_bytes(len.try_into().expect("4 bytes")) as usize)?.to_vec();
        let hash: [u8; 32] = take(32)?.try_into().expect("32 bytes");
        let len = take(4)?;
        let receipts =
            take(u32::from_le_bytes(len.try_into().expect("4 bytes")) as usize)?.to_vec();
        out.push((
            BlockRecordV1 {
                input,
                state_hash_after: hash,
            },
            receipts,
        ));
    }
    Ok(out)
}

pub struct Store {
    conn: Connection,
    /// The head (the full state) is written every this many blocks and at
    /// every `CHECKPOINT_END` (F-07); 1 writes it with every block.
    head_every: u64,
    /// Block commits wait for the disk (`synchronous=FULL`) unless set to
    /// [`Durability::Normal`] (F-07).
    blocks: Durability,
    /// The last archive blob read, unpacked: `(first height, blocks)`. A
    /// validator catching up reads one checkpoint's blocks in a row (F-08).
    archive_cache: std::sync::Mutex<Option<(u64, std::sync::Arc<Vec<StoredBlock>>)>>,
}

/// How a block commit reaches the disk (F-07). In WAL mode `Normal` survives
/// a crash of the node but may lose the last blocks to a power loss: right
/// for a validator, which fetches them again, and never for the sequencer,
/// which must not forget a block it has published.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Durability {
    Full,
    Normal,
}

fn i(v: u64) -> i64 {
    v as i64
}

fn u(v: i64) -> u64 {
    v as u64
}

fn arr32(v: Vec<u8>, what: &'static str) -> Result<[u8; 32]> {
    v.try_into().map_err(|_| StoreError::Corrupt(what))
}

impl Store {
    /// Opens (or creates) the store for this lane. A new store starts at
    /// height 0 with the genesis state, which is also snapshot 0.
    pub fn open(
        path: &Path,
        lane_id: &[u8; 32],
        config_hash: &[u8; 32],
        genesis_state: &[u8],
    ) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn, lane_id, config_hash, genesis_state)
    }

    pub fn open_in_memory(
        lane_id: &[u8; 32],
        config_hash: &[u8; 32],
        genesis_state: &[u8],
    ) -> Result<Self> {
        Self::init(
            Connection::open_in_memory()?,
            lane_id,
            config_hash,
            genesis_state,
        )
    }

    fn init(
        conn: Connection,
        lane_id: &[u8; 32],
        config_hash: &[u8; 32],
        genesis_state: &[u8],
    ) -> Result<Self> {
        // Free pages go back to the file system as the store prunes (F-06).
        // It takes effect on a new file; `compact` converts an older store
        // with its VACUUM.
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        // The WAL file shrinks back to this after a checkpoint of SQLite's.
        conn.pragma_update(None, "journal_size_limit", WAL_LIMIT_BYTES)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        let mut store = Self {
            conn,
            head_every: 1,
            blocks: Durability::Full,
            archive_cache: std::sync::Mutex::new(None),
        };
        match store.meta("lane_id")? {
            Some(stored) => {
                if stored != lane_id.as_slice()
                    || store.meta("config_hash")?.as_deref() != Some(config_hash.as_slice())
                {
                    return Err(StoreError::WrongLane);
                }
            }
            None => {
                let tx = store.conn.transaction()?;
                tx.execute(
                    "INSERT INTO meta (key, value) VALUES ('lane_id', ?1)",
                    params![lane_id.as_slice()],
                )?;
                tx.execute(
                    "INSERT INTO meta (key, value) VALUES ('config_hash', ?1)",
                    params![config_hash.as_slice()],
                )?;
                tx.execute(
                    "INSERT INTO meta (key, value) VALUES ('schema', ?1)",
                    params![SCHEMA_VERSION],
                )?;
                tx.execute(
                    "INSERT INTO head (id, height, state) VALUES (0, 0, ?1)",
                    params![genesis_state],
                )?;
                tx.execute(
                    "INSERT INTO snapshots (seq, height, state) VALUES (0, 0, ?1)",
                    params![genesis_state],
                )?;
                tx.commit()?;
            }
        }
        Ok(store)
    }

    fn meta(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    // --- Blocks and state -------------------------------------------------------

    /// Writes the head every `blocks` blocks (and at every checkpoint end)
    /// instead of with every block (F-07). The blocks after it are re-executed
    /// when the store is opened again.
    pub fn set_head_every(&mut self, blocks: u64) {
        self.head_every = blocks.max(1);
    }

    /// How block commits reach the disk (F-07). [`Store::record_signed`]
    /// always waits for the disk.
    pub fn set_block_durability(&mut self, d: Durability) -> Result<()> {
        self.blocks = d;
        self.sync(d)
    }

    fn sync(&self, d: Durability) -> Result<()> {
        let mode = match d {
            Durability::Full => "FULL",
            Durability::Normal => "NORMAL",
        };
        self.conn.pragma_update(None, "synchronous", mode)?;
        Ok(())
    }

    /// `(height, state bytes)` of the persisted head: the state after the
    /// last stored block, or (F-07) after an earlier one, at most
    /// `head_every` blocks back. [`Store::tip`] is the last stored block.
    pub fn head(&self) -> Result<(u64, Vec<u8>)> {
        let (h, s): (i64, Vec<u8>) =
            self.conn
                .query_row("SELECT height, state FROM head WHERE id = 0", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?;
        Ok((u(h), s))
    }

    /// The height of the last stored block (0 before any).
    pub fn tip(&self) -> Result<u64> {
        let h: i64 = self
            .conn
            .prepare_cached("SELECT COALESCE(MAX(height), 0) FROM blocks")?
            .query_row([], |r| r.get(0))?;
        Ok(u(h))
    }

    /// Writes the head: `state` is the state after block `height`.
    pub fn persist_head(&mut self, height: u64, state: &[u8]) -> Result<()> {
        self.conn
            .prepare_cached("UPDATE head SET height = ?1, state = ?2 WHERE id = 0")?
            .execute(params![i(height), state])?;
        Ok(())
    }

    /// Stores block `height` with its receipts and the state it produced, in
    /// one transaction. `checkpoint_seq` is set for a `CHECKPOINT_END` block,
    /// whose output state becomes that checkpoint's snapshot. The head is
    /// written at a checkpoint end and every `head_every` blocks (F-07).
    pub fn commit_block(
        &mut self,
        height: u64,
        record: &BlockRecordV1,
        receipts: &[u8],
        state: &[u8],
        checkpoint_seq: Option<u64>,
    ) -> Result<()> {
        // Cached statements: these run on every block (F-06).
        let tx = self.conn.transaction()?;
        let tip: i64 = tx
            .prepare_cached("SELECT COALESCE(MAX(height), 0) FROM blocks")?
            .query_row([], |r| r.get(0))?;
        if u(tip) + 1 != height {
            return Err(StoreError::Conflict("block height is not tip + 1"));
        }
        tx.prepare_cached(
            "INSERT INTO blocks (height, input, state_hash_after, receipts) VALUES (?1, ?2, ?3, ?4)",
        )?
        .execute(params![i(height), record.input, record.state_hash_after.as_slice(), receipts])?;
        if checkpoint_seq.is_some() || height.is_multiple_of(self.head_every) {
            tx.prepare_cached("UPDATE head SET height = ?1, state = ?2 WHERE id = 0")?
                .execute(params![i(height), state])?;
        }
        if let Some(seq) = checkpoint_seq {
            tx.prepare_cached("INSERT INTO snapshots (seq, height, state) VALUES (?1, ?2, ?3)")?
                .execute(params![i(seq), i(height), state])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Block `height` and its receipts bytes, from `blocks` or the archive.
    /// `None` past the tip, or below a validator's pruned history (see
    /// [`Store::first_block`]).
    pub fn block(&self, height: u64) -> Result<Option<(BlockRecordV1, Vec<u8>)>> {
        if let Some(b) = self.raw_block(height)? {
            return Ok(Some(b));
        }
        Ok(self
            .archive_holding(height)?
            .and_then(|(first, blocks)| blocks.get((height - first) as usize).cloned()))
    }

    fn raw_block(&self, height: u64) -> Result<Option<(BlockRecordV1, Vec<u8>)>> {
        let row: Option<(Vec<u8>, Vec<u8>, Vec<u8>)> = self
            .conn
            .prepare_cached(
                "SELECT input, state_hash_after, receipts FROM blocks WHERE height = ?1",
            )?
            .query_row(params![i(height)], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()?;
        row.map(|(input, hash, receipts)| {
            Ok((
                BlockRecordV1 {
                    input,
                    state_hash_after: arr32(hash, "state_hash_after")?,
                },
                receipts,
            ))
        })
        .transpose()
    }

    /// Blocks `from..=to` with their receipts, in order, from the archive
    /// and `blocks`. A missing one is an error.
    pub fn blocks(&self, from: u64, to: u64) -> Result<Vec<(BlockRecordV1, Vec<u8>)>> {
        if from > to {
            return Ok(Vec::new());
        }
        let raw_from: Option<i64> = self
            .conn
            .prepare_cached("SELECT MIN(height) FROM blocks WHERE height BETWEEN ?1 AND ?2")?
            .query_row(params![i(from), i(to)], |r| r.get(0))?;
        let raw_from = raw_from.map_or(to + 1, u);
        let mut out = Vec::new();
        let mut h = from;
        while h < raw_from {
            let (first, blocks) = self
                .archive_holding(h)?
                .ok_or(StoreError::Corrupt("missing blocks"))?;
            let last = (first + blocks.len() as u64 - 1).min(raw_from - 1);
            out.extend_from_slice(&blocks[(h - first) as usize..=(last - first) as usize]);
            h = last + 1;
        }
        if raw_from <= to {
            out.extend(self.raw_blocks(raw_from, to)?);
        }
        if out.len() as u64 != to - from + 1 {
            return Err(StoreError::Corrupt("missing blocks"));
        }
        Ok(out)
    }

    fn raw_blocks(&self, from: u64, to: u64) -> Result<Vec<(BlockRecordV1, Vec<u8>)>> {
        let mut stmt = self.conn.prepare_cached("SELECT input, state_hash_after, receipts FROM blocks WHERE height BETWEEN ?1 AND ?2 ORDER BY height")?;
        let rows = stmt.query_map(params![i(from), i(to)], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (input, hash, receipts) = row?;
            out.push((
                BlockRecordV1 {
                    input,
                    state_hash_after: arr32(hash, "state_hash_after")?,
                },
                receipts,
            ));
        }
        Ok(out)
    }

    /// The archived checkpoint that holds `height`, unpacked:
    /// `(its first height, its blocks)`.
    fn archive_holding(
        &self,
        height: u64,
    ) -> Result<Option<(u64, std::sync::Arc<Vec<StoredBlock>>)>> {
        let mut cache = self.archive_cache.lock().expect("archive cache");
        if let Some((first, blocks)) = cache.as_ref() {
            if (*first..*first + blocks.len() as u64).contains(&height) {
                return Ok(Some((*first, blocks.clone())));
            }
        }
        let row: Option<(i64, i64, Vec<u8>)> = self
            .conn
            .prepare_cached(
                "SELECT first_height, last_height, blob FROM block_archive WHERE first_height <= ?1 ORDER BY first_height DESC LIMIT 1",
            )?
            .query_row(params![i(height)], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        let Some((first, last, blob)) = row else {
            return Ok(None);
        };
        if u(last) < height {
            return Ok(None);
        }
        let blocks = unpack(&blob)?;
        if blocks.len() as u64 != u(last) - u(first) + 1 {
            return Err(StoreError::Corrupt("block archive"));
        }
        let blocks = std::sync::Arc::new(blocks);
        *cache = Some((u(first), blocks.clone()));
        Ok(Some((u(first), blocks)))
    }

    /// The lowest block height this store can serve, if any (F-08): 1 on a
    /// sequencer; on a validator, the first block after its oldest kept
    /// snapshot.
    pub fn first_block(&self) -> Result<Option<u64>> {
        let v: Option<i64> = self.conn.query_row(
            "SELECT MIN(h) FROM (SELECT MIN(height) AS h FROM blocks UNION ALL SELECT MIN(first_height) FROM block_archive)",
            [],
            |r| r.get(0),
        )?;
        Ok(v.map(u))
    }

    /// The snapshot taken at block `height`, if the store keeps one:
    /// `(seq, state)`.
    pub fn snapshot_at(&self, height: u64) -> Result<Option<(u64, Vec<u8>)>> {
        let row: Option<(i64, Vec<u8>)> = self
            .conn
            .query_row(
                "SELECT seq, state FROM snapshots WHERE height = ?1",
                params![i(height)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.map(|(seq, state)| (u(seq), state)))
    }

    /// Archives or drops the blocks of up to `limit` accepted checkpoints,
    /// oldest first, up to and including the oldest snapshot
    /// [`Store::prune`] keeps: the blocks left start right after a kept
    /// snapshot (F-08). Returns how many checkpoints' blocks it handled.
    pub fn prune_history(&mut self, mode: History, limit: usize) -> Result<usize> {
        let Some(accepted) = self.last_checkpoint_with(CheckpointStatus::Accepted)? else {
            return Ok(0);
        };
        let keep_from = accepted.saturating_sub(KEEP_ACCEPTED_SNAPSHOTS);
        // Checkpoints are archived in seq order; on a validator, blocks
        // before the first one left are gone.
        let after: i64 = match mode {
            History::Archive => {
                self.conn
                    .query_row("SELECT COALESCE(MAX(seq), 0) FROM block_archive", [], |r| {
                        r.get(0)
                    })?
            }
            History::Drop => 0,
        };
        let rows: Vec<(i64, i64, i64)> = {
            let mut stmt = self.conn.prepare(
                "SELECT seq, first_height, last_height FROM checkpoints WHERE status = 'accepted' AND seq > ?1 AND seq <= ?2 AND EXISTS (SELECT 1 FROM blocks WHERE height BETWEEN first_height AND last_height) ORDER BY seq LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![after, i(keep_from), limit as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        let mut done = 0;
        for (seq, first, last) in rows {
            let tx = self.conn.transaction()?;
            if mode == History::Archive {
                let blocks = {
                    let mut stmt = tx.prepare_cached("SELECT input, state_hash_after, receipts FROM blocks WHERE height BETWEEN ?1 AND ?2 ORDER BY height")?;
                    let rows = stmt.query_map(params![first, last], |r| {
                        Ok((
                            r.get::<_, Vec<u8>>(0)?,
                            r.get::<_, Vec<u8>>(1)?,
                            r.get::<_, Vec<u8>>(2)?,
                        ))
                    })?;
                    let mut out = Vec::new();
                    for row in rows {
                        let (input, hash, receipts) = row?;
                        out.push((
                            BlockRecordV1 {
                                input,
                                state_hash_after: arr32(hash, "state_hash_after")?,
                            },
                            receipts,
                        ));
                    }
                    out
                };
                if blocks.len() as i64 != last - first + 1 {
                    return Err(StoreError::Corrupt("missing blocks"));
                }
                tx.execute(
                    "INSERT INTO block_archive (seq, first_height, last_height, blob) VALUES (?1, ?2, ?3, ?4)",
                    params![seq, first, last, pack(&blocks)],
                )?;
            }
            tx.execute(
                "DELETE FROM blocks WHERE height BETWEEN ?1 AND ?2",
                params![first, last],
            )?;
            tx.commit()?;
            done += 1;
        }
        if done > 0 {
            self.release_free_pages()?;
        }
        Ok(done)
    }

    /// `(height, state)` right after checkpoint `seq` (0 = genesis).
    pub fn snapshot(&self, seq: u64) -> Result<Option<(u64, Vec<u8>)>> {
        let row: Option<(i64, Vec<u8>)> = self
            .conn
            .query_row(
                "SELECT height, state FROM snapshots WHERE seq = ?1",
                params![i(seq)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.map(|(h, s)| (u(h), s)))
    }

    // --- Checkpoints -------------------------------------------------------------

    pub fn insert_checkpoint(&mut self, row: &CheckpointRow) -> Result<()> {
        self.conn.execute(
            "INSERT INTO checkpoints (seq, header, batch, first_height, last_height, withdrawals, status, epoch, sigs, stellar_tx_hash, stellar_ledger, batch_len)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                i(row.seq),
                row.header,
                row.batch,
                i(row.first_height),
                i(row.last_height),
                row.withdrawals,
                row.status.as_str(),
                row.epoch.map(i),
                row.sigs,
                row.stellar_tx_hash,
                row.stellar_ledger,
                row.batch.len() as i64
            ],
        )?;
        Ok(())
    }

    pub fn set_signed(&mut self, seq: u64, epoch: u64, sigs_json: &str) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE checkpoints SET status = 'signed', epoch = ?2, sigs = ?3 WHERE seq = ?1 AND status = 'sequenced'",
            params![i(seq), i(epoch), sigs_json],
        )?;
        if n == 1 {
            Ok(())
        } else {
            Err(StoreError::Conflict(
                "checkpoint is not waiting for signatures",
            ))
        }
    }

    /// Idempotent: a second report of the same acceptance is fine.
    pub fn set_accepted(&mut self, seq: u64, stellar_tx_hash: &str, ledger: u32) -> Result<()> {
        let row = self
            .checkpoint(seq)?
            .ok_or(StoreError::Conflict("unknown checkpoint"))?;
        match row.status {
            CheckpointStatus::Accepted
                if row.stellar_tx_hash.as_deref() == Some(stellar_tx_hash) =>
            {
                Ok(())
            }
            CheckpointStatus::Accepted => Err(StoreError::Conflict(
                "checkpoint already accepted with another transaction",
            )),
            CheckpointStatus::Sequenced => Err(StoreError::Conflict("checkpoint is not signed")),
            CheckpointStatus::Signed => {
                self.conn.execute(
                    "UPDATE checkpoints SET status = 'accepted', stellar_tx_hash = ?2, stellar_ledger = ?3 WHERE seq = ?1",
                    params![i(seq), stellar_tx_hash, ledger],
                )?;
                Ok(())
            }
        }
    }

    /// Sequencer, after a signer rotation: checkpoints signed under an epoch
    /// older than `epoch` and not accepted yet go back to waiting for
    /// signatures, so the current set signs them. Their old signatures stay:
    /// the header does not hold the epoch, so those of validators still in
    /// the set are reused. Returns their seqs, lowest first.
    pub fn unsign_before_epoch(&mut self, epoch: u64) -> Result<Vec<u64>> {
        let tx = self.conn.transaction()?;
        let seqs = {
            let mut stmt = tx.prepare(
                "SELECT seq FROM checkpoints WHERE status = 'signed' AND epoch < ?1 ORDER BY seq",
            )?;
            let rows = stmt.query_map(params![i(epoch)], |r| r.get::<_, i64>(0))?;
            rows.map(|r| r.map(u))
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        tx.execute(
            "UPDATE checkpoints SET status = 'sequenced', epoch = NULL WHERE status = 'signed' AND epoch < ?1",
            params![i(epoch)],
        )?;
        tx.commit()?;
        Ok(seqs)
    }

    /// Validators: every checkpoint up to `seq` is accepted on Stellar (the
    /// contract accepts them in order).
    pub fn mark_accepted_through(&mut self, seq: u64) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE checkpoints SET status = 'accepted' WHERE seq <= ?1 AND status != 'accepted'",
            params![i(seq)],
        )?)
    }

    pub fn checkpoint(&self, seq: u64) -> Result<Option<CheckpointRow>> {
        self.conn
            .query_row(
                "SELECT seq, header, batch, first_height, last_height, withdrawals, status, epoch, sigs, stellar_tx_hash, stellar_ledger, COALESCE(batch_len, length(batch)) FROM checkpoints WHERE seq = ?1",
                params![i(seq)],
                row_of,
            )
            .optional()?
            .transpose()
    }

    /// Checkpoints with `status`, lowest seq first.
    pub fn checkpoints_with(&self, status: CheckpointStatus) -> Result<Vec<CheckpointRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, header, batch, first_height, last_height, withdrawals, status, epoch, sigs, stellar_tx_hash, stellar_ledger, COALESCE(batch_len, length(batch)) FROM checkpoints WHERE status = ?1 ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![status.as_str()], row_of)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??);
        }
        Ok(out)
    }

    /// The lowest-seq checkpoint with `status`: what the signer and the
    /// relayer take next, without reading the others' batches (F-06).
    pub fn first_checkpoint_with(&self, status: CheckpointStatus) -> Result<Option<CheckpointRow>> {
        self.conn
            .prepare_cached(
                "SELECT seq, header, batch, first_height, last_height, withdrawals, status, epoch, sigs, stellar_tx_hash, stellar_ledger, COALESCE(batch_len, length(batch)) FROM checkpoints WHERE status = ?1 ORDER BY seq LIMIT 1",
            )?
            .query_row(params![status.as_str()], row_of)
            .optional()?
            .transpose()
    }

    /// How many checkpoints have `status`.
    pub fn count_checkpoints_with(&self, status: CheckpointStatus) -> Result<usize> {
        let n: i64 = self
            .conn
            .prepare_cached("SELECT COUNT(*) FROM checkpoints WHERE status = ?1")?
            .query_row(params![status.as_str()], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// `(seq, header, withdrawals)` of every checkpoint accepted on Stellar,
    /// lowest first, without the batch: what withdrawal proofs need.
    pub fn accepted_withdrawals(&self) -> Result<Vec<(u64, Vec<u8>, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, header, withdrawals FROM checkpoints WHERE status = 'accepted' ORDER BY seq",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                u(r.get::<_, i64>(0)?),
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Drops what the store no longer needs (DEC-105), at most `limit` rows
    /// of each kind per call, so no call holds the store for long:
    /// - snapshots older than the last accepted checkpoint minus
    ///   [`KEEP_ACCEPTED_SNAPSHOTS`], except genesis (seq 0). The escape tree
    ///   and `export-proofs` read the last accepted one; newer ones stay too.
    /// - the batch bytes of accepted checkpoints older than that: the batch
    ///   is on Stellar and can be rebuilt from `blocks`; `batch_len` keeps
    ///   its size.
    ///
    /// Blocks, headers, withdrawal leaves and the inbox are never pruned.
    /// Cleared flags go too, and with an incremental store the free pages
    /// go back to the file system (F-06).
    pub fn prune(&mut self, limit: usize) -> Result<Pruned> {
        let Some(accepted) = self.last_checkpoint_with(CheckpointStatus::Accepted)? else {
            return Ok(Pruned::default());
        };
        let keep_from = accepted.saturating_sub(KEEP_ACCEPTED_SNAPSHOTS);
        let tx = self.conn.transaction()?;
        let snapshots = tx.execute(
            "DELETE FROM snapshots WHERE seq IN (SELECT seq FROM snapshots WHERE seq > 0 AND seq < ?1 ORDER BY seq LIMIT ?2)",
            params![i(keep_from), limit as i64],
        )?;
        let batches = tx.execute(
            "UPDATE checkpoints SET batch_len = length(batch), batch = x'' WHERE seq IN (SELECT seq FROM checkpoints WHERE status = 'accepted' AND seq < ?1 AND length(batch) > 0 ORDER BY seq LIMIT ?2)",
            params![i(keep_from), limit as i64],
        )?;
        tx.execute("DELETE FROM flags WHERE cleared = 1", [])?;
        tx.commit()?;
        self.release_free_pages()?;
        Ok(Pruned { snapshots, batches })
    }

    /// Hands the free pages (from pruning, and from the head row rewritten
    /// every block) back to the file system. A no-op on a store made before
    /// F-06 until `compact` converts it.
    fn release_free_pages(&self) -> Result<()> {
        let free: i64 = self
            .conn
            .query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
        if free > 0 {
            // One page per step: step through them all.
            let mut stmt = self.conn.prepare("PRAGMA incremental_vacuum")?;
            let mut rows = stmt.query([])?;
            while rows.next()?.is_some() {}
        }
        Ok(())
    }

    /// Gives the space freed by [`Store::prune`] back to the file system.
    /// Rewrites the whole file: run it with the node stopped.
    pub fn vacuum(&self) -> Result<()> {
        self.conn.execute_batch("VACUUM")?;
        Ok(())
    }

    /// The highest seq with `status`, if any.
    pub fn last_checkpoint_with(&self, status: CheckpointStatus) -> Result<Option<u64>> {
        let v: Option<i64> = self.conn.query_row(
            "SELECT MAX(seq) FROM checkpoints WHERE status = ?1",
            params![status.as_str()],
            |r| r.get(0),
        )?;
        Ok(v.map(u))
    }

    /// The highest checkpoint seq stored, whatever its status.
    pub fn last_checkpoint_seq(&self) -> Result<Option<u64>> {
        let v: Option<i64> = self
            .conn
            .query_row("SELECT MAX(seq) FROM checkpoints", [], |r| r.get(0))?;
        Ok(v.map(u))
    }

    // --- Inbox and feeds -----------------------------------------------------------

    /// Records inbox message `index` as reported from Stellar. Re-reporting
    /// the same message is fine; a different one is a conflict.
    pub fn put_inbox(&mut self, msg: &InboxMsgV1, acc_after: &[u8; 32]) -> Result<()> {
        let bytes = msg.encode();
        let existing: Option<(Vec<u8>, Vec<u8>)> = self
            .conn
            .query_row(
                "SELECT msg, acc_after FROM inbox WHERE idx = ?1",
                params![i(msg.index)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match existing {
            Some((m, a)) if m == bytes && a == acc_after => Ok(()),
            Some(_) => Err(StoreError::Conflict(
                "inbox message differs from the one recorded",
            )),
            None => {
                self.conn.execute(
                    "INSERT INTO inbox (idx, msg, acc_after) VALUES (?1, ?2, ?3)",
                    params![i(msg.index), bytes.as_slice(), acc_after.as_slice()],
                )?;
                Ok(())
            }
        }
    }

    /// Inbox messages from `index` on, in order, with their `acc_after`.
    pub fn inbox_from(&self, index: u64, limit: usize) -> Result<Vec<(InboxMsgV1, [u8; 32])>> {
        let mut stmt = self
            .conn
            .prepare("SELECT msg, acc_after FROM inbox WHERE idx >= ?1 ORDER BY idx LIMIT ?2")?;
        let rows = stmt.query_map(params![i(index), limit as i64], |r| {
            Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (m, a) = row?;
            out.push((
                InboxMsgV1::decode(&m).map_err(|_| StoreError::Corrupt("inbox msg"))?,
                arr32(a, "acc_after")?,
            ));
        }
        Ok(out)
    }

    /// `acc_after` of message `index`.
    pub fn inbox_acc(&self, index: u64) -> Result<Option<[u8; 32]>> {
        let v: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT acc_after FROM inbox WHERE idx = ?1",
                params![i(index)],
                |r| r.get(0),
            )
            .optional()?;
        v.map(|a| arr32(a, "acc_after")).transpose()
    }

    /// The number of inbox messages recorded (they are contiguous from 0).
    pub fn inbox_count(&self) -> Result<u64> {
        let v: Option<i64> = self
            .conn
            .query_row("SELECT MAX(idx) FROM inbox", [], |r| r.get(0))?;
        Ok(v.map_or(0, |m| u(m) + 1))
    }

    /// `Σ amount` of DEPOSIT messages with index `< through` (the contract's `cum_deposits`).
    pub fn cum_deposits(&self, through: u64) -> Result<i128> {
        let mut total = 0i128;
        for (m, _) in self.inbox_from(0, through as usize)? {
            if m.index < through && m.kind == caravel_core::inbox::InboxKind::Deposit {
                total += m.amount;
            }
        }
        Ok(total)
    }

    /// Keeps the latest feed update for `slot`. The table is still called
    /// `oracle` (its M0 name, keyed by market id), so M0 stores open as they are.
    pub fn put_feed(&mut self, slot: u32, update: &[u8]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO oracle (market_id, update_bytes) VALUES (?1, ?2) ON CONFLICT(market_id) DO UPDATE SET update_bytes = excluded.update_bytes",
            params![slot, update],
        )?;
        Ok(())
    }

    /// The latest update of every feed slot, by slot.
    pub fn feed_updates(&self) -> Result<Vec<(u32, Vec<u8>)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT market_id, update_bytes FROM oracle ORDER BY market_id")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, u32>(0)?, r.get::<_, Vec<u8>>(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    // --- Validator: signed headers and flagged blocks --------------------------------------

    /// The header hash signed for `seq`, if any.
    pub fn signed(&self, seq: u64) -> Result<Option<[u8; 32]>> {
        let v: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT header_hash FROM signed WHERE seq = ?1",
                params![i(seq)],
                |r| r.get(0),
            )
            .optional()?;
        v.map(|h| arr32(h, "signed header hash")).transpose()
    }

    pub fn last_signed_seq(&self) -> Result<Option<u64>> {
        let v: Option<i64> = self
            .conn
            .query_row("SELECT MAX(seq) FROM signed", [], |r| r.get(0))?;
        Ok(v.map(u))
    }

    /// Records a signature before it is returned. Refuses a second, different header for a seq.
    pub fn record_signed(&mut self, seq: u64, header_hash: &[u8; 32]) -> Result<()> {
        match self.signed(seq)? {
            Some(h) if h == *header_hash => Ok(()),
            Some(_) => Err(StoreError::Conflict(
                "a different header was already signed for this seq",
            )),
            None => {
                // The equivocation guard: on disk before the signature
                // leaves, whatever the blocks' durability (F-07).
                if self.blocks != Durability::Full {
                    self.sync(Durability::Full)?;
                }
                let r = self.conn.execute(
                    "INSERT INTO signed (seq, header_hash) VALUES (?1, ?2)",
                    params![i(seq), header_hash.as_slice()],
                );
                if self.blocks != Durability::Full {
                    self.sync(self.blocks)?;
                }
                r?;
                Ok(())
            }
        }
    }

    pub fn flag_block(&mut self, height: u64, reason: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO flags (height, reason) VALUES (?1, ?2) ON CONFLICT(height) DO UPDATE SET reason = excluded.reason, cleared = 0",
            params![i(height), reason],
        )?;
        Ok(())
    }

    /// Uncleared flags on blocks `from..=to`.
    pub fn flags_in(&self, from: u64, to: u64) -> Result<Vec<(u64, String)>> {
        let mut stmt = self.conn.prepare("SELECT height, reason FROM flags WHERE cleared = 0 AND height BETWEEN ?1 AND ?2 ORDER BY height")?;
        let rows = stmt.query_map(params![i(from), i(to)], |r| Ok((u(r.get(0)?), r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn clear_flags(&mut self, to: u64) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE flags SET cleared = 1 WHERE height <= ?1",
            params![i(to)],
        )?)
    }
}

/// Brings a store made by an earlier release up to date, idempotently
/// (DEC-105): the `batch_len` column of pruned batches, and the index the
/// per-block checkpoint queries use.
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let has_len = conn
        .prepare("SELECT 1 FROM pragma_table_info('checkpoints') WHERE name = 'batch_len'")?
        .exists([])?;
    if !has_len {
        conn.execute_batch("ALTER TABLE checkpoints ADD COLUMN batch_len INTEGER")?;
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS checkpoints_status_seq ON checkpoints (status, seq)",
    )
}

fn row_of(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<CheckpointRow>> {
    let status: String = r.get(6)?;
    let epoch: Option<i64> = r.get(7)?;
    let seq: i64 = r.get(0)?;
    let header: Vec<u8> = r.get(1)?;
    let batch: Vec<u8> = r.get(2)?;
    let first_height: i64 = r.get(3)?;
    let last_height: i64 = r.get(4)?;
    let withdrawals: String = r.get(5)?;
    let sigs: Option<String> = r.get(8)?;
    let stellar_tx_hash: Option<String> = r.get(9)?;
    let stellar_ledger: Option<u32> = r.get(10)?;
    let batch_len: i64 = r.get(11)?;
    Ok(
        CheckpointStatus::parse(&status).map(|status| CheckpointRow {
            seq: u(seq),
            header,
            batch,
            batch_len: u(batch_len) as usize,
            first_height: u(first_height),
            last_height: u(last_height),
            withdrawals,
            status,
            epoch: epoch.map(u),
            sigs,
            stellar_tx_hash,
            stellar_ledger,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATE: usize = 64 * 1024;

    fn pragma(s: &Store, name: &str) -> i64 {
        s.conn
            .query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
            .unwrap()
    }

    /// 40 blocks, a checkpoint (with a 64 KiB batch and snapshot) every 4,
    /// all accepted on Stellar.
    fn fill(s: &mut Store) {
        for h in 1..=40u64 {
            let record = BlockRecordV1 {
                input: vec![h as u8; 100],
                state_hash_after: [h as u8; 32],
            };
            let end = (h % 4 == 0).then_some(h / 4);
            s.commit_block(h, &record, &[1; 10], &vec![h as u8; STATE], end)
                .unwrap();
            if let Some(seq) = end {
                s.insert_checkpoint(&CheckpointRow {
                    seq,
                    header: vec![1; 100],
                    batch: vec![2; STATE],
                    batch_len: STATE,
                    first_height: h - 3,
                    last_height: h,
                    withdrawals: "[]".into(),
                    status: CheckpointStatus::Sequenced,
                    epoch: None,
                    sigs: None,
                    stellar_tx_hash: None,
                    stellar_ledger: None,
                })
                .unwrap();
            }
        }
        assert_eq!(
            s.count_checkpoints_with(CheckpointStatus::Sequenced)
                .unwrap(),
            10
        );
        assert_eq!(
            s.first_checkpoint_with(CheckpointStatus::Sequenced)
                .unwrap()
                .map(|r| r.seq),
            Some(1)
        );
        for seq in 1..=10 {
            s.set_signed(seq, 1, "[]").unwrap();
            s.set_accepted(seq, &format!("{seq:064x}"), seq as u32)
                .unwrap();
        }
        assert_eq!(
            s.count_checkpoints_with(CheckpointStatus::Accepted)
                .unwrap(),
            10
        );
        assert!(s
            .first_checkpoint_with(CheckpointStatus::Sequenced)
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_new_store_shrinks_as_it_prunes() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("s.sqlite"), &[1; 32], &[2; 32], &[0; 8]).unwrap();
        assert_eq!(pragma(&s, "auto_vacuum"), 2, "incremental");
        fill(&mut s);
        let pages = pragma(&s, "page_count");
        s.flag_block(3, "test").unwrap();
        s.clear_flags(3).unwrap();
        while s.prune(100).unwrap() != Pruned::default() {}
        assert_eq!(pragma(&s, "freelist_count"), 0, "free pages went back");
        assert!(
            pragma(&s, "page_count") < pages * 2 / 3,
            "{} of {pages} pages left",
            pragma(&s, "page_count")
        );
        let flags: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM flags", [], |r| r.get(0))
            .unwrap();
        assert_eq!(flags, 0, "a cleared flag is dropped");
    }

    #[test]
    fn compact_converts_an_older_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.sqlite");
        {
            // A store as releases before F-06 made it.
            let c = Connection::open(&path).unwrap();
            c.pragma_update(None, "journal_mode", "WAL").unwrap();
            c.execute_batch(SCHEMA).unwrap();
        }
        let mut s = Store::open(&path, &[1; 32], &[2; 32], &[0; 8]).unwrap();
        assert_eq!(
            pragma(&s, "auto_vacuum"),
            0,
            "the setting waits for a VACUUM"
        );
        fill(&mut s);
        while s.prune(100).unwrap() != Pruned::default() {}
        assert!(
            pragma(&s, "freelist_count") > 0,
            "freed pages stay in the file"
        );
        s.vacuum().unwrap();
        assert_eq!(pragma(&s, "auto_vacuum"), 2, "converted");
        assert_eq!(pragma(&s, "freelist_count"), 0);
    }

    fn all_blocks(s: &Store) -> Vec<StoredBlock> {
        (1..=40).map(|h| s.block(h).unwrap().unwrap()).collect()
    }

    #[test]
    fn the_sequencer_archives_old_blocks_and_serves_them_as_before() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("s.sqlite"), &[1; 32], &[2; 32], &[0; 8]).unwrap();
        fill(&mut s);
        let before = all_blocks(&s);
        // Accepted through 10, so snapshots from 7 stay: blocks of 1..=7 go
        // to the archive, a pass of 3 checkpoints at a time.
        assert_eq!(s.prune_history(History::Archive, 3).unwrap(), 3);
        assert_eq!(s.prune_history(History::Archive, 3).unwrap(), 3);
        assert_eq!(s.prune_history(History::Archive, 3).unwrap(), 1);
        assert_eq!(s.prune_history(History::Archive, 3).unwrap(), 0);
        let raw: i64 = s
            .conn
            .query_row("SELECT MIN(height) FROM blocks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(raw, 29, "blocks after snapshot 7 stay as rows");
        assert_eq!(s.first_block().unwrap(), Some(1));
        assert_eq!(all_blocks(&s), before, "one at a time");
        assert_eq!(s.blocks(1, 40).unwrap(), before, "a range across both");
        assert_eq!(
            s.blocks(6, 9).unwrap(),
            before[5..9].to_vec(),
            "across two archives"
        );
        assert!(s.block(41).unwrap().is_none());
        // Archived blocks go on growing the chain as before.
        let (r, rc) = before[0].clone();
        s.commit_block(41, &r, &rc, &[0; 8], None).unwrap();
        assert_eq!(s.tip().unwrap(), 41);
    }

    #[test]
    fn a_validator_drops_old_blocks_down_to_a_kept_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("v.sqlite"), &[1; 32], &[2; 32], &[0; 8]).unwrap();
        fill(&mut s);
        let before = all_blocks(&s);
        while s.prune(100).unwrap() != Pruned::default() {}
        assert_eq!(s.prune_history(History::Drop, 100).unwrap(), 7);
        assert_eq!(s.first_block().unwrap(), Some(29));
        assert!(
            s.snapshot_at(28).unwrap().is_some(),
            "the blocks left follow a kept snapshot"
        );
        assert!(s.block(28).unwrap().is_none());
        assert_eq!(s.blocks(29, 40).unwrap(), before[28..].to_vec());
        assert!(s.blocks(20, 40).is_err(), "a range into dropped blocks");
        assert_eq!(s.prune_history(History::Drop, 100).unwrap(), 0);
    }

    #[test]
    fn a_damaged_archive_is_an_error() {
        let blocks = vec![(
            BlockRecordV1 {
                input: vec![7; 50],
                state_hash_after: [3; 32],
            },
            vec![1; 9],
        )];
        assert_eq!(unpack(&pack(&blocks)).unwrap(), blocks);
        let mut blob = pack(&blocks);
        blob[0] = 9;
        assert!(unpack(&blob).is_err(), "another version");
        let blob = pack(&blocks);
        assert!(unpack(&blob[..blob.len() - 3]).is_err(), "cut short");
    }
}
