# waitforlsn – design and source guide

This document explains **how the program works and why it is built this way**. It is also
the front page of the generated source documentation (`make doc`), where each module,
function and struct is documented in detail.

Related documents: `README.md` (quick start), `PARAMETERS.md` (every option and output
field), `CLAUDE.md` (project status, findings, test plan).

---

## 1. What the program measures

The application needs to know when a transaction is **complete**, meaning it is safe to
tell the user "done", and a later read from the standby will see the change. Two ways to
get there are compared:

| Test | Primary | The transaction is complete when… |
|------|---------|--------------------------------|
| **T1** `--mode sync` | `synchronous_commit = on` (local WAL flush) | `COMMIT` returns on the primary |
| **T2** `--mode waitfor` | `synchronous_commit = off` (async commit) | `WAIT FOR LSN '<commit lsn>'` returns `success` on the standby |

T2 moves the waiting from the primary's commit path to the application, and waits for the
standby instead of the local disk. The program measures what this costs (latency,
throughput, connections) and what it buys (read-your-writes on the standby, durability).

---

## 2. Architecture

```text
 ┌─────────────────────────────────── waitforlsn process (tokio runtime) ─────────────────────────────────┐
 │                                                                                                        │
 │   main.rs                         workload.rs                              metrics.rs                  │
 │   ───────                         ───────────                              ──────────                  │
 │   parse Args (cli.rs)             run_task × --tasks ──► TaskStats (own)   Hists (HDR, µs)             │
 │   build pools, checks        ┌──► primary_step ──┐       merged at end     TaskStats, Live (atomics)   │
 │   spawn tasks + helpers ─────┤                   │                         PoolUsage / UsageAcc        │
 │   summary + CSV              └──► standby_step ──┤                                                     │
 │                                                  │                                                     │
 │   pool_sampler ──(Pool::status every 10ms)──► PoolStats                                                │
 │   progress_reporter ──(own monitoring connection)──► pg_stat_replication                               │
 └───────────────┬──────────────────────────────────┬─────────────────────────────────────────────────────┘
                 │ primary pool (deadpool)          │ standby pool (deadpool)
        ┌────────▼────────┐    WAL streaming    ┌───▼─────────────┐
        │  primary        │ ──────────────────► │  standby        │
        │  INSERT, LSN    │  (async, physical,  │  WAIT FOR LSN   │
        │                 │   slot standby1)    │  verify SELECT  │
        └─────────────────┘                     └─────────────────┘
```

Source files:

| File | Responsibility |
|------|----------------|
| `src/main.rs` | program lifecycle: runtime, pools, startup checks, spawning, summary, CSV, pool sampler, progress reporter |
| `src/cli.rs` | all command-line options (`Args`) and the derived settings (`sync_commit()`, `uses_standby()`, …) |
| `src/workload.rs` | one worker task's transaction loop, the SQL, `WAIT FOR` helper, error formatting |
| `src/metrics.rs` | histograms, counters, pool usage accumulators, merge logic |

---

## 3. Overall flow

### 3.1 Program lifecycle

```text
main()
 │  Args::parse()                                   cli.rs: options + defaults
 │  build tokio runtime (--threads)
 └─ run(args)
     │
     ├─ 1. Pools
     │     primary pool  = make_pool(make_config(--primary, "wfl_primary", synchronous_commit))
     │     standby pool  = only if uses_standby()  (waitfor mode, or --verify)
     │
     ├─ 2. Startup checks + schema
     │     primary: pg_is_in_recovery() must be false
     │              CREATE TABLE IF NOT EXISTS wfl_test; TRUNCATE if --truncate
     │              read server_version, synchronous_commit, wal_writer_* for the report
     │     standby: pg_is_in_recovery() must be true
     │              WAIT FOR LSN <primary insert lsn>  → table is visible on the standby,
     │                                                   and WAIT FOR works at all
     │
     ├─ 3. Warm-up: open every pool connection now (not during measurement)
     │
     ├─ 4. Print run configuration
     │
     ├─ 5. Run
     │     build Ctx (shared, read-only + atomics)
     │     spawn: Ctrl-C handler      → sets ctx.stop
     │            pool_sampler        → PoolStats (every --pool-sample-interval)
     │            progress_reporter   → prints every --report-interval
     │            run_task × --tasks  → each returns its TaskStats
     │     await all tasks, merge TaskStats; abort sampler + reporter
     │
     └─ 6. Report: print_summary(); write_csv() if --output
```

