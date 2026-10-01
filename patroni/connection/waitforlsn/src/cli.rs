//! Command-line options.
//!
//! Every setting of a test run is an option here, so one binary covers the whole test matrix
//! (`scripts/run_matrix.sh` just calls it with different options).
//!
//! - [`Args`] is parsed by clap. The `///` comment on each field is also its `--help` text, so the
//!   first line of each stays short. User-facing explanations of every option are in
//!   `PARAMETERS.md`.
//! - Settings that depend on several options (e.g. "is the standby needed?") are methods on
//!   [`Args`], so `main.rs` and `workload.rs` never repeat that logic.
use clap::{Parser, ValueEnum};
use std::path::PathBuf;
use std::time::Duration;

/// Which test to run (DESIGN.md §1).
///
/// The mode decides the primary's `synchronous_commit` default and whether the
/// application waits on the standby before counting a transaction as complete.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Mode {
    /// T1: synchronous_commit=on on the primary; txn is complete when COMMIT returns.
    Sync,
    /// T2: synchronous_commit=off on the primary; txn is complete when WAIT FOR LSN succeeds on the standby.
    Waitfor,
}

impl Mode {
    /// Lower-case name as used on the command line and in the CSV `mode` column.
    pub fn name(&self) -> &'static str {
        match self {
            Mode::Sync => "sync",
            Mode::Waitfor => "waitfor",
        }
    }
    /// synchronous_commit used for the primary sessions unless --synchronous-commit overrides it.
    pub fn default_sync_commit(&self) -> &'static str {
        match self {
            Mode::Sync => "on",
            Mode::Waitfor => "off",
        }
    }
}

/// The `MODE` option of `WAIT FOR LSN`: which point on the standby to wait for.
///
/// `primary_flush` isn't offered: it runs on the primary, and this program always waits on
/// the standby.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
#[allow(clippy::enum_variant_names)] // names mirror PostgreSQL's mode names on purpose
pub enum WaitMode {
    /// Wait until the WAL is replayed: the change is visible to queries on the standby
    #[value(name = "standby_replay")]
    StandbyReplay,
    /// Wait until the WAL is fsynced on the standby: durable there, maybe not visible yet
    #[value(name = "standby_flush")]
    StandbyFlush,
    /// Wait until the WAL is written (not fsynced) on the standby
    #[value(name = "standby_write")]
    StandbyWrite,
}

impl WaitMode {
    /// The mode name exactly as `WAIT FOR ... WITH (MODE '<name>')` expects it.
    pub fn as_sql(&self) -> &'static str {
        match self {
            WaitMode::StandbyReplay => "standby_replay",
            WaitMode::StandbyFlush => "standby_flush",
            WaitMode::StandbyWrite => "standby_write",
        }
    }
}

/// clap value parser for durations: accepts human-friendly values such as `500ms`, `30s`,
/// `2m`, `1m30s`. A unit is required, so a bare `90` is rejected rather than guessed.
fn parse_duration(s: &str) -> Result<Duration, String> {
    humantime::parse_duration(s).map_err(|e| e.to_string())
}

/// All command-line options of a run. Parsed once in `main()`, then read-only.
#[derive(Parser, Debug)]
#[command(version,
          about = "Evaluate PostgreSQL 19 WAIT FOR LSN: sync commit vs async commit + WAIT FOR on a standby",
          after_help = "Durations need a unit: 500ms, 30s, 2m, 1m30s.\nFull parameter reference: PARAMETERS.md")]
pub struct Args {
    /// Test to run: sync (T1) or waitfor (T2)
    #[arg(long, value_enum, default_value_t = Mode::Waitfor)]
    pub mode: Mode,

