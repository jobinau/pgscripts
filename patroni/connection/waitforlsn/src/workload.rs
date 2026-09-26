//! The per-transaction workload: what one simulated application client does.
//!
//! [`run_task`] is the body of each worker task (DESIGN.md §3.2). It loops until
//! [`Ctx::keep_going`] says stop, and each iteration is one transaction:
//!
//! 1. [`primary_step`]: take a primary connection, autocommit `INSERT ... RETURNING id`,
//!    optionally fetch the commit LSN ([`LSN_SQL`], normalised by
//!    [`normalize_insert_lsn`]), give the connection back (D1, D2, D5).
//! 2. [`standby_step`], only if a standby pool exists: take a standby connection,
//!    `WAIT FOR LSN` in waitfor mode ([`wait_for_lsn`], D3), optionally the read-your-writes
//!    check ([`VERIFY_SQL`]).
//! 3. Decide whether the transaction is *completed* (D6) and record the statistics.
//!
//! Errors are returned as `(step name, message)` pairs, so [`run_task`] can count them per
//! step and continue (D11).
use crate::cli::Mode;
use crate::metrics::{rec, Live, TaskStats};
use deadpool_postgres::Pool;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio_postgres::SimpleQueryMessage;

/// The transaction: one autocommit INSERT. `RETURNING id` gives the key for `--verify` (D5).
pub const INSERT_SQL: &str = "INSERT INTO wfl_test (task_id, payload) VALUES ($1, $2) RETURNING id";

/// The commit LSN query (D2). Run it **after** the INSERT has returned, on the **same**
/// connection: the insert position is then at or past the end of our commit record. Don't
/// use `pg_current_wal_flush_lsn()`. With `synchronous_commit=off` it can still be behind
/// our commit record, and `WAIT FOR` would return too early.
pub const LSN_SQL: &str = "SELECT pg_current_wal_insert_lsn()::text";

/// Read-your-writes check on the standby (`--verify`).
pub const VERIFY_SQL: &str = "SELECT 1 FROM wfl_test WHERE id = $1";

/// Everything the worker tasks share, built once in `main.rs` and passed as `Arc<Ctx>`
/// (DESIGN.md §3.4). Read-only apart from the atomics, so the hot path takes no locks.
pub struct Ctx {
    /// T1 (`sync`) or T2 (`waitfor`).
    pub mode: Mode,
    /// Primary pool. Its sessions start with the run's `synchronous_commit` (D4).
    pub primary: Pool,
    /// Standby pool: `None` unless waitfor mode or `--verify`.
    pub standby: Option<Pool>,
    /// Fetch the commit LSN after each INSERT (waitfor, or sync with `--fetch-lsn`).
    pub needs_lsn: bool,
    /// Run the read-your-writes check (`--verify`).
    pub verify: bool,
    /// `WAIT FOR` MODE, already in SQL spelling (`standby_flush`, …).
    pub wait_mode_sql: &'static str,
    /// `WAIT FOR` TIMEOUT in milliseconds. Must be > 0 (0 means "wait forever").
    pub wait_timeout_ms: u128,
    /// `--raw-insert-lsn`: skip [`normalize_insert_lsn`] (to demonstrate the stall).
    pub raw_insert_lsn: bool,
    /// Primary's WAL page size in bytes (normally 8192), for [`normalize_insert_lsn`].
    pub wal_block_size: u64,
    /// Primary's WAL segment size in bytes (normally 16MB), for [`normalize_insert_lsn`].
    pub wal_segment_size: u64,
    /// Text inserted in every row (`--payload-size` bytes).
    pub payload: String,
    /// No new transactions start after this instant (`--duration`), if set.
    pub deadline: Option<Instant>,
    /// Stop after this many transactions started in total (`--txns`), if set.
    pub max_txns: Option<u64>,
    /// Transactions started so far over all tasks. Only counted when `max_txns` is set.
    pub issued: AtomicU64,
    /// Set by the Ctrl-C handler: finish the current transaction, then stop.
    pub stop: AtomicBool,
    /// Live counters for the progress lines.
    pub live: Live,
    /// How many error messages were printed so far (only the first 10 are).
    pub errors_printed: AtomicU64,
}

