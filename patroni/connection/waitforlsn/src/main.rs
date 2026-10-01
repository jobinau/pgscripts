// The crate-level documentation (front page of `make doc`) is DESIGN.md: overall flow,
// architecture and design decisions D1–D13. Keeping it in one Markdown file means the same
// text renders on GitHub and in rustdoc.
#![doc = include_str!("../DESIGN.md")]

// main.rs: program lifecycle (DESIGN.md §3.1). Builds the pools, checks the servers, starts
// the worker tasks plus the helper tasks (Ctrl-C handler, pool sampler, progress reporter),
// and writes the final report.

mod cli;
mod metrics;
mod workload;

use clap::Parser;
use cli::{Args, Mode};
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
use metrics::{q_ms, Live, PoolUsage, TaskStats};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_postgres::{Config, NoTls};
use workload::{wait_for_lsn, Ctx, WaitStatus};

/// Error type for setup and reporting code: any error, sendable between threads. The
/// workload itself doesn't use it. It reports errors as `(step, message)` (D11).
type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Entry point: parse the options, build a multi-threaded tokio runtime (worker thread
/// count from `--threads`), and run [`run`] on it. A setup error ends the program with a
/// non-zero exit status.
fn main() -> Result<(), BoxError> {
    let args = Args::parse();

    // Build the tokio runtime by hand so --threads can set the worker thread count.
    let mut rt = tokio::runtime::Builder::new_multi_thread();
    rt.enable_all();
    if let Some(n) = args.threads {
        rt.worker_threads(n);
    }
    rt.build()?.block_on(run(args))
}

/// Parse a connection string and add the program's defaults, unless they are already set:
/// `application_name` (so sessions are recognisable in `pg_stat_activity`) and
/// `connect_timeout=5s`.
///
/// For the primary, `sync_commit` is appended to the libpq startup `options` as
/// `-c synchronous_commit=<value>`, so every pooled session starts with the run's setting
/// before its first query (D4).
fn make_config(dsn: &str, app_name: &str, sync_commit: Option<&str>) -> Result<Config, BoxError> {
    let mut cfg: Config = dsn.parse()?;
    if cfg.get_application_name().is_none() {
        cfg.application_name(app_name);
    }
    if cfg.get_connect_timeout().is_none() {
        cfg.connect_timeout(Duration::from_secs(5));
    }
    if let Some(sc) = sync_commit {
        let opts = match cfg.get_options() {
            Some(o) if !o.is_empty() => format!("{o} -c synchronous_commit={sc}"),
            _ => format!("-c synchronous_commit={sc}"),
        };
        cfg.options(&opts);
    }
    Ok(cfg)
}

/// Build a deadpool pool of at most `size` connections.
///
/// - `RecyclingMethod::Fast`: a returned connection is only checked with `is_closed()`,
///   with no extra round trip per checkout. Broken connections are discarded and replaced
///   on demand (D11).
/// - `wait_timeout` 30s: a task gives up waiting for a free connection after 30s (counted
///   as a `*_acquire` error) instead of hanging forever.
/// - `create_timeout` 10s: limit on opening a new connection.
fn make_pool(cfg: Config, size: usize) -> Result<Pool, BoxError> {
    let mgr = Manager::from_config(
        cfg,
        NoTls,
        // Fast: only checks is_closed(); no extra round trip per checkout
        ManagerConfig { recycling_method: RecyclingMethod::Fast },
    );
    let pool = Pool::builder(mgr)
        .max_size(size)
        .runtime(Runtime::Tokio1)
        .wait_timeout(Some(Duration::from_secs(30)))   // max wait for a free connection
        .create_timeout(Some(Duration::from_secs(10))) // max time to open a new connection
        .build()?;
    Ok(pool)
}

/// Open all `n` connections of a pool before the measurement starts, by checking them all
/// out at once and returning them. Connection setup (backend fork, authentication) then
/// doesn't count as transaction latency (D10).
async fn warm_up(pool: &Pool, n: usize) -> Result<(), BoxError> {
    let mut held = Vec::with_capacity(n);
    for _ in 0..n {
        held.push(pool.get().await?);
    }
    Ok(()) // all dropped -> back in the pool
}

/// `SHOW <setting>` as a string, for the run header and CSV.
async fn show(client: &tokio_postgres::Client, setting: &str) -> Result<String, BoxError> {
    let row = client.query_one(&format!("SHOW {setting}"), &[]).await?;
    Ok(row.get(0))
}

/// Sampled usage of both pools (D9). Written by [`pool_sampler`]; read by
/// [`progress_reporter`] (which also resets the interval values) and by the summary / CSV.
/// One `Mutex` per pool: the only contention is between the sampler and the reporter,
/// never the worker tasks.
struct PoolStats {
    /// Primary pool usage.
    primary: Mutex<PoolUsage>,
    /// Standby pool usage. `None` when the run has no standby pool.
    standby: Option<Mutex<PoolUsage>>,
}

impl PoolStats {
    /// (label, usage) for each pool in use.
    fn each(&self) -> Vec<(&'static str, &Mutex<PoolUsage>)> {
        let mut v = vec![("primary", &self.primary)];
        if let Some(s) = &self.standby {
            v.push(("standby", s));
        }
        v
    }
}

/// Facts about the primary, read once at startup, for the run header, the CSV and the LSN
/// normalisation.
struct ServerInfo {
    /// `server_version`, e.g. `19beta4 (Debian 19~beta4-1.pgdg13+1)`.
    version: String,
    /// Effective `synchronous_commit` of a pooled session, read back to confirm D4 worked.
    sync_commit: String,
    /// `wal_writer_delay`: the main driver of T2 latency (D2).
    wal_writer_delay: String,
    /// `wal_writer_flush_after`: `0` wakes the WAL writer on every async commit (D2).
    wal_writer_flush_after: String,
    /// `synchronous_standby_names` (empty = no synchronous replication).
    sync_standby_names: String,
    /// WAL page size in bytes, for `workload::normalize_insert_lsn`.
    wal_block_size: u64,
    /// WAL segment size in bytes, for `workload::normalize_insert_lsn`.
    wal_segment_size: u64,
}

/// The whole run, in the order of DESIGN.md §3.1: pools → startup checks and schema →
/// warm-up → header → workers and helpers → summary and CSV. Returns an error only for
/// setup/reporting problems. Errors during the workload are counted, not returned.
async fn run(args: Args) -> Result<(), BoxError> {
    //------------------------- 1. Pools -------------------------------------------------------
    let primary = make_pool(
        make_config(&args.primary, "wfl_primary", Some(args.sync_commit()))?,
        args.primary_pool,
    )?;
    let standby = if args.uses_standby() {
        Some(make_pool(make_config(&args.standby, "wfl_standby", None)?, args.standby_pool)?)
    } else {
        None
    };

    //------------------------- 2. Sanity checks + schema --------------------------------------
    let info = {
        let c = primary.get().await?;
        let in_recovery: bool = c.query_one("SELECT pg_is_in_recovery()", &[]).await?.get(0);
        if in_recovery {
            return Err("--primary points to a server in recovery (a standby)".into());
        }
        c.batch_execute(
            "CREATE TABLE IF NOT EXISTS wfl_test (
                 id         BIGSERIAL PRIMARY KEY,
                 task_id    INT NOT NULL,
                 payload    TEXT NOT NULL,
                 created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp())",
        )
        .await?;
        if args.truncate {
            c.batch_execute("TRUNCATE wfl_test").await?;
        }
        ServerInfo {
            version: show(&c, "server_version").await?,
            sync_commit: show(&c, "synchronous_commit").await?,
            wal_writer_delay: show(&c, "wal_writer_delay").await?,
            wal_writer_flush_after: show(&c, "wal_writer_flush_after").await?,
            sync_standby_names: show(&c, "synchronous_standby_names").await?,
            wal_block_size: c.query_one("SELECT current_setting('wal_block_size')::bigint", &[])
                .await?.get::<_, i64>(0) as u64,
            wal_segment_size: c.query_one("SELECT pg_size_bytes(current_setting('wal_segment_size'))", &[])
                .await?.get::<_, i64>(0) as u64,
        }
    };

    if let Some(sp) = &standby {
        // Make sure the schema change has reached the standby, and that WAIT FOR works at all.
        // The target is always page-boundary corrected here, even with --raw-insert-lsn: after
        // CREATE TABLE nothing else may write WAL, so a raw page-header LSN could make this
        // startup check wait its full 30s (DESIGN.md D2).
        // REPRO: --raw-insert-lsn deliberately affects only the measured run, not this check.
        let raw: String = primary.get().await?.query_one(workload::LSN_SQL, &[]).await?.get(0);
        let pos = workload::parse_lsn(&raw).ok_or(format!("unparsable LSN {raw:?}"))?;
        let lsn = workload::format_lsn(workload::normalize_insert_lsn(
            pos, info.wal_block_size, info.wal_segment_size));
        let c = sp.get().await?;
        let in_recovery: bool = c.query_one("SELECT pg_is_in_recovery()", &[]).await?.get(0);
        if !in_recovery {
            return Err("--standby points to a server that is not in recovery".into());
        }
        match wait_for_lsn(&c, &lsn, "standby_replay", 30_000).await? {
            WaitStatus::Success => {}
            s => return Err(format!("startup WAIT FOR LSN '{lsn}' returned {s:?}").into()),
        }
    }

    warm_up(&primary, args.primary_pool).await?;
    if let Some(sp) = &standby {
        warm_up(sp, args.standby_pool).await?;
    }

    //------------------------- 3. Print the run configuration --------------------------------
    let duration = args.effective_duration();
    println!("== WAIT FOR LSN evaluation ==");
    println!("server              : PostgreSQL {}", info.version);
    println!("mode                : {}", args.mode.name());
    println!("synchronous_commit  : {} (session on primary)", info.sync_commit);
    println!("sync_standby_names  : '{}'", info.sync_standby_names);
    println!("wal_writer_delay    : {}", info.wal_writer_delay);
    println!("wal_writer_flush_af : {}", info.wal_writer_flush_after);
    if args.mode == Mode::Waitfor {
        println!("wait mode / timeout : {} / {:?}", args.wait_mode.as_sql(), args.wait_timeout);
        if args.raw_insert_lsn {
            println!("LSN page-boundary fix: DISABLED (--raw-insert-lsn)");
        }
    }
    println!("tasks               : {}", args.tasks);
    println!("pools (pri/stby)    : {} / {}", args.primary_pool,
             if standby.is_some() { args.standby_pool.to_string() } else { "-".into() });
    println!("verify / fetch_lsn  : {} / {}", args.verify, args.needs_lsn());
    println!("duration / txns     : {:?} / {:?}", duration, args.txns);
    println!();

    //------------------------- 4. Run the workload -------------------------------------------
    let ctx = Arc::new(Ctx {
        mode: args.mode,
        primary: primary.clone(),
        standby,
        needs_lsn: args.needs_lsn(),
        verify: args.verify,
        wait_mode_sql: args.wait_mode.as_sql(),
        wait_timeout_ms: args.wait_timeout.as_millis(),
        raw_insert_lsn: args.raw_insert_lsn,
        wal_block_size: info.wal_block_size,
        wal_segment_size: info.wal_segment_size,
        payload: "x".repeat(args.payload_size),
        deadline: duration.map(|d| Instant::now() + d),
        max_txns: args.txns,
        issued: AtomicU64::new(0),
        stop: AtomicBool::new(false),
        live: Live::new(),
        errors_printed: AtomicU64::new(0),
    });

    // Ctrl-C: stop starting new txns, let in-flight ones finish, then report as usual.
    {
        let ctx = Arc::clone(&ctx);
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                eprintln!("\nCtrl-C: stopping...");
                ctx.stop.store(true, Ordering::Relaxed);
            }
        });
    }

    // Pool usage sampler: polls deadpool's Pool::status() every --pool-sample-interval.
    let pool_stats = Arc::new(PoolStats {
        primary: Mutex::new(PoolUsage::new(args.primary_pool)),
        standby: ctx.standby.as_ref().map(|_| Mutex::new(PoolUsage::new(args.standby_pool))),
    });
    let sampler = tokio::spawn(pool_sampler(
        ctx.primary.clone(),
        ctx.standby.clone(),
        Arc::clone(&pool_stats),
        args.pool_sample_interval,
    ));

    let start = Instant::now();
    let reporter = if args.report_interval > Duration::ZERO {
        Some(tokio::spawn(progress_reporter(
            Arc::clone(&ctx),
            Arc::clone(&pool_stats),
            args.primary.clone(),
            args.report_interval,
            start,
        )))
    } else {
        None
    };

    let mut handles = Vec::with_capacity(args.tasks);
    for task_id in 0..args.tasks {
        handles.push(tokio::spawn(workload::run_task(Arc::clone(&ctx), task_id as i32)));
    }
    let mut total = TaskStats::new();
    for h in handles {
        total.merge(&h.await?);
    }
    let elapsed = start.elapsed();
    if let Some(r) = reporter {
        r.abort();
    }
    sampler.abort();

    //------------------------- 5. Report ------------------------------------------------------
    print_summary(&args, &total, &pool_stats, elapsed);
    if let Some(path) = &args.output {
        write_csv(path, &args, &info, &total, &pool_stats, elapsed)?;
        println!("\nsummary appended to {}", path.display());
    }
    Ok(())
}