    /// libpq-style connection string for the primary
    #[arg(long, env = "PRIMARY_DSN",
          default_value = "host=localhost port=5433 user=postgres password=postgres dbname=postgres")]
    pub primary: String,

    /// libpq-style connection string for the standby
    #[arg(long, env = "STANDBY_DSN",
          default_value = "host=localhost port=5434 user=postgres password=postgres dbname=postgres")]
    pub standby: String,

    /// Number of concurrent worker tasks (simulated application clients)
    #[arg(long, default_value_t = 32)]
    pub tasks: usize,

    /// Max connections in the primary pool
    #[arg(long, default_value_t = 16)]
    pub primary_pool: usize,

    /// Max connections in the standby pool
    #[arg(long, default_value_t = 32)]
    pub standby_pool: usize,

    /// How long to run (default 30s unless --txns is given)
    #[arg(long, value_parser = parse_duration)]
    pub duration: Option<Duration>,

    /// Stop after this many transactions in total (across all tasks)
    #[arg(long)]
    pub txns: Option<u64>,

    /// WAIT FOR ... MODE used in waitfor mode
    #[arg(long, value_enum, default_value_t = WaitMode::StandbyReplay)]
    pub wait_mode: WaitMode,

    /// WAIT FOR ... TIMEOUT (must be > 0: PostgreSQL treats 0 as "wait forever")
    #[arg(long, value_parser = parse_duration, default_value = "5s")]
    pub wait_timeout: Duration,

    /// Override synchronous_commit for primary sessions (e.g. remote_apply for an extra baseline)
    #[arg(long)]
    pub synchronous_commit: Option<String>,

    /// After the txn is complete, SELECT the row on the standby (read-your-writes check).
    /// In sync mode this measures how often a read on the standby would miss the write.
    #[arg(long)]
    pub verify: bool,

    /// In sync mode, also fetch the LSN after commit (same round trips as waitfor, for a fair comparison)
    #[arg(long)]
    pub fetch_lsn: bool,

    /// Wait for pg_current_wal_insert_lsn() as-is, without the page-boundary fix (DESIGN.md D2).
    /// Only for demonstrating the upstream problem: some waits then stall until their timeout.
    /// REPRO: see repro/REPRODUCE_WITH_WAITFORLSN.md
    #[arg(long)]
    pub raw_insert_lsn: bool,

    /// Size of the text payload inserted per transaction (bytes)
    #[arg(long, default_value_t = 100)]
    pub payload_size: usize,

    /// TRUNCATE the test table before the run
    #[arg(long)]
    pub truncate: bool,

    /// Progress report interval (0s disables)
    #[arg(long, value_parser = parse_duration, default_value = "5s")]
    pub report_interval: Duration,

    /// How often to sample pool usage (connections in use / tasks waiting)
    #[arg(long, value_parser = parse_duration, default_value = "10ms")]
    pub pool_sample_interval: Duration,

    /// Append a one-line summary to this CSV file (header written if the file is new)
    #[arg(long)]
    pub output: Option<PathBuf>,

    /// Free-text label stored in the CSV (e.g. "ec2 m6i.xlarge, wwd=10ms")
    #[arg(long, default_value = "")]
    pub label: String,

    /// Number of tokio worker threads (default: number of CPUs)
    #[arg(long)]
    pub threads: Option<usize>,
}

impl Args {
    /// Effective `synchronous_commit` for primary sessions: `--synchronous-commit` if given,
    /// otherwise the mode's default (`on` for sync, `off` for waitfor). Applied per session
    /// through the startup options (DESIGN.md D4).
    pub fn sync_commit(&self) -> &str {
        self.synchronous_commit
            .as_deref()
            .unwrap_or(self.mode.default_sync_commit())
    }

    /// Does this run need the standby at all? Waitfor mode always does, and sync mode only
    /// for `--verify`. When false, no standby pool is created and `--standby` is never used.
    pub fn uses_standby(&self) -> bool {
        self.mode == Mode::Waitfor || self.verify
    }

    /// Do we fetch the commit LSN after each commit? Always in waitfor mode (it's the
    /// `WAIT FOR` target, DESIGN.md D2). In sync mode only with `--fetch-lsn`, to match
    /// T2's round trips.
    pub fn needs_lsn(&self) -> bool {
        self.mode == Mode::Waitfor || self.fetch_lsn
    }

    /// Effective run duration: `--duration` if given; otherwise no time limit when `--txns`
    /// is given, and 30s when neither is. `None` means "no deadline".
    pub fn effective_duration(&self) -> Option<Duration> {
        match (self.duration, self.txns) {
            (Some(d), _) => Some(d),
            (None, Some(_)) => None,
            (None, None) => Some(Duration::from_secs(30)),
        }
    }
}