/// Format a tokio-postgres error readably. Its `Display` only says "db error", so show
/// severity, SQLSTATE and the server's message when the error came from the server.
pub fn pg_err(e: &tokio_postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => format!("{} {}: {}", db.severity(), db.code().code(), db.message()),
        None => e.to_string(),
    }
}

/// Same as [`pg_err`] for pool errors (a failed connection attempt comes wrapped in
/// `PoolError::Backend`). Timeouts and other pool errors use their own message.
pub fn pool_err(e: &deadpool_postgres::PoolError) -> String {
    match e {
        deadpool_postgres::PoolError::Backend(pe) => pg_err(pe),
        other => other.to_string(),
    }
}

/// Result of a `WAIT FOR LSN ... NO_THROW` call. With `NO_THROW` the expected outcomes
/// come back as a status row, not as errors (D3).
#[derive(Debug, PartialEq)]
pub enum WaitStatus {
    /// The standby reached the LSN in the requested mode: the transaction is complete.
    Success,
    /// TIMEOUT expired first. The transaction is committed on the primary but not confirmed.
    Timeout,
    /// The server is not a standby (any more), e.g. after promotion. Returned immediately.
    NotInRecovery,
    /// A status string this program doesn't know (e.g. added by a later PostgreSQL version).
    Other(String),
}

impl WaitStatus {
    /// Map the server's `status` column text to a [`WaitStatus`].
    fn parse(s: &str) -> Self {
        match s {
            "success" => WaitStatus::Success,
            "timeout" => WaitStatus::Timeout,
            "not in recovery" => WaitStatus::NotInRecovery,
            other => WaitStatus::Other(other.to_string()),
        }
    }
}

/// Check that `s` looks like an LSN: `X/Y`, 1–8 hex digits on each side.
///
/// `WAIT FOR` takes the LSN as a literal inside the SQL text (it can't be a bind
/// parameter), so this check runs before building the statement, and nothing but an LSN can
/// be injected (D3).
pub fn is_valid_lsn(s: &str) -> bool {
    match s.split_once('/') {
        Some((hi, lo)) => {
            !hi.is_empty() && !lo.is_empty() && hi.len() <= 8 && lo.len() <= 8
                && hi.chars().all(|c| c.is_ascii_hexdigit())
                && lo.chars().all(|c| c.is_ascii_hexdigit())
        }
        None => false,
    }
}

/// Parse an LSN string (`X/Y`, hex) into its 64-bit byte position.
pub fn parse_lsn(s: &str) -> Option<u64> {
    if !is_valid_lsn(s) {
        return None;
    }
    let (hi, lo) = s.split_once('/')?;
    Some((u64::from_str_radix(hi, 16).ok()? << 32) | u64::from_str_radix(lo, 16).ok()?)
}

/// Format a 64-bit WAL position the way PostgreSQL 19 prints it (`X/XXXXXXXX`).
pub fn format_lsn(pos: u64) -> String {
    format!("{:X}/{:08X}", pos >> 32, pos & 0xFFFF_FFFF)
}

/// Size of the short WAL page header (every page except the first of a segment).
const SHORT_PAGE_HEADER: u64 = 24;
/// Size of the long WAL page header (first page of every segment).
const LONG_PAGE_HEADER: u64 = 40;

/// Turn the insert position into a target that the standby is guaranteed to reach
/// without any further WAL being written (DESIGN.md D2, "page boundary" case).
///
/// When the last inserted record ends exactly at a WAL page boundary,
/// `pg_current_wal_insert_lsn()` returns the position *after the next page's header*
/// (boundary + 24, or + 40 at a segment start). No record ends there. The standby's
/// write/flush/replay positions stop at the boundary and only move past the header when
/// the *next* record arrives. If every client is waiting at that moment, no new WAL is
/// written and `WAIT FOR` sleeps until its timeout. (Observed: about 1 in 2000 waits with
/// 4 tasks, all 4 timing out on the same LSN at page offset 24.) Such an LSN is moved back
/// to the boundary, which is where the previous record really ended. Any other LSN is
/// returned unchanged.
pub fn normalize_insert_lsn(pos: u64, wal_block_size: u64, wal_segment_size: u64) -> u64 {
    if wal_block_size == 0 || wal_segment_size == 0 {
        return pos;
    }
    let first_page_of_segment = pos % wal_segment_size < wal_block_size;
    let header = if first_page_of_segment { LONG_PAGE_HEADER } else { SHORT_PAGE_HEADER };
    if pos % wal_block_size == header { pos - header } else { pos }
}