/// Helper task: sample both pools' `status()` every `every` into [`PoolStats`] (D9).
/// `size - available` = connections checked out right now. `waiting` = tasks blocked in
/// `pool.get()` because the pool is exhausted. Missed ticks are skipped rather than
/// bunched up, so samples stay evenly spaced. Runs until aborted at the end of the run.
async fn pool_sampler(primary: Pool, standby: Option<Pool>, stats: Arc<PoolStats>, every: Duration) {
    let mut tick = tokio::time::interval(every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let s = primary.status();
        stats.primary.lock().unwrap().sample(s.size, s.available, s.waiting);
        if let (Some(p), Some(m)) = (&standby, &stats.standby) {
            let s = p.status();
            m.lock().unwrap().sample(s.size, s.available, s.waiting);
        }
    }
}

/// Helper task: every `every`, print one progress line (TPS since the last line, totals,
/// replication lag) plus one line per pool (usage since the last line, then reset).
///
/// Replication lag comes from `pg_stat_replication` over a **dedicated** connection (the
/// raw `--primary` DSN), so monitoring never takes a slot from the workload's pool (D12). If
/// that connection fails, progress lines are still printed without lag.
async fn progress_reporter(
    ctx: Arc<Ctx>,
    pools: Arc<PoolStats>,
    primary_dsn: String,
    every: Duration,
    start: Instant,
) {
    let mon = match tokio_postgres::connect(&primary_dsn, NoTls).await {
        Ok((client, conn)) => {
            tokio::spawn(conn);
            Some(client)
        }
        Err(e) => {
            eprintln!("progress: monitoring connection failed: {e}");
            None
        }
    };
    let lag_sql = "SELECT coalesce(string_agg(format('%s lag=%s replay_lag=%s', application_name,
                       pg_size_pretty(pg_wal_lsn_diff(pg_current_wal_insert_lsn(), replay_lsn)),
                       coalesce(replay_lag::text, '-')), ', '), 'no standby')
                   FROM pg_stat_replication";

    let mut tick = tokio::time::interval(every);
    tick.tick().await; // first tick fires immediately
    let (mut last_done, mut last_failed) = (0u64, 0u64);
    loop {
        tick.tick().await;
        let done = ctx.live.completed.load(Ordering::Relaxed);
        let failed = ctx.live.failed.load(Ordering::Relaxed);
        let lag = match &mon {
            Some(c) => c
                .query_one(lag_sql, &[])
                .await
                .map(|r| r.get::<_, String>(0))
                .unwrap_or_else(|e| format!("lag query failed: {e}")),
            None => "-".into(),
        };
        println!(
            "[{:>6.1}s] tps={:>8.1} completed={:>9} failed(+{})={} | {}",
            start.elapsed().as_secs_f64(),
            (done - last_done) as f64 / every.as_secs_f64(),
            done,
            failed - last_failed,
            failed,
            lag
        );
        // Pool usage since the previous report (then reset the interval accumulator)
        for (name, m) in pools.each() {
            let mut u = m.lock().unwrap();
            println!(
                "          pool {name:<8} in_use now {:>3}/{:<3} avg {:>6.1} max {:>3} | waiting now {:>3} avg {:>6.1} max {:>3}",
                u.now_in_use, u.max_size, u.interval.mean_in_use(), u.interval.max_in_use,
                u.now_waiting, u.interval.mean_waiting(), u.interval.max_waiting
            );
            u.interval = Default::default();
        }
        last_done = done;
        last_failed = failed;
    }
}

