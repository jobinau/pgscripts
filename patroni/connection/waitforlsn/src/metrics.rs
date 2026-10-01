//! Latency histograms, counters and pool usage accumulators.
//!
//! Design (DESIGN.md D8, D9):
//! - Every worker task owns its own [`TaskStats`], so the hot path takes no locks. The stats
//!   are merged once, after all tasks have finished ([`TaskStats::merge`]).
//! - Latencies go into HDR histograms in **microseconds** ([`Hists`]), which merge without
//!   losing precision. The final percentiles are exact over all transactions, not averages
//!   of per-task percentiles.
//! - [`Live`] holds the only counters shared while the run is in progress (relaxed atomics),
//!   for the progress lines.
//! - [`PoolUsage`] / [`UsageAcc`] accumulate samples of deadpool's `Pool::status()`, taken by
//!   the `pool_sampler` task in `main.rs`.
use hdrhistogram::Histogram;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Create an empty latency histogram: microseconds from 1µs to 10 minutes, 3 significant
/// digits (a value is stored with ≤0.1% error). Anything longer is clamped by [`rec`].
fn new_hist() -> Histogram<u64> {
    Histogram::new_with_bounds(1, 600_000_000, 3).expect("valid histogram bounds")
}

/// One latency histogram per step of a transaction (see DESIGN.md §3.2 for where each
/// step is timed). Field names match the rows of the summary table.
pub struct Hists {
    /// Whole transaction as the application sees it. Completed transactions only (D6).
    pub total: Histogram<u64>,
    /// Waiting for a connection from the primary pool (`pool.get()`).
    pub primary_acquire: Histogram<u64>,
    /// The autocommit `INSERT ... RETURNING id` round trip, including the commit.
    pub commit: Histogram<u64>,
    /// `SELECT pg_current_wal_insert_lsn()` after the commit (D2).
    pub lsn_fetch: Histogram<u64>,
    /// Waiting for a connection from the standby pool.
    pub standby_acquire: Histogram<u64>,
    /// `WAIT FOR LSN` round trip, for every status (so slow timeouts stay visible).
    pub wait: Histogram<u64>,
    /// Read-your-writes `SELECT` on the standby (`--verify`).
    pub verify: Histogram<u64>,
}

impl Hists {
    /// All histograms empty.
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

    /// Add all values of `o` into `self`, histogram by histogram.
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