/// Build the `WAIT FOR` statement text. `NO_THROW` makes timeout / not-in-recovery come back
/// as a status row. The caller must have validated `lsn` ([`wait_for_lsn`] does).
pub fn wait_sql(lsn: &str, mode: &str, timeout_ms: u128) -> String {
    format!("WAIT FOR LSN '{lsn}' WITH (MODE '{mode}', TIMEOUT '{timeout_ms}ms', NO_THROW)")
}

/// Run `WAIT FOR LSN` on a standby connection and return its status.
///
/// Sent with `simple_query` because the LSN is part of the SQL text, so there is nothing
/// to prepare (D3). It must be the first statement of its transaction. On an autocommit
/// pooled connection it always is. Errors are returned formatted by [`pg_err`].
pub async fn wait_for_lsn(
    client: &tokio_postgres::Client,
    lsn: &str,
    mode: &str,
    timeout_ms: u128,
) -> Result<WaitStatus, String> {
    if !is_valid_lsn(lsn) {
        return Err(format!("refusing invalid LSN {lsn:?}"));
    }
    // simple_query: the LSN is part of the SQL text, so there's nothing to prepare
    let msgs = client
        .simple_query(&wait_sql(lsn, mode, timeout_ms))
        .await
        .map_err(|e| pg_err(&e))?;
    for m in msgs {
        if let SimpleQueryMessage::Row(row) = m {
            return Ok(WaitStatus::parse(row.get(0).unwrap_or("")));
        }
    }
    Err("WAIT FOR returned no row".to_string())
}

impl Ctx {
    /// Should the worker start another transaction? No after Ctrl-C, after the deadline,
    /// or once `max_txns` transactions have been started. Counting `issued` here means
    /// failed attempts count towards `--txns` too.
    fn keep_going(&self) -> bool {
        if self.stop.load(Ordering::Relaxed) {
            return false;
        }
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            return false;
        }
        // Only counted when --txns is set; failed attempts count too
        if self.max_txns.is_some_and(|max| self.issued.fetch_add(1, Ordering::Relaxed) >= max) {
            return false;
        }
        true
    }

    /// Print the first 10 errors of the run to stderr, so a broken setup is obvious right
    /// away. Later errors are only counted (in `TaskStats`), so an outage doesn't flood the terminal.
    fn log_error(&self, task_id: i32, step: &str, msg: &str) {
        if self.errors_printed.fetch_add(1, Ordering::Relaxed) < 10 {
            eprintln!("[task {task_id}] {step} error: {msg}");
        }
    }
}