/// Print the final report: counters, `WAIT FOR` statuses, verify results, errors per step
/// (with the last message), the pool usage table and the latency table (one row per
/// non-empty histogram, in ms).
fn print_summary(args: &Args, t: &TaskStats, pools: &PoolStats, elapsed: Duration) {
    let secs = elapsed.as_secs_f64();
    println!("\n== Summary ({:.1}s) ==", secs);
    println!("completed txns      : {}  ({:.1} tps)", t.completed, t.completed as f64 / secs);
    if args.mode == Mode::Waitfor {
        println!(
            "WAIT FOR status     : success={} timeout={} not_in_recovery={} other={}",
            t.wait_success, t.wait_timeout, t.wait_not_in_recovery, t.wait_other
        );
    }
    if args.verify {
        println!("verify (row visible): ok={} missing={}", t.verify_ok, t.verify_missing);
    }
    if args.needs_lsn() {
        // REPRO: with --raw-insert-lsn, compare this with the WAIT FOR timeout count above
        println!(
            "page-header LSNs    : {}  ({})",
            t.boundary_lsns,
            if args.raw_insert_lsn { "waited for as-is: --raw-insert-lsn" } else { "corrected to the page boundary" }
        );
    }
    println!("errors              : {}", t.error_total());
    for (step, n) in &t.errors {
        println!("  {step:<16}: {n}  last: {}", t.last_error.get(step).map(String::as_str).unwrap_or(""));
    }

    println!("\npool usage     max_size  in_use avg  in_use max   util avg  waiting avg  waiting max");
    for (name, m) in pools.each() {
        let u = m.lock().unwrap();
        println!(
            "{:<14} {:>8} {:>11.1} {:>11} {:>9.1}% {:>12.1} {:>12}",
            name, u.max_size, u.run.mean_in_use(), u.run.max_in_use, u.run_util_pct(),
            u.run.mean_waiting(), u.run.max_waiting
        );
    }

    println!("\nlatency (ms)         count      mean       p50       p95       p99     p99.9       max");
    for (name, h) in t.h.named() {
        if h.is_empty() {
            continue;
        }
        println!(
            "{:<16} {:>9} {:>9.3} {:>9.3} {:>9.3} {:>9.3} {:>9.3} {:>9.3}",
            name,
            h.len(),
            h.mean() / 1000.0,
            q_ms(h, 0.50),
            q_ms(h, 0.95),
            q_ms(h, 0.99),
            q_ms(h, 0.999),
            h.max() as f64 / 1000.0
        );
    }
}

