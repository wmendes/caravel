//! Price history and recent fills for the perps views (M0.7, H-15, DEC-103).
//!
//! The lane keeps the oracle price in its state, not its history. The node
//! builds the history from its own blocks: every accepted oracle update
//! (`Event::Oracle { accepted: true }`) goes into one-minute candles of the
//! oracle price, which is the mark price. 5, 15 and 60-minute candles are
//! built from those. The latest fills per market are kept the same way.
//!
//! Both are display data, not consensus. They live in memory and in a small
//! SQLite file beside the node's store, written once a minute with the height
//! they reach. At start the node loads the file and replays the stored blocks
//! after that height, so a restart loses nothing and replays little.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use caravel_core::block::BlockInputV1;
use caravel_runtime::store::Store;
use caravel_types::receipts::{Event, Receipts};
use caravel_types::state::StateV1;
use rusqlite::{params, Connection};
use serde::Serialize;

use crate::views::{self, FillView};

/// One minute, the base candle.
pub const MINUTE_MS: u64 = 60_000;
/// One-minute candles kept per market: seven days.
pub const KEEP_MINUTES: usize = 7 * 24 * 60;
/// Fills kept per market for `/v1/markets/{id}/trades`.
pub const FILLS_KEPT: usize = 1000;
/// At most this many stored blocks are replayed at start (about a day at 0.5 s).
pub const WARM_MAX_BLOCKS: u64 = 200_000;
/// The candle intervals the API serves, in minutes.
pub const INTERVALS: [(&str, u64); 4] = [("1m", 1), ("5m", 5), ("15m", 15), ("1h", 60)];

/// One candle of the oracle price, in stroops per lot; `t` is its start (ms).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candle {
    pub t: u64,
    pub o: i64,
    pub h: i64,
    pub l: i64,
    pub c: i64,
}

impl Candle {
    fn new(t: u64, price: i64) -> Self {
        Self {
            t,
            o: price,
            h: price,
            l: price,
            c: price,
        }
    }

    fn add(&mut self, price: i64) {
        self.h = self.h.max(price);
        self.l = self.l.min(price);
        self.c = price;
    }
}

/// A candle as the API serves it: prices as decimal strings, like every price.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct CandleView {
    pub t: u64,
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
}

impl From<Candle> for CandleView {
    fn from(c: Candle) -> Self {
        Self {
            t: c.t,
            open: c.o.to_string(),
            high: c.h.to_string(),
            low: c.l.to_string(),
            close: c.c.to_string(),
        }
    }
}

#[derive(Default)]
pub struct History {
    /// One-minute candles per market, oldest first; the last one may be open.
    candles: BTreeMap<u16, VecDeque<Candle>>,
    /// Latest fills per market, newest first.
    fills: BTreeMap<u16, VecDeque<FillView>>,
    /// Fills not written to the file yet.
    unsaved: Vec<FillView>,
    /// The last block indexed.
    height: u64,
    /// The minute of the last write.
    saved_minute: u64,
    db: Option<Connection>,
}

impl History {
    /// The last block indexed.
    pub fn height(&self) -> u64 {
        self.height
    }

    /// Adds an oracle price at `ts_ms` to `market`'s one-minute candles.
    pub fn add_price(&mut self, market: u16, ts_ms: u64, price: i64) {
        let t = ts_ms - ts_ms % MINUTE_MS;
        let q = self.candles.entry(market).or_default();
        match q.back_mut() {
            Some(c) if c.t == t => c.add(price),
            // A late price for a closed minute is not rewritten into it.
            Some(c) if c.t > t => {}
            _ => {
                q.push_back(Candle::new(t, price));
                while q.len() > KEEP_MINUTES {
                    q.pop_front();
                }
            }
        }
    }

    fn add_fill(&mut self, f: FillView) {
        let q = self.fills.entry(f.market_id).or_default();
        q.push_front(f.clone());
        q.truncate(FILLS_KEPT);
        self.unsaved.push(f);
    }

