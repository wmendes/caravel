//! Per-phase timings (M0.9, F-01): how long each step of making or following a
//! block takes on this machine, as p50 / p99 / max over the last samples.
//!
//! Node-side only: wall-clock time never reaches consensus. It feeds
//! `/v1/status` (`perf`) and the soak reports.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

/// Samples kept per phase: at 500 ms blocks, about the last 8.5 minutes.
pub const WINDOW: usize = 1024;

/// One phase's summary, in microseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct PhaseStats {
    pub n: usize,
    pub p50_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
}

/// A rolling window of samples per named phase.
#[derive(Debug, Default)]
pub struct Perf {
    phases: BTreeMap<&'static str, VecDeque<u64>>,
}

impl Perf {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one sample of `phase`.
    pub fn record(&mut self, phase: &'static str, took: Duration) {
        let q = self.phases.entry(phase).or_default();
        if q.len() == WINDOW {
            q.pop_front();
        }
        q.push_back(u64::try_from(took.as_micros()).unwrap_or(u64::MAX));
    }

    /// Times `f` as `phase`.
    pub fn time<T>(&mut self, phase: &'static str, f: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let out = f();
        self.record(phase, start.elapsed());
        out
    }

    /// Every phase's p50 / p99 / max over its window.
    pub fn summary(&self) -> BTreeMap<&'static str, PhaseStats> {
        self.phases
            .iter()
            .filter(|(_, q)| !q.is_empty())
            .map(|(name, q)| {
                let mut v: Vec<u64> = q.iter().copied().collect();
                v.sort_unstable();
                let at = |p: usize| v[(v.len() - 1) * p / 100];
                let stats = PhaseStats {
                    n: v.len(),
                    p50_us: at(50),
                    p99_us: at(99),
                    max_us: *v.last().expect("non-empty"),
                };
                (*name, stats)
            })
            .collect()
    }

    /// Adds `other`'s phases to this summary (a node merges its own timings
    /// with its core's).
    pub fn merged(&self, other: &Perf) -> BTreeMap<&'static str, PhaseStats> {
        let mut out = self.summary();
        out.extend(other.summary());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_over_the_window() {
        let mut p = Perf::new();
        for us in 1..=100u64 {
            p.record("build", Duration::from_micros(us));
        }
        let s = p.summary()["build"];
        assert_eq!(s.n, 100);
        assert_eq!(s.p50_us, 50);
        assert_eq!(s.p99_us, 99);
        assert_eq!(s.max_us, 100);
    }

    #[test]
    fn the_window_drops_the_oldest() {
        let mut p = Perf::new();
        p.record("commit", Duration::from_secs(9));
        for _ in 0..WINDOW {
            p.record("commit", Duration::from_micros(10));
        }
        let s = p.summary()["commit"];
        assert_eq!(s.n, WINDOW);
        assert_eq!(s.max_us, 10, "the 9 s outlier left the window");
    }

    #[test]
    fn time_returns_the_value_and_records() {
        let mut p = Perf::new();
        assert_eq!(p.time("seal", || 7), 7);
        assert_eq!(p.summary()["seal"].n, 1);
        let mut q = Perf::new();
        q.record("lock_wait", Duration::from_micros(3));
        let m = p.merged(&q);
        assert!(m.contains_key("seal") && m.contains_key("lock_wait"));
    }
}