    /// `(name, histogram)` pairs in display order, used to print the summary table.
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

/// Record a duration into a histogram as microseconds. Values below 1µs count as 1µs, and
/// values above the histogram's upper bound are clamped instead of being dropped.
pub fn rec(h: &mut Histogram<u64>, d: Duration) {
    h.saturating_record(d.as_micros().max(1) as u64);
}

/// Histogram value at quantile `q` (0.0–1.0), converted to milliseconds. An empty
/// histogram gives 0.0, so CSV columns for unused steps are 0 rather than garbage.
pub fn q_ms(h: &Histogram<u64>, q: f64) -> f64 {
    if h.is_empty() { 0.0 } else { h.value_at_quantile(q) as f64 / 1000.0 }
}

/// Everything one worker task measured. Owned by the task while it runs, then returned
/// and merged into the run total.
pub struct TaskStats {
    /// Latency histograms per step.
    pub h: Hists,
    /// Transactions the application considers complete (D6).
    pub completed: u64,
    /// `WAIT FOR` returned `success`.
    pub wait_success: u64,
    /// `WAIT FOR` returned `timeout` (`--wait-timeout` expired on the standby).
    pub wait_timeout: u64,
    /// `WAIT FOR` returned `not in recovery` (the standby was promoted).
    pub wait_not_in_recovery: u64,
    /// `WAIT FOR` returned a status string this program doesn't know.
    pub wait_other: u64,
    /// `--verify` found the row on the standby.
    pub verify_ok: u64,
    /// `--verify` did not find the row on the standby when it checked.
    pub verify_missing: u64,
    /// Commit LSNs that pointed just past a WAL page header (DESIGN.md D2). These are the
    /// targets that can stall `WAIT FOR`: corrected normally, waited for as-is with
    /// `--raw-insert-lsn`. REPRO: compare with `wait_timeout` (see repro/REPRODUCE_WITH_WAITFORLSN.md).
    pub boundary_lsns: u64,
    /// WAL positions captured right after the latest `WAIT FOR` timeout (see
    /// `workload::diagnose_timeout`). Taken and printed by `run_task`.
    pub last_timeout_diag: Option<String>,
    /// Error count per step name (`"commit"`, `"wait_for_lsn"`, …). A BTreeMap so the
    /// summary lists steps in a stable order.
    pub errors: BTreeMap<&'static str, u64>,
    /// Most recent error message per step, shown in the summary.
    pub last_error: BTreeMap<&'static str, String>,
}

impl TaskStats {
    /// Empty statistics.
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
            boundary_lsns: 0,
            last_timeout_diag: None,
            errors: BTreeMap::new(),
            last_error: BTreeMap::new(),
        }
    }

    /// Count one error for `step` and remember its message.
    pub fn error(&mut self, step: &'static str, msg: String) {
        *self.errors.entry(step).or_insert(0) += 1;
        self.last_error.insert(step, msg);
    }

    /// Total number of errors over all steps.
    pub fn error_total(&self) -> u64 {
        self.errors.values().sum()
    }

    /// Add another task's statistics into this one (counters summed, histograms merged,
    /// last error messages overwritten by `o`'s).
    pub fn merge(&mut self, o: &TaskStats) {
        self.h.merge(&o.h);
        self.completed += o.completed;
        self.wait_success += o.wait_success;
        self.wait_timeout += o.wait_timeout;
        self.wait_not_in_recovery += o.wait_not_in_recovery;
        self.wait_other += o.wait_other;
        self.verify_ok += o.verify_ok;
        self.verify_missing += o.verify_missing;
        self.boundary_lsns += o.boundary_lsns;
        for (k, v) in &o.errors {
            *self.errors.entry(k).or_insert(0) += v;
        }
        for (k, v) in &o.last_error {
            self.last_error.insert(k, v.clone());
        }
    }
}

/// Running sums over pool usage samples, for averages and maximums. Plain integers: the
/// struct is always used behind the `Mutex` of [`PoolUsage`]'s owner.
#[derive(Clone, Copy, Default)]
pub struct UsageAcc {
    /// Number of samples taken.
    pub samples: u64,
    /// Sum of `in_use` over all samples (for the mean).
    sum_in_use: u64,
    /// Highest `in_use` seen.
    pub max_in_use: usize,
    /// Sum of `waiting` over all samples (for the mean).
    sum_waiting: u64,
    /// Highest `waiting` seen.
    pub max_waiting: usize,
}

impl UsageAcc {
    /// Add one sample.
    fn add(&mut self, in_use: usize, waiting: usize) {
        self.samples += 1;
        self.sum_in_use += in_use as u64;
        self.max_in_use = self.max_in_use.max(in_use);
        self.sum_waiting += waiting as u64;
        self.max_waiting = self.max_waiting.max(waiting);
    }

    /// Average number of connections in use (0.0 before the first sample).
    pub fn mean_in_use(&self) -> f64 {
        if self.samples == 0 { 0.0 } else { self.sum_in_use as f64 / self.samples as f64 }
    }

    /// Average number of tasks waiting for a connection (0.0 before the first sample).
    pub fn mean_waiting(&self) -> f64 {
        if self.samples == 0 { 0.0 } else { self.sum_waiting as f64 / self.samples as f64 }
    }
}

