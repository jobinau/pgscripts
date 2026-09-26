// Latency histograms and counters ------------------------------------------------------
// Every worker task owns its own `TaskStats` (no locking on the hot path); the stats are
// merged once at the end. Only the few live counters needed for progress reports are atomics.
use hdrhistogram::Histogram;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

// Histograms record microseconds, from 1us up to 10 minutes, 3 significant digits.
fn new_hist() -> Histogram<u64> {
    Histogram::new_with_bounds(1, 600_000_000, 3).expect("valid histogram bounds")
}

/// One histogram per step of a transaction.
pub struct Hists {
    pub total: Histogram<u64>,           // whole txn as the application sees it
    pub primary_acquire: Histogram<u64>, // waiting for a primary pool connection
    pub commit: Histogram<u64>,          // INSERT (autocommit) round trip
    pub lsn_fetch: Histogram<u64>,       // SELECT pg_current_wal_insert_lsn()
    pub standby_acquire: Histogram<u64>, // waiting for a standby pool connection
    pub wait: Histogram<u64>,            // WAIT FOR LSN round trip
    pub verify: Histogram<u64>,          // read-your-writes SELECT on the standby
}

impl Hists {
    pub fn new() -> Self {
        Hists {
            total: new_hist(),
            primary_acquire: new_hist(),
            commit: new_hist(),
            lsn_fetch: new_hist(),
            standby_acquire: new_hist(),
            wait: new_hist(),
            verify: new_hist(),
        }
    }

    fn merge(&mut self, o: &Hists) {
        // `add` only fails if the other histogram has values outside our bounds; same bounds here.
        let _ = self.total.add(&o.total);
        let _ = self.primary_acquire.add(&o.primary_acquire);
        let _ = self.commit.add(&o.commit);
        let _ = self.lsn_fetch.add(&o.lsn_fetch);
        let _ = self.standby_acquire.add(&o.standby_acquire);
        let _ = self.wait.add(&o.wait);
        let _ = self.verify.add(&o.verify);
    }

    /// (name, histogram) pairs in display order.
    pub fn named(&self) -> [(&'static str, &Histogram<u64>); 7] {
        [
            ("total", &self.total),
            ("primary_acquire", &self.primary_acquire),
            ("commit", &self.commit),
            ("lsn_fetch", &self.lsn_fetch),
            ("standby_acquire", &self.standby_acquire),
            ("wait_for_lsn", &self.wait),
            ("verify", &self.verify),
        ]
    }
}

/// Record a duration into a histogram (clamped to the histogram bounds).
pub fn rec(h: &mut Histogram<u64>, d: Duration) {
    h.saturating_record(d.as_micros().max(1) as u64);
}

/// Histogram value at a quantile, in milliseconds.
pub fn q_ms(h: &Histogram<u64>, q: f64) -> f64 {
    if h.is_empty() { 0.0 } else { h.value_at_quantile(q) as f64 / 1000.0 }
}

/// Everything one worker task measured.
pub struct TaskStats {
    pub h: Hists,
    pub completed: u64,          // txns the application considers complete
    pub wait_success: u64,
    pub wait_timeout: u64,
    pub wait_not_in_recovery: u64,
    pub wait_other: u64,         // unexpected status string
    pub verify_ok: u64,
    pub verify_missing: u64,     // row not visible on the standby when checked
    pub errors: BTreeMap<&'static str, u64>, // errors by step
    pub last_error: BTreeMap<&'static str, String>,
}

impl TaskStats {
    pub fn new() -> Self {
        TaskStats {
            h: Hists::new(),
            completed: 0,
            wait_success: 0,
            wait_timeout: 0,
            wait_not_in_recovery: 0,
            wait_other: 0,
            verify_ok: 0,
            verify_missing: 0,
            errors: BTreeMap::new(),
            last_error: BTreeMap::new(),
        }
    }

    pub fn error(&mut self, step: &'static str, msg: String) {
        *self.errors.entry(step).or_insert(0) += 1;
        self.last_error.insert(step, msg);
    }

    pub fn error_total(&self) -> u64 {
        self.errors.values().sum()
    }

    pub fn merge(&mut self, o: &TaskStats) {
        self.h.merge(&o.h);
        self.completed += o.completed;
        self.wait_success += o.wait_success;
        self.wait_timeout += o.wait_timeout;
        self.wait_not_in_recovery += o.wait_not_in_recovery;
        self.wait_other += o.wait_other;
        self.verify_ok += o.verify_ok;
        self.verify_missing += o.verify_missing;
        for (k, v) in &o.errors {
            *self.errors.entry(k).or_insert(0) += v;
        }
        for (k, v) in &o.last_error {
            self.last_error.insert(k, v.clone());
        }
    }
}

/// Live counters shared by all tasks, read by the progress reporter.
pub struct Live {
    pub completed: AtomicU64,
    pub failed: AtomicU64, // errors + non-success WAIT FOR statuses
}

impl Live {
    pub fn new() -> Self {
        Live { completed: AtomicU64::new(0), failed: AtomicU64::new(0) }
    }
    pub fn inc_completed(&self) {
        self.completed.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_failed(&self) {
        self.failed.fetch_add(1, Ordering::Relaxed);
    }
}
