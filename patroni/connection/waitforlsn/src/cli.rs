// Command-line options -----------------------------------------------------------------
// All knobs of a test run live here, so one binary covers the whole test matrix in CLAUDE.md.
use clap::{Parser, ValueEnum};
use std::path::PathBuf;
use std::time::Duration;

/// Which test to run.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Mode {
    /// T1: synchronous_commit=on on the primary; txn is complete when COMMIT returns.
    Sync,
    /// T2: synchronous_commit=off on the primary; txn is complete when WAIT FOR LSN succeeds on the standby.
    Waitfor,
}

impl Mode {
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

/// The MODE option of WAIT FOR (only the standby-side modes make sense for this app).
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum WaitMode {
    #[value(name = "standby_replay")]
    StandbyReplay,
    #[value(name = "standby_flush")]
    StandbyFlush,
    #[value(name = "standby_write")]
    StandbyWrite,
}

impl WaitMode {
    pub fn as_sql(&self) -> &'static str {
        match self {
            WaitMode::StandbyReplay => "standby_replay",
            WaitMode::StandbyFlush => "standby_flush",
            WaitMode::StandbyWrite => "standby_write",
        }
    }
}

// Accept human-friendly durations such as "500ms", "30s", "2m".
fn parse_duration(s: &str) -> Result<Duration, String> {
    humantime::parse_duration(s).map_err(|e| e.to_string())
}

#[derive(Parser, Debug)]
#[command(version, about = "Evaluate PostgreSQL 19 WAIT FOR LSN: sync commit vs async commit + WAIT FOR on a standby")]
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

    /// WAIT FOR ... TIMEOUT
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

    /// Size of the text payload inserted per transaction (bytes)
    #[arg(long, default_value_t = 100)]
    pub payload_size: usize,

    /// TRUNCATE the test table before the run
    #[arg(long)]
    pub truncate: bool,

    /// Progress report interval (0s disables)
    #[arg(long, value_parser = parse_duration, default_value = "5s")]
    pub report_interval: Duration,

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
    pub fn sync_commit(&self) -> &str {
        self.synchronous_commit
            .as_deref()
            .unwrap_or(self.mode.default_sync_commit())
    }

    /// Does this run need the standby at all?
    pub fn uses_standby(&self) -> bool {
        self.mode == Mode::Waitfor || self.verify
    }

    /// Do we fetch the commit LSN after each commit?
    pub fn needs_lsn(&self) -> bool {
        self.mode == Mode::Waitfor || self.fetch_lsn
    }

    /// Effective run duration: explicit, or 30s when neither --duration nor --txns is given.
    pub fn effective_duration(&self) -> Option<Duration> {
        match (self.duration, self.txns) {
            (Some(d), _) => Some(d),
            (None, Some(_)) => None,
            (None, None) => Some(Duration::from_secs(30)),
        }
    }
}
