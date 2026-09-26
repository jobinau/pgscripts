// WAIT FOR LSN evaluation driver -------------------------------------------------------
// Two deadpool-postgres pools (primary + standby); see CLAUDE.md for the idea and test plan.
mod cli;
mod metrics;
mod workload;

use clap::Parser;
use cli::{Args, Mode};
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
use metrics::{q_ms, Live, TaskStats};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_postgres::{Config, NoTls};
use workload::{wait_for_lsn, Ctx, WaitStatus};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

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

/// Parse a DSN and add our defaults: application_name, connect_timeout, and (primary only)
/// the synchronous_commit setting applied to every new session via the startup `options`.
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

/// Open every connection up front so the measurement doesn't include connection setup.
async fn warm_up(pool: &Pool, n: usize) -> Result<(), BoxError> {
    let mut held = Vec::with_capacity(n);
    for _ in 0..n {
        held.push(pool.get().await?);
    }
    Ok(()) // all dropped -> back in the pool
}

async fn show(client: &tokio_postgres::Client, setting: &str) -> Result<String, BoxError> {
    let row = client.query_one(&format!("SHOW {setting}"), &[]).await?;
    Ok(row.get(0))
}

/// Facts about the servers that go into the report / CSV.
struct ServerInfo {
    version: String,
    sync_commit: String,
    wal_writer_delay: String,
    sync_standby_names: String,
}

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
            sync_standby_names: show(&c, "synchronous_standby_names").await?,
        }
    };

    if let Some(sp) = &standby {
        // Make sure the schema change has reached the standby, and that WAIT FOR works at all.
        let lsn: String = primary.get().await?.query_one(workload::LSN_SQL, &[]).await?.get(0);
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
    if args.mode == Mode::Waitfor {
        println!("wait mode / timeout : {} / {:?}", args.wait_mode.as_sql(), args.wait_timeout);
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

    let start = Instant::now();
    let reporter = if args.report_interval > Duration::ZERO {
        Some(tokio::spawn(progress_reporter(
            Arc::clone(&ctx),
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

    //------------------------- 5. Report ------------------------------------------------------
    print_summary(&args, &total, elapsed);
    if let Some(path) = &args.output {
        write_csv(path, &args, &info, &total, elapsed)?;
        println!("\nsummary appended to {}", path.display());
    }
    Ok(())
}

/// Every interval print TPS and the replication lag (from a dedicated monitoring connection,
/// so it doesn't take a connection away from the primary pool).
async fn progress_reporter(ctx: Arc<Ctx>, primary_dsn: String, every: Duration, start: Instant) {
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
        last_done = done;
        last_failed = failed;
    }
}

fn print_summary(args: &Args, t: &TaskStats, elapsed: Duration) {
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
    println!("errors              : {}", t.error_total());
    for (step, n) in &t.errors {
        println!("  {step:<16}: {n}  last: {}", t.last_error.get(step).map(String::as_str).unwrap_or(""));
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

fn write_csv(
    path: &std::path::Path,
    args: &Args,
    info: &ServerInfo,
    t: &TaskStats,
    elapsed: Duration,
) -> Result<(), BoxError> {
    let new_file = !path.exists() || std::fs::metadata(path)?.len() == 0;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    if new_file {
        writeln!(
            f,
            "ts,label,server_version,mode,sync_commit,wal_writer_delay,wait_mode,wait_timeout_ms,\
             tasks,primary_pool,standby_pool,verify,fetch_lsn,payload_size,elapsed_s,completed,tps,\
             errors,wait_timeout,wait_not_in_recovery,verify_missing,\
             total_mean_ms,total_p50_ms,total_p95_ms,total_p99_ms,total_max_ms,\
             commit_p50_ms,commit_p99_ms,lsn_fetch_p50_ms,wait_p50_ms,wait_p95_ms,wait_p99_ms,\
             primary_acquire_p99_ms,standby_acquire_p99_ms"
        )?;
    }
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
    let secs = elapsed.as_secs_f64();
    let h = &t.h;
    writeln!(
        f,
        "{ts},\"{}\",{},{},{},{},{},{},{},{},{},{},{},{},{:.2},{},{:.1},{},{},{},{},\
         {:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3}",
        args.label.replace('"', "'"),
        info.version.split_whitespace().next().unwrap_or(""),
        args.mode.name(),
        info.sync_commit,
        info.wal_writer_delay,
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
    )?;
    Ok(())
}