### 3.2 One worker task

Each task is a simulated application client. It runs transactions back to back (closed
loop, no think time) until `Ctx::keep_going()` says stop: deadline reached, `--txns`
reached, or Ctrl-C.

```text
run_task(ctx, task_id)
 loop while keep_going():
   t0 = now
   ┌─ primary_step ────────────────────────────────────────────────────────────┐
   │  conn = primary_pool.get()                        → primary_acquire       │
   │  INSERT INTO wfl_test ... RETURNING id            → commit                │
   │     (autocommit: this statement IS the transaction)                       │
   │  if needs_lsn: SELECT pg_current_wal_insert_lsn() → lsn_fetch             │
   │                normalize_insert_lsn (page-boundary fix, D2)               │
   │  drop(conn)   ← connection returns to the pool BEFORE any waiting         │
   └───────────────────────────────────────────────────────────────────────────┘
        error → count per step, print first 10, sleep 100ms, next iteration
   ┌─ standby_step (only if the standby pool exists) ──────────────────────────┐
   │  conn = standby_pool.get()                        → standby_acquire       │
   │  waitfor: WAIT FOR LSN 'lsn' WITH (MODE, TIMEOUT, NO_THROW) → wait_for_lsn│
   │           status: success | timeout | not in recovery                     │
   │  if --verify and (sync mode or success):                                  │
   │           SELECT 1 FROM wfl_test WHERE id = $1    → verify                │
   │  drop(conn)                                                               │
   └───────────────────────────────────────────────────────────────────────────┘
   complete?  sync: primary step OK (and standby step OK if it ran)
              waitfor: status == success
   yes → record total = now - t0, completed += 1
   no  → failed += 1   (not in the latency histogram, not in TPS)
```

### 3.3 T1 vs T2 on the wire

```text
T1  sync                                   T2  waitfor
app        primary          standby        app        primary          standby
 │ INSERT ──►│                               │ INSERT ──►│
 │           │ write WAL                     │           │ write WAL (buffers)
 │           │ fsync WAL  ◄─ the wait        │◄── id ────│ return immediately
 │           │ wake walsender ──► WAL        │ SELECT lsn►│
 │◄── id ────│                  write/       │◄── lsn ───│
 │  (done)   │                  flush/       │ WAIT FOR LSN ──────────────►│ sleeps until
 │                              replay       │           │ WAL writer     │ LSN reached
 │                                           │           │ fsync (≤ delay)│
 │                                           │           │ ──── WAL ─────►│ write
 │                                           │           │                │ flush
 │                                           │           │                │ replay
 │                                           │◄───────────── success ─────│
 │                                           │  (done)
```

In T1 the backend itself flushes WAL before `COMMIT` returns. In T2 nobody flushes at
commit time: the WAL writer process does it later, and only then can the WAL leave the
primary (see D2). That is why WAL writer settings dominate T2 latency.

### 3.4 Concurrency and shared state

```text
                      Arc<Ctx>  (shared by all tasks, the reporter and the Ctrl-C handler)
            ┌───────────────────────────────────────────────────────────────┐
            │ read-only : mode, pools, SQL settings, payload, deadline      │
            │ atomics   : issued (for --txns), stop (Ctrl-C),               │
            │             live.completed / live.failed (progress lines),    │
            │             errors_printed                                    │
            └───────────────────────────────────────────────────────────────┘
 run_task 0 ─► TaskStats 0 ┐
 run_task 1 ─► TaskStats 1 ├─► merged once at the end ─► summary / CSV
 run_task N ─► TaskStats N ┘

 pool_sampler ─► Arc<PoolStats> (Mutex per pool) ◄─ progress_reporter (reads + resets interval)
                                                 ◄─ summary / CSV (reads run totals)
```

The hot path (a transaction) touches no locks. It only does relaxed atomic increments and
writes into the task's own `TaskStats`.

### 3.5 Where the time goes in T2

```text
 app:     |--acquire--|--INSERT/commit--|--lsn fetch--|--acquire--|------------ WAIT FOR ------------|
 primary:              commit record in WAL buffers ...... WAL writer flush ── walsender sends ──►
 standby:                                                                  write ── flush ── replay
                                                                             ▲       ▲        ▲
                                                               standby_write │ standby_flush  │ standby_replay
```

The `WAIT FOR` part is roughly: time until the WAL writer flushes the commit record,
plus network, plus standby write/fsync/replay. The first term is controlled by
`wal_writer_delay` and `wal_writer_flush_after` (D2, D13).