/// Step 1, primary side: INSERT (= commit) and optionally the commit LSN.
///
/// Returns `(row id, Some(normalised LSN))`, or `(row id, None)` when the LSN isn't needed.
/// The pooled connection is dropped (returned to the pool) at the end of this function,
/// **before** the standby step starts, so replication lag never holds primary connections (D1).
/// Timed: `primary_acquire`, `commit`, `lsn_fetch`.
async fn primary_step(
    ctx: &Ctx,
    task_id: i32,
    st: &mut TaskStats,
) -> Result<(i64, Option<String>), (&'static str, String)> {
    let t = Instant::now();
    let client = ctx.primary.get().await.map_err(|e| ("primary_acquire", pool_err(&e)))?;
    rec(&mut st.h.primary_acquire, t.elapsed());

    // Autocommit INSERT: when this returns the txn is committed (flushed locally in T1,
    // possibly not yet flushed in T2 because synchronous_commit=off).
    let t = Instant::now();
    let stmt = client.prepare_cached(INSERT_SQL).await.map_err(|e| ("commit", pg_err(&e)))?;
    let row = client
        .query_one(&stmt, &[&task_id, &ctx.payload])
        .await
        .map_err(|e| ("commit", pg_err(&e)))?;
    let id: i64 = row.get(0);
    rec(&mut st.h.commit, t.elapsed());

    let lsn = if ctx.needs_lsn {
        let t = Instant::now();
        let stmt = client.prepare_cached(LSN_SQL).await.map_err(|e| ("lsn_fetch", pg_err(&e)))?;
        let row = client.query_one(&stmt, &[]).await.map_err(|e| ("lsn_fetch", pg_err(&e)))?;
        rec(&mut st.h.lsn_fetch, t.elapsed());
        let raw: String = row.get(0);
        // An LSN just past a page header could stall WAIT FOR: move it to the page boundary (D2)
        let pos = parse_lsn(&raw).ok_or(("lsn_fetch", format!("unparsable LSN {raw:?}")))?;
        if ctx.raw_insert_lsn {
            Some(raw)
        } else {
            Some(format_lsn(normalize_insert_lsn(pos, ctx.wal_block_size, ctx.wal_segment_size)))
        }
    } else {
        None
    };
    Ok((id, lsn))
    // <- `client` dropped here: connection back to the primary pool
}

/// Step 2, standby side: `WAIT FOR LSN` (waitfor mode) and/or the read-your-writes check.
///
/// Returns the `WAIT FOR` status (`None` in sync mode) and whether the verify found the row
/// (`None` if it didn't run). The verify only runs when the transaction counts as complete:
/// always in sync mode, only after `Success` in waitfor mode. It always runs *after* the
/// wait, never before (a snapshot held before `WAIT FOR` could block replay, D3).
/// Timed: `standby_acquire`, `wait`, `verify`.
async fn standby_step(
    ctx: &Ctx,
    standby: &Pool,
    id: i64,
    lsn: Option<&str>,
    st: &mut TaskStats,
) -> Result<(Option<WaitStatus>, Option<bool>), (&'static str, String)> {
    let t = Instant::now();
    let client = standby.get().await.map_err(|e| ("standby_acquire", pool_err(&e)))?;
    rec(&mut st.h.standby_acquire, t.elapsed());

    let status = if ctx.mode == Mode::Waitfor {
        let lsn = lsn.expect("waitfor mode always fetches the LSN");
        let t = Instant::now();
        let s = wait_for_lsn(&client, lsn, ctx.wait_mode_sql, ctx.wait_timeout_ms)
            .await
            .map_err(|e| ("wait_for_lsn", e))?;
        rec(&mut st.h.wait, t.elapsed());
        Some(s)
    } else {
        None
    };

    // Only verify when the txn counts as complete: always in sync mode, success in waitfor mode.
    let should_verify = ctx.verify && matches!(status, None | Some(WaitStatus::Success));
    let found = if should_verify {
        let t = Instant::now();
        let stmt = client.prepare_cached(VERIFY_SQL).await.map_err(|e| ("verify", pg_err(&e)))?;
        let rows = client.query(&stmt, &[&id]).await.map_err(|e| ("verify", pg_err(&e)))?;
        rec(&mut st.h.verify, t.elapsed());
        Some(!rows.is_empty())
    } else {
        None
    };
    Ok((status, found))
}