    /// Indexes one block: its accepted oracle prices and its fills, which it
    /// returns. `st` names the accounts in fills (the state after the block,
    /// or any later one: account slots are never reused).
    pub fn index_block(
        &mut self,
        st: &StateV1,
        height: u64,
        input: &[u8],
        receipts: &[u8],
    ) -> Vec<FillView> {
        let Ok(block) = BlockInputV1::decode(input) else {
            return Vec::new();
        };
        let ts = block.timestamp_ms;
        let Ok(r) = Receipts::decode(receipts) else {
            self.height = height;
            return Vec::new();
        };
        for e in r.receipts.iter().flat_map(|x| x.events.iter()) {
            if let Event::Oracle {
                market,
                price,
                accepted: true,
            } = e
            {
                self.add_price(*market, ts, *price);
            }
        }
        let fills = views::fills(st, height, ts, &r);
        for f in &fills {
            self.add_fill(f.clone());
        }
        self.height = height;
        if ts / MINUTE_MS > self.saved_minute {
            if let Err(e) = self.save() {
                eprintln!("perps history not saved: {e:#}");
            }
            self.saved_minute = ts / MINUTE_MS;
        }
        fills
    }

    /// `market`'s candles of `minutes` minutes, the last `limit`, oldest first.
    pub fn candles(&self, market: u16, minutes: u64, limit: usize) -> Vec<CandleView> {
        let Some(q) = self.candles.get(&market) else {
            return Vec::new();
        };
        let span = minutes * MINUTE_MS;
        let mut out: Vec<Candle> = Vec::new();
        for c in q {
            let t = c.t - c.t % span;
            match out.last_mut() {
                Some(a) if a.t == t => {
                    a.h = a.h.max(c.h);
                    a.l = a.l.min(c.l);
                    a.c = c.c;
                }
                _ => out.push(Candle { t, ..*c }),
            }
        }
        let skip = out.len().saturating_sub(limit);
        out.into_iter().skip(skip).map(CandleView::from).collect()
    }

    /// The latest `limit` fills of `market`, newest first.
    pub fn fills(&self, market: u16, limit: usize) -> Vec<FillView> {
        self.fills
            .get(&market)
            .map(|q| q.iter().take(limit).cloned().collect())
            .unwrap_or_default()
    }

    /// The file beside a store at `db`.
    pub fn path_for(db: &Path) -> PathBuf {
        db.with_file_name("perps-history.sqlite")
    }