/// Usage of one connection pool (DESIGN.md D9).
///
/// - `in_use`  = connections checked out by tasks (`size - available` of deadpool's status)
/// - `waiting` = tasks blocked in `pool.get()` because every connection is in use
///
/// Kept three ways: the latest sample (`now_*`), since the previous progress line
/// (`interval`, reset by the reporter), and for the whole run (`run`, used by the summary
/// and CSV).
pub struct PoolUsage {
    /// Configured maximum pool size (`--primary-pool` / `--standby-pool`).
    pub max_size: usize,
    /// Connections in use at the latest sample.
    pub now_in_use: usize,
    /// Tasks waiting at the latest sample.
    pub now_waiting: usize,
    /// Samples since the last progress line. The reporter resets this after printing.
    pub interval: UsageAcc,
    /// Samples over the whole run.
    pub run: UsageAcc,
}

impl PoolUsage {
    /// No samples yet for a pool of `max_size` connections.
    pub fn new(max_size: usize) -> Self {
        PoolUsage { max_size, now_in_use: 0, now_waiting: 0, interval: UsageAcc::default(), run: UsageAcc::default() }
    }

    /// Record one sample of deadpool's `Status` (`size`, `available`, `waiting`).
    pub fn sample(&mut self, size: usize, available: usize, waiting: usize) {
        // saturating_sub: never underflow if the fields are read at slightly different moments
        let in_use = size.saturating_sub(available);
        self.now_in_use = in_use;
        self.now_waiting = waiting;
        self.interval.add(in_use, waiting);
        self.run.add(in_use, waiting);
    }

    /// Average utilisation over the run, as a percentage of `max_size`.
    pub fn run_util_pct(&self) -> f64 {
        if self.max_size == 0 { 0.0 } else { 100.0 * self.run.mean_in_use() / self.max_size as f64 }
    }
}

/// Counters shared by all tasks while the run is in progress, read by the progress reporter.
/// Relaxed ordering is enough: they are statistics, not synchronisation.
pub struct Live {
    /// Completed transactions so far.
    pub completed: AtomicU64,
    /// Attempts that did not complete: errors plus non-`success` `WAIT FOR` statuses.
    pub failed: AtomicU64,
}

impl Live {
    /// Both counters at zero.
    pub fn new() -> Self {
        Live { completed: AtomicU64::new(0), failed: AtomicU64::new(0) }
    }

    /// Count one completed transaction.
    pub fn inc_completed(&self) {
        self.completed.fetch_add(1, Ordering::Relaxed);
    }

    /// Count one attempt that did not complete.
    pub fn inc_failed(&self) {
        self.failed.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_usage_in_use_and_averages() {
        let mut u = PoolUsage::new(8);
        u.sample(8, 2, 0); // 6 in use
        u.sample(8, 0, 4); // 8 in use, 4 waiting
        u.sample(4, 4, 0); // 0 in use (pool shrank, all idle)
        assert_eq!(u.now_in_use, 0);
        assert_eq!(u.run.max_in_use, 8);
        assert_eq!(u.run.max_waiting, 4);
        assert!((u.run.mean_in_use() - 14.0 / 3.0).abs() < 1e-9);
        assert!((u.run_util_pct() - 100.0 * (14.0 / 3.0) / 8.0).abs() < 1e-9);
        // available > size must not underflow
        u.sample(1, 2, 0);
        assert_eq!(u.now_in_use, 0);
    }

    #[test]
    fn task_stats_merge_sums_counters_and_histograms() {
        let mut a = TaskStats::new();
        let mut b = TaskStats::new();
        a.completed = 3;
        b.completed = 4;
        a.error("commit", "x".into());
        b.error("commit", "y".into());
        b.error("verify", "z".into());
        rec(&mut a.h.total, Duration::from_millis(1));
        rec(&mut b.h.total, Duration::from_millis(3));
        a.merge(&b);
        assert_eq!(a.completed, 7);
        assert_eq!(a.errors["commit"], 2);
        assert_eq!(a.error_total(), 3);
        assert_eq!(a.last_error["commit"], "y");
        assert_eq!(a.h.total.len(), 2);
        assert!((q_ms(&a.h.total, 1.0) - 3.0).abs() < 0.01);
    }

    #[test]
    fn q_ms_of_empty_histogram_is_zero() {
        assert_eq!(q_ms(&new_hist(), 0.99), 0.0);
    }
}