/// One worker task = one simulated application client (D7). Runs transactions back to
/// back until [`Ctx::keep_going`] returns false, then returns everything it measured.
///
/// Completion rule (D6): the primary step succeeded, and, if the standby step ran, it
/// succeeded too and (in waitfor mode) returned `Success`. Only completed transactions are
/// timed in `total`. After an error the task sleeps 100ms before the next attempt, so a
/// dead server doesn't turn into a busy loop (D11).
pub async fn run_task(ctx: std::sync::Arc<Ctx>, task_id: i32) -> TaskStats {
    let mut st = TaskStats::new();

    while ctx.keep_going() {
        let t0 = Instant::now();

        // ---- primary ----
        let (id, lsn) = match primary_step(&ctx, task_id, &mut st).await {
            Ok(v) => v,
            Err((step, msg)) => {
                ctx.log_error(task_id, step, &msg);
                st.error(step, msg);
                ctx.live.inc_failed();
                // back off a little so a dead server doesn't spin the CPU
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };

        // ---- standby (waitfor mode, or sync mode with --verify) ----
        let mut complete = true;
        if let Some(standby) = &ctx.standby {
            match standby_step(&ctx, standby, id, lsn.as_deref(), &mut st).await {
                Ok((status, found)) => {
                    match status {
                        None => {}
                        Some(WaitStatus::Success) => st.wait_success += 1,
                        Some(WaitStatus::Timeout) => {
                            ctx.log_error(task_id, "wait_for_lsn",
                                          &format!("timeout waiting for LSN {}", lsn.as_deref().unwrap_or("?")));
                            st.wait_timeout += 1;
                            complete = false;
                        }
                        Some(WaitStatus::NotInRecovery) => { st.wait_not_in_recovery += 1; complete = false; }
                        Some(WaitStatus::Other(s)) => {
                            ctx.log_error(task_id, "wait_for_lsn", &format!("unexpected status {s:?}"));
                            st.wait_other += 1;
                            complete = false;
                        }
                    }
                    match found {
                        Some(true) => st.verify_ok += 1,
                        Some(false) => st.verify_missing += 1,
                        None => {}
                    }
                }
                Err((step, msg)) => {
                    ctx.log_error(task_id, step, &msg);
                    st.error(step, msg);
                    complete = false;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }

        if complete {
            rec(&mut st.h.total, t0.elapsed());
            st.completed += 1;
            ctx.live.inc_completed();
        } else {
            ctx.live.inc_failed();
        }
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsn_validation() {
        assert!(is_valid_lsn("0/3002D98"));
        assert!(is_valid_lsn("16/B374D848"));
        assert!(!is_valid_lsn("0/3002D98'; DROP TABLE x; --"));
        assert!(!is_valid_lsn("03002D98"));
        assert!(!is_valid_lsn("/1"));
        assert!(!is_valid_lsn("G/1"));
    }

    #[test]
    fn lsn_parse_and_format_round_trip() {
        assert_eq!(parse_lsn("0/0CAE2018"), Some(0x0CAE2018));
        assert_eq!(parse_lsn("16/B374D848"), Some((0x16 << 32) | 0xB374D848));
        assert_eq!(parse_lsn("0/1'; --"), None);
        assert_eq!(format_lsn(0x0CAE2018), "0/0CAE2018");
        assert_eq!(format_lsn((0x16 << 32) | 0x3), "16/00000003");
    }

    #[test]
    fn insert_lsn_after_page_header_moves_to_boundary() {
        let (blk, seg) = (8192, 16 * 1024 * 1024);
        // the LSN that made WAIT FOR time out: page offset 24 (short header)
        assert_eq!(normalize_insert_lsn(0x0CAE2018, blk, seg), 0x0CAE2000);
        // first page of a segment: long header (40 bytes)
        assert_eq!(normalize_insert_lsn(0x0D000028, blk, seg), 0x0D000000);
        // ordinary positions are unchanged
        assert_eq!(normalize_insert_lsn(0x0CAE2F30, blk, seg), 0x0CAE2F30);
        assert_eq!(normalize_insert_lsn(0x0CAE2000, blk, seg), 0x0CAE2000);
        // offset 24 inside the first page of a segment is a real record end, not a header
        assert_eq!(normalize_insert_lsn(0x0D000018, blk, seg), 0x0D000018);
    }

    #[test]
    fn wait_sql_format() {
        assert_eq!(
            wait_sql("0/1", "standby_flush", 5000),
            "WAIT FOR LSN '0/1' WITH (MODE 'standby_flush', TIMEOUT '5000ms', NO_THROW)"
        );
    }
}