    /// Opens (or creates) the file and loads what it holds.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS candles (market INTEGER NOT NULL, t INTEGER NOT NULL, o INTEGER NOT NULL, h INTEGER NOT NULL, l INTEGER NOT NULL, c INTEGER NOT NULL, PRIMARY KEY (market, t));
             CREATE TABLE IF NOT EXISTS fills (id INTEGER PRIMARY KEY AUTOINCREMENT, market INTEGER NOT NULL, json TEXT NOT NULL);",
        )?;
        let height = conn
            .query_row("SELECT v FROM meta WHERE k = 'height'", [], |r| {
                r.get::<_, i64>(0)
            })
            .map_or(0, |v| v as u64);
        let mut h = History {
            height,
            ..History::default()
        };
        {
            let mut stmt =
                conn.prepare("SELECT market, t, o, h, l, c FROM candles ORDER BY market, t")?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)? as u16,
                    Candle {
                        t: r.get::<_, i64>(1)? as u64,
                        o: r.get(2)?,
                        h: r.get(3)?,
                        l: r.get(4)?,
                        c: r.get(5)?,
                    },
                ))
            })?;
            for row in rows {
                let (m, c) = row?;
                let q = h.candles.entry(m).or_default();
                q.push_back(c);
                while q.len() > KEEP_MINUTES {
                    q.pop_front();
                }
            }
        }
        {
            let mut stmt = conn.prepare("SELECT json FROM fills ORDER BY id")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for row in rows {
                if let Ok(f) = serde_json::from_str::<FillView>(&row?) {
                    let q = h.fills.entry(f.market_id).or_default();
                    q.push_front(f);
                    q.truncate(FILLS_KEPT);
                }
            }
        }
        h.saved_minute = h
            .candles
            .values()
            .filter_map(|q| q.back())
            .map(|c| c.t / MINUTE_MS)
            .max()
            .unwrap_or(0);
        h.db = Some(conn);
        Ok(h)
    }

    /// Writes the candles that changed since the last write, the new fills
    /// and the height reached, in one transaction; trims old rows.
    pub fn save(&mut self) -> Result<()> {
        let Some(conn) = self.db.as_mut() else {
            return Ok(());
        };
        let since = self.saved_minute.saturating_sub(1) * MINUTE_MS;
        let tx = conn.transaction()?;
        for (m, q) in &self.candles {
            for c in q.iter().rev().take_while(|c| c.t >= since) {
                tx.execute(
                    "INSERT INTO candles (market, t, o, h, l, c) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT (market, t) DO UPDATE SET o = ?3, h = ?4, l = ?5, c = ?6",
                    params![*m as i64, c.t as i64, c.o, c.h, c.l, c.c],
                )?;
            }
            if let Some(first) = q.front() {
                tx.execute(
                    "DELETE FROM candles WHERE market = ?1 AND t < ?2",
                    params![*m as i64, first.t as i64],
                )?;
            }
        }
        for f in self.unsaved.drain(..) {
            tx.execute(
                "INSERT INTO fills (market, json) VALUES (?1, ?2)",
                params![f.market_id as i64, serde_json::to_string(&f)?],
            )?;
        }
        for m in self.fills.keys() {
            tx.execute(
                "DELETE FROM fills WHERE market = ?1 AND id NOT IN (SELECT id FROM fills WHERE market = ?1 ORDER BY id DESC LIMIT ?2)",
                params![*m as i64, FILLS_KEPT as i64],
            )?;
        }
        tx.execute(
            "INSERT INTO meta (k, v) VALUES ('height', ?1) ON CONFLICT (k) DO UPDATE SET v = ?1",
            params![self.height as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Replays the stored blocks after the last one indexed, up to `head`
    /// (at most [`WARM_MAX_BLOCKS`] of them), then writes the file.
    pub fn catch_up(&mut self, store: &Store, st: &StateV1, head: u64) -> Result<u64> {
        let from = (self.height + 1).max(head.saturating_sub(WARM_MAX_BLOCKS) + 1);
        let mut h = from;
        while h <= head {
            let to = (h + 999).min(head);
            for (i, (record, receipts)) in store.blocks(h, to)?.into_iter().enumerate() {
                self.index_block(st, h + i as u64, &record.input, &receipts);
            }
            h = to + 1;
        }
        self.height = self.height.max(head);
        self.save()?;
        Ok(head.saturating_sub(from) + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minutes_and_intervals() {
        let mut h = History::default();
        // 10:00:05, 10:00:50, 10:01:10, 10:04:59, 10:05:00
        let base = 36_000_000;
        h.add_price(1, base + 5_000, 100);
        h.add_price(1, base + 50_000, 120);
        h.add_price(1, base + 70_000, 90);
        h.add_price(1, base + 299_000, 95);
        h.add_price(1, base + 300_000, 110);
        let m1 = h.candles(1, 1, 10);
        assert_eq!(m1.len(), 4);
        assert_eq!(
            (
                m1[0].open.as_str(),
                m1[0].high.as_str(),
                m1[0].low.as_str(),
                m1[0].close.as_str()
            ),
            ("100", "120", "100", "120")
        );
        let m5 = h.candles(1, 5, 10);
        assert_eq!(m5.len(), 2);
        assert_eq!(m5[0].t, base);
        assert_eq!(
            (
                m5[0].open.as_str(),
                m5[0].high.as_str(),
                m5[0].low.as_str(),
                m5[0].close.as_str()
            ),
            ("100", "120", "90", "95")
        );
        assert_eq!(m5[1].open, "110");
        // limit keeps the latest
        assert_eq!(
            h.candles(1, 1, 2).first().map(|c| c.t),
            Some(base + 240_000)
        );
        // a late price for a closed minute is ignored
        h.add_price(1, base + 10_000, 1);
        assert_eq!(h.candles(1, 1, 10)[0].low, "100");
        assert!(h.candles(2, 1, 10).is_empty());
    }

    #[test]
    fn keeps_seven_days() {
        let mut h = History::default();
        for i in 0..(KEEP_MINUTES as u64 + 10) {
            h.add_price(1, i * MINUTE_MS, i as i64);
        }
        let all = h.candles(1, 1, usize::MAX);
        assert_eq!(all.len(), KEEP_MINUTES);
        assert_eq!(all[0].t, 10 * MINUTE_MS);
    }

    #[test]
    fn survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("perps-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("perps-history.sqlite");
        let _ = std::fs::remove_file(&path);
        {
            let mut h = History::open(&path).unwrap();
            h.add_price(1, 60_000, 100);
            h.add_price(1, 61_000, 130);
            h.add_price(1, 125_000, 90);
            h.height = 42;
            h.saved_minute = 2;
            h.save().unwrap();
        }
        let h = History::open(&path).unwrap();
        assert_eq!(h.height(), 42);
        let c = h.candles(1, 1, 10);
        assert_eq!(c.len(), 2);
        assert_eq!(
            (c[0].open.as_str(), c[0].high.as_str(), c[0].close.as_str()),
            ("100", "130", "130")
        );
        assert_eq!(c[1].close, "90");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
