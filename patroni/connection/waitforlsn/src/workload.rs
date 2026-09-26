// The per-transaction workload ---------------------------------------------------------
// One worker task = one simulated application client running transactions in a loop.
use crate::cli::Mode;
use crate::metrics::{rec, Live, TaskStats};
use deadpool_postgres::Pool;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio_postgres::SimpleQueryMessage;

pub const INSERT_SQL: &str = "INSERT INTO wfl_test (task_id, payload) VALUES ($1, $2) RETURNING id";
// Read AFTER the COMMIT returned, on the same connection: the insert position is at or
// past the end of our commit record. Don't use pg_current_wal_flush_lsn(): with
// synchronous_commit=off it can still be behind our commit record (see CLAUDE.md).
pub const LSN_SQL: &str = "SELECT pg_current_wal_insert_lsn()::text";
pub const VERIFY_SQL: &str = "SELECT 1 FROM wfl_test WHERE id = $1";

/// Everything the worker tasks share (read-only apart from the atomics).
pub struct Ctx {
    pub mode: Mode,
    pub primary: Pool,
    pub standby: Option<Pool>,
    pub needs_lsn: bool,
    pub verify: bool,
    pub wait_mode_sql: &'static str,
    pub wait_timeout_ms: u128,
    pub payload: String,
    pub deadline: Option<Instant>,
    pub max_txns: Option<u64>,
    pub issued: AtomicU64,  // txns started so far (for --txns)
    pub stop: AtomicBool,   // set by Ctrl-C
    pub live: Live,
    pub errors_printed: AtomicU64,
}

/// tokio-postgres' Display only says "db error"; include SQLSTATE + server message instead.
pub fn pg_err(e: &tokio_postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => format!("{} {}: {}", db.severity(), db.code().code(), db.message()),
        None => e.to_string(),
    }
}

/// Same for pool errors (a failed connection attempt is wrapped in PoolError::Backend).
pub fn pool_err(e: &deadpool_postgres::PoolError) -> String {
    match e {
        deadpool_postgres::PoolError::Backend(pe) => pg_err(pe),
        other => other.to_string(),
    }
}

/// Result of a WAIT FOR LSN call (with NO_THROW the server reports a status, not an error).
#[derive(Debug, PartialEq)]
pub enum WaitStatus {
    Success,
    Timeout,
    NotInRecovery,
    Other(String),
}

impl WaitStatus {
    fn parse(s: &str) -> Self {
        match s {
            "success" => WaitStatus::Success,
            "timeout" => WaitStatus::Timeout,
            "not in recovery" => WaitStatus::NotInRecovery,
            other => WaitStatus::Other(other.to_string()),
        }
    }
}

/// Minimal validation of an LSN string ("X/Y", hex) before putting it into SQL.
/// WAIT FOR takes a literal, not a bind parameter, so this stops SQL injection.
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

/// Build the WAIT FOR statement. NO_THROW so timeout / not-in-recovery come back as a status.
pub fn wait_sql(lsn: &str, mode: &str, timeout_ms: u128) -> String {
    format!("WAIT FOR LSN '{lsn}' WITH (MODE '{mode}', TIMEOUT '{timeout_ms}ms', NO_THROW)")
}

/// Run WAIT FOR on a standby connection and return the status column.
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
    /// Should the worker start another transaction?
    fn keep_going(&self) -> bool {
        if self.stop.load(Ordering::Relaxed) {
            return false;
        }
        if let Some(d) = self.deadline {
            if Instant::now() >= d {
                return false;
            }
        }
        if let Some(max) = self.max_txns {
            if self.issued.fetch_add(1, Ordering::Relaxed) >= max {
                return false;
            }
        }
        true
    }

    /// Print the first few errors so a broken setup is obvious; the rest are only counted.
    fn log_error(&self, task_id: i32, step: &str, msg: &str) {
        if self.errors_printed.fetch_add(1, Ordering::Relaxed) < 10 {
            eprintln!("[task {task_id}] {step} error: {msg}");
        }
    }
}

/// Step 1: primary side. Returns (row id, commit LSN if requested).
/// The pooled connection is dropped (returned to the pool) at the end of this function,
/// before the standby step starts, so replication lag doesn't hold primary connections.
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
        Some(row.get::<_, String>(0))
    } else {
        None
    };
    Ok((id, lsn))
    // <- `client` dropped here: connection back to the primary pool
}

/// Step 2: standby side. WAIT FOR LSN (waitfor mode) and/or a read-your-writes check.
/// Returns the WAIT FOR status (None in sync mode) and whether the verify found the row.
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

/// One worker task: loop until the deadline / txn limit / Ctrl-C.
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
                        Some(WaitStatus::Timeout) => { st.wait_timeout += 1; complete = false; }
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
    fn wait_sql_format() {
        assert_eq!(
            wait_sql("0/1", "standby_flush", 5000),
            "WAIT FOR LSN '0/1' WITH (MODE 'standby_flush', TIMEOUT '5000ms', NO_THROW)"
        );
    }
}