---

## 4. Design decisions

Each decision has an ID. Code comments refer to these (e.g. "see DESIGN.md D2").

### D1. Two pools; release the primary connection before waiting
- **Problem:** in T2 a transaction spends most of its time waiting on the standby.
- **Decision:** separate deadpool pools for primary and standby, sized independently.
  `primary_step` returns (dropping its connection) *before* `standby_step` starts.
- **Why:** if the primary connection were held during the wait, the primary pool would have
  to be as large as the number of waiting clients. Replication lag would turn into
  primary connection exhaustion. This way the primary pool only covers the short
  INSERT + LSN round trips, and the waiting load lands on the standby pool.
- **Consequence:** the standby pool needs about `tasks × wait / total` connections. That's
  why pool usage is measured (D9).

### D2. Wait for `pg_current_wal_insert_lsn()`, read after the commit, on the same connection
- **Problem:** which LSN proves "my commit reached the standby"?
- **Options:**
  - `pg_current_wal_flush_lsn()`: **wrong** with async commit. When `COMMIT` returns, the
    commit record may not be flushed yet, so the flush LSN can be *before* our commit
    record. Waiting for it could return `success` for a transaction the standby doesn't
    have yet.
  - `pg_current_wal_lsn()` (write position): same problem, one step later.
  - The exact end LSN of our commit record: PostgreSQL doesn't return it to the client.
  - **`pg_current_wal_insert_lsn()` after `COMMIT` returned**: the insert position is at
    or after the end of every record already inserted, including our commit record.
- **Decision:** the last option, run as a separate autocommit statement on the same
  connection after the INSERT has returned, then corrected for the page-boundary case
  below (`normalize_insert_lsn`).
- **Why the details matter:**
  - *After* the commit: an LSN read inside the transaction, or in the same multi-statement
    simple query (which runs as one implicit transaction), comes from before the commit record.
  - It's a (slightly) conservative target. It may include other sessions' later WAL, so
    we might wait a bit longer than strictly needed, but never too short.
- **Page-boundary case (found in testing, fixed):** when the last inserted record ends
  exactly at a WAL page boundary, the insert position is reported *after the next page's
  header* (boundary + 24 bytes, or + 40 at the start of a segment). No record ends
  there. The standby's write/flush/replay positions stop at the boundary and only pass
  the header when the *next* record arrives. With a closed-loop workload, all clients can
  be waiting for that same LSN at once, so no new WAL is written and every `WAIT FOR` sleeps
  until its timeout.
  - Observed with 4 tasks: about 1 in 2000 waits (`standby_flush` and `standby_replay`)
    timed out after the full 3s, all 4 tasks on the same LSN `0/0CAE2018`
    (page offset 24).
  - Fix: `normalize_insert_lsn` moves such an LSN back to the boundary, where the previous
    record really ended. Page and segment sizes are read from the server at startup. After
    the fix: 0 timeouts in about 83k waits, max wait 39ms.
  - This affects **any** application that waits for `pg_current_wal_insert_lsn()`. The
    same root cause is under discussion on pgsql-hackers (thread "test: avoid redundant
    standby catchup in 049_wait_for_lsn", patches to expose `GetXLogInsertEndRecPtr()` to
    SQL). The standalone reproducer, evidence and draft reply are in `repro/`
    (`PGSQL_HACKERS_REPORT.md`). `--raw-insert-lsn` turns the correction off, to demonstrate
    the stall or to check an upstream fix.
- **Background verified in the PostgreSQL source:**
  - `walsender.c` `XLogSendPhysical()` sends only up to `GetFlushRecPtr()`: *"it's unsafe to
    send WAL that is not securely down to disk on the primary"*. So in T2 the WAL leaves
    the primary only after the WAL writer has flushed it.
  - Consequence: a `standby_flush`/`standby_replay` success implies the commit is also
    flushed on the **primary**. T2 is at least as durable as T1, plus a copy on the standby.
  - `xlog.c` `XLogSetAsyncXactLSN()` / `XLogBackgroundFlush()`: an async commit wakes the WAL
    writer only if it is hibernating, or if `wal_writer_flush_after` blocks are pending
    (always, if it is `0`). The WAL writer flushes only *complete* WAL pages unless nothing
    else is pending or `wal_writer_delay` has passed since its last flush. So a commit
    sitting on a partial page can wait up to about `wal_writer_delay`.
  - Measured consequence (local, 32 tasks): with the default `wal_writer_flush_after=1MB`
    and `wal_writer_delay=200ms`, T2 does about 104 tps (p50 392ms). With
    `wal_writer_flush_after=0`, T2 does 2706 tps (p50 10.9ms), slightly better than T1's
    2634 tps (p50 12.0ms), while also giving read-your-writes and a flush on both servers.
    The tail still reaches about `wal_writer_delay` (p99.9 212ms with a 200ms delay, 20ms with
    10ms), because of the partial-page rule above. So the two settings work together:
    `flush_after=0` for the median, and a small `wal_writer_delay` for the tail.