/// Append one line describing the run to the CSV file at `path`. The header is written
/// first if the file is new or empty. The column order must match between the header, the
/// format string and the argument list. When adding a column, change all three together
/// and update `PARAMETERS.md`.
fn write_csv(
    path: &std::path::Path,
    args: &Args,
    info: &ServerInfo,
    t: &TaskStats,
    pools: &PoolStats,
    elapsed: Duration,
) -> Result<(), BoxError> {
    let new_file = !path.exists() || std::fs::metadata(path)?.len() == 0;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    if new_file {
        writeln!(
            f,
            "ts,label,server_version,mode,sync_commit,wal_writer_delay,wal_writer_flush_after,wait_mode,wait_timeout_ms,\
             tasks,primary_pool,standby_pool,verify,fetch_lsn,payload_size,elapsed_s,completed,tps,\
             errors,wait_timeout,wait_not_in_recovery,verify_missing,\
             total_mean_ms,total_p50_ms,total_p95_ms,total_p99_ms,total_max_ms,\
             commit_p50_ms,commit_p99_ms,lsn_fetch_p50_ms,wait_p50_ms,wait_p95_ms,wait_p99_ms,\
             primary_acquire_p99_ms,standby_acquire_p99_ms,\
             primary_inuse_avg,primary_inuse_max,primary_waiting_avg,primary_waiting_max,\
             standby_inuse_avg,standby_inuse_max,standby_waiting_avg,standby_waiting_max,\
             raw_insert_lsn,boundary_lsns"
        )?;
    }
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
    let secs = elapsed.as_secs_f64();
    let h = &t.h;
    // Pool usage columns (standby columns are empty when the standby pool isn't used)
    let usage = |m: Option<&Mutex<PoolUsage>>| match m {
        Some(m) => {
            let u = m.lock().unwrap();
            format!("{:.2},{},{:.2},{}", u.run.mean_in_use(), u.run.max_in_use,
                    u.run.mean_waiting(), u.run.max_waiting)
        }
        None => ",,,".to_string(),
    };
    writeln!(
        f,
        "{ts},\"{}\",{},{},{},{},{},{},{},{},{},{},{},{},{},{:.2},{},{:.1},{},{},{},{},\
         {:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{},{},{},{}",
        args.label.replace('"', "'"),
        info.version.split_whitespace().next().unwrap_or(""),
        args.mode.name(),
        info.sync_commit,
        info.wal_writer_delay,
        info.wal_writer_flush_after,
        if args.mode == Mode::Waitfor { args.wait_mode.as_sql() } else { "" },
        args.wait_timeout.as_millis(),
        args.tasks,
        args.primary_pool,
        if args.uses_standby() { args.standby_pool } else { 0 },
        args.verify,
        args.needs_lsn(),
        args.payload_size,
        secs,
        t.completed,
        t.completed as f64 / secs,
        t.error_total(),
        t.wait_timeout,
        t.wait_not_in_recovery,
        t.verify_missing,
        if h.total.is_empty() { 0.0 } else { h.total.mean() / 1000.0 },
        q_ms(&h.total, 0.50),
        q_ms(&h.total, 0.95),
        q_ms(&h.total, 0.99),
        h.total.max() as f64 / 1000.0,
        q_ms(&h.commit, 0.50),
        q_ms(&h.commit, 0.99),
        q_ms(&h.lsn_fetch, 0.50),
        q_ms(&h.wait, 0.50),
        q_ms(&h.wait, 0.95),
        q_ms(&h.wait, 0.99),
        q_ms(&h.primary_acquire, 0.99),
        q_ms(&h.standby_acquire, 0.99),
        usage(Some(&pools.primary)),
        usage(pools.standby.as_ref()),
        args.raw_insert_lsn,
        t.boundary_lsns,
    )?;
    Ok(())
}
