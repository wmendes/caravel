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

/// What one [`Store::prune`] call dropped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    pub snapshots: usize,
    pub batches: usize,
}

pub struct Store {
    conn: Connection,
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
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        let mut store = Self { conn };
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

    /// `(height, state bytes)` after the last stored block.
    pub fn head(&self) -> Result<(u64, Vec<u8>)> {
        let (h, s): (i64, Vec<u8>) =
            self.conn
                .query_row("SELECT height, state FROM head WHERE id = 0", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?;
        Ok((u(h), s))
    }

    /// Stores block `height` with its receipts and the state it produced, in
    /// one transaction. `checkpoint_seq` is set for a `CHECKPOINT_END` block,
    /// whose output state becomes that checkpoint's snapshot.
    pub fn commit_block(
        &mut self,
        height: u64,
        record: &BlockRecordV1,
        receipts: &[u8],
        state: &[u8],
        checkpoint_seq: Option<u64>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        let head: i64 = tx.query_row("SELECT height FROM head WHERE id = 0", [], |r| r.get(0))?;
        if u(head) + 1 != height {
            return Err(StoreError::Conflict("block height is not head + 1"));
        }
        tx.execute(
            "INSERT INTO blocks (height, input, state_hash_after, receipts) VALUES (?1, ?2, ?3, ?4)",
            params![i(height), record.input, record.state_hash_after.as_slice(), receipts],
        )?;
        tx.execute(
            "UPDATE head SET height = ?1, state = ?2 WHERE id = 0",
            params![i(height), state],
        )?;
        if let Some(seq) = checkpoint_seq {
            tx.execute(
                "INSERT INTO snapshots (seq, height, state) VALUES (?1, ?2, ?3)",
                params![i(seq), i(height), state],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Block `height` and its receipts bytes.
    pub fn block(&self, height: u64) -> Result<Option<(BlockRecordV1, Vec<u8>)>> {
        let row: Option<(Vec<u8>, Vec<u8>, Vec<u8>)> = self
            .conn
            .query_row(
                "SELECT input, state_hash_after, receipts FROM blocks WHERE height = ?1",
                params![i(height)],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
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

    /// Blocks `from..=to` with their receipts, in order.
    pub fn blocks(&self, from: u64, to: u64) -> Result<Vec<(BlockRecordV1, Vec<u8>)>> {
        let mut stmt = self.conn.prepare("SELECT input, state_hash_after, receipts FROM blocks WHERE height BETWEEN ?1 AND ?2 ORDER BY height")?;
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
        if out.len() as u64 != to.saturating_sub(from) + 1 && from <= to {
            return Err(StoreError::Corrupt("missing blocks"));
        }
        Ok(out)
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
        tx.commit()?;
        Ok(Pruned { snapshots, batches })
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
                self.conn.execute(
                    "INSERT INTO signed (seq, header_hash) VALUES (?1, ?2)",
                    params![i(seq), header_hash.as_slice()],
                )?;
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