- **Cost:** one extra round trip per transaction (`lsn_fetch`). `--fetch-lsn` adds the same
  round trip to T1 for a fair comparison.

### D3. `WAIT FOR` through `simple_query`, with a validated literal and `NO_THROW`
- The LSN in `WAIT FOR LSN '...'` is part of the statement syntax, not an expression, so
  it can't be a bind parameter. The statement is built as text (`wait_sql`) and sent with
  `simple_query`. Preparing it would give a new statement for every LSN.
- Building SQL from a string needs protection: `is_valid_lsn` only accepts `hex/hex`
  (at most 8 digits each), so nothing else can be injected. The LSN comes from our own
  server, but the check costs nothing.
- `NO_THROW` makes `timeout` and `not in recovery` come back as a **status row** instead of
  an error. Expected outcomes are data (`WaitStatus`). Errors are reserved for real
  failures (connection lost, syntax).
- `WAIT FOR` must be the first thing in a transaction on its connection (a held snapshot
  or lock could block the replay it waits for). Autocommit on a clean pooled connection
  guarantees that. The verify `SELECT` comes *after* the wait, never before.

### D4. `synchronous_commit` set per session through the startup packet
- `make_config` appends `-c synchronous_commit=<value>` to the libpq `options` of the
  primary pool. Every pooled connection starts with the right setting.
- **Why not `ALTER SYSTEM`:** T1 and T2 can run against the same cluster, back to back,
  without changing server config. `--synchronous-commit` allows extra baselines
  (`remote_apply`, …) the same way.
- **Why not `SET` after checkout:** it would cost a round trip, or a pool hook, and could be
  lost if a connection is replaced. Startup options are applied by the server before the
  first query.
- The effective value is read back with `SHOW` and printed/stored, so a typo is visible.

### D5. The transaction is a single autocommit `INSERT ... RETURNING id`
- The smallest real write transaction: one round trip, one row, one commit record. This
  isolates the commit cost, which is what differs between T1 and T2.
- `RETURNING id` gives the key for the read-your-writes check (`--verify`).
- `--payload-size` scales WAL volume per transaction without changing the shape.
- Statements are prepared once per connection (`prepare_cached`, deadpool's per-connection
  cache), except `WAIT FOR` (D3).

### D6. What counts as "completed", and what is timed
- **Completed** = the application may report success: in `sync`, the commit (plus the
  optional verify) succeeded. In `waitfor`, `WAIT FOR` returned `success`.
- `timeout` / `not in recovery` are **not** completed. The row is committed on the primary,
  but the application can't confirm it. That's exactly the case an application would have
  to handle (retry the wait, read from the primary, or report "unknown").
- `total` latency and TPS include only completed transactions. Each step has its own
  histogram that includes all attempts (e.g. `wait_for_lsn` includes timeouts), so slow
  failures stay visible.
- Timing is client-side (`Instant`), so it includes network and pool queuing: what the
  application actually experiences.

### D7. Closed-loop load with tokio tasks
- `--tasks` tasks each run one transaction at a time with no pause. Offered load follows
  the system's speed. This finds saturation throughput and latency at a given concurrency,
  not behaviour at a fixed arrival rate. (An open-loop, rate-limited mode could be added later.)
- Tasks are tokio tasks on a multi-thread runtime (`--threads`), so thousands of clients
  cost little. The number of *connections* is set by the pools, not by `--tasks`, as in a
  real pooled application.

### D8. Metrics: per-task HDR histograms, merged at the end
- Each task owns a `TaskStats` (7 HDR histograms in microseconds, 1µs to 10min, 3
  significant digits, plus counters). Nothing is shared on the hot path, so measuring
  doesn't distort the measurement.
- After the run, all `TaskStats` are merged (histograms add losslessly) for exact
  percentiles over all transactions.
- Progress lines need live numbers, so `Live` holds two relaxed atomic counters.

### D9. Pool usage by sampling `Pool::status()`
- deadpool reports `size`, `available` and `waiting`. `in_use = size - available`.
- A `pool_sampler` task polls both pools every `--pool-sample-interval` (10ms) into
  `PoolUsage`: current values, an **interval** accumulator (reset by each progress line)
  and a **run** accumulator (summary + CSV).
- **Why sampling instead of instrumenting every `get()`:** no extra work on the hot path.
  Short spikes between samples can be missed, but queuing also shows exactly in the
  `*_acquire` histograms.

### D10. Startup checks and warm-up
- Server role checks (`pg_is_in_recovery`) catch swapped or wrong DSNs, which would
  otherwise produce plausible but meaningless numbers.
- The startup `WAIT FOR` does three things: proves the feature works, proves the standby
  is streaming from *this* primary, and guarantees `wfl_test` exists on the standby
  before any verify query is prepared.
- Warm-up opens every pool connection first, so connection setup (fork, auth) isn't
  counted as transaction latency.

### D11. Errors are data; the run continues
- Every error is tagged with its step (`primary_acquire`, `commit`, `lsn_fetch`,
  `standby_acquire`, `wait_for_lsn`, `verify`), counted, and the last message kept. Only
  the first 10 are printed, so a dead server doesn't flood the terminal.
- After an error the task sleeps 100ms, so an outage doesn't become a busy loop.
- Broken connections are discarded by deadpool on return (`RecyclingMethod::Fast` checks
  `is_closed()`), and new ones are opened on demand. So a restarted standby is picked up
  again automatically (verified: stop/start and promotion tests).
- On a `WAIT FOR` timeout, `diagnose_timeout` captures the primary's flush/write/insert
  positions, the walsender's `sent_lsn` and the standby's receive/replay positions, and
  adds them to the timeout message. This tells where a target got stuck (see
  repro/REPRODUCE_WITH_WAITFORLSN.md §5).
- `pg_err`/`pool_err` show SQLSTATE and the server message. tokio-postgres' own `Display`
  just says "db error".

### D12. Monitoring on its own connection
- The progress reporter queries `pg_stat_replication` over a dedicated connection, not a
  pooled one, so monitoring never competes with the workload for pool slots or skews
  pool usage.

### D13. Environment choices (Docker, not Rust)
- WAL writer settings (`wal_writer_delay`, `wal_writer_flush_after`) are set with
  `ALTER SYSTEM` (init script), not `-c` on the command line. Command-line settings override `ALTER SYSTEM`, which would make runtime changes
  (`scripts/set_wal_writer_delay.sh`, `run_matrix.sh`) impossible without a restart.
- Asynchronous physical replication with a replication slot (the standby can't fall behind
  WAL retention) and `hot_standby_feedback=on` (fewer replay conflicts cancelling standby
  queries).
- Ports 5433/5434, so a local PostgreSQL on 5432 isn't disturbed.

---

## 5. Known limitations

- **Single-row transactions only.** Larger transactions change the commit-to-WAL ratio.
  `--payload-size` covers WAL volume but not multi-statement transactions.
- **Conservative LSN (D2).** Waiting for the insert position can include other sessions'
  WAL. Slight over-waiting under high concurrency.
- **Header sizes are hard-coded** (24/40 bytes, D2). They have been stable across
  PostgreSQL versions, but a WAL format change would need `SHORT_PAGE_HEADER`/`LONG_PAGE_HEADER` updated.
- **Local measurements** (WSL2, one disk for both servers) say little about fsync costs
  and network latency. Use the EC2 layout for real numbers.
- **Promotion handling** is only counted (`not in recovery`), not acted on. A real
  application would re-route reads to the new primary.
- The pool sampler can miss spikes shorter than its interval (D9).

---

## 6. Where to change things

| To… | Change |
|-----|--------|
| add an option | `cli.rs` (`Args` field + doc comment), use it in `main.rs`/`Ctx`, document it in `PARAMETERS.md` |
| change the transaction | `workload.rs`: `INSERT_SQL`, `primary_step` (keep D1 and D2 intact) |
| add a timed step | a histogram in `Hists` (`new`, `merge`, `named`), `rec()` it in `workload.rs`, CSV column in `write_csv` |
| add a CSV column | `write_csv` header **and** format string **and** argument, same position; update `PARAMETERS.md` |
| handle a new `WAIT FOR` status | `WaitStatus::parse` and the `match` in `run_task` |
