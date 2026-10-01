# WAIT FOR LSN evaluation (PostgreSQL 19)

## Status (keep this section current)

- **2026-09-26**: Docker environment, Rust app and matrix scripts are implemented and
  tested locally (WSL2, single host). T1, T2, standby stop/start and promotion all work.
  First local numbers are in *Findings so far*.
- **2026-09-26**: Added pool usage reporting (connections in use and tasks waiting per pool,
  sampled from deadpool `Pool::status()`) to progress lines, summary and CSV.
- **2026-09-26**: Added `PARAMETERS.md`, the user reference for every option, the
  output/CSV columns and the script env vars. **Keep it in sync whenever an option, an
  output field or a CSV column changes.**
- **2026-09-26**: Source documentation: `DESIGN.md` (overall flow, architecture, decisions
  D1–D13, limitations) is the rustdoc front page. Every module, function, struct, field and
  const has a doc comment. `make doc`, `make lint` (clippy `missing_docs_in_private_items`,
  warnings are errors), `make test` (7 unit tests).
- **2026-09-26**: **Bug found and fixed (D2): WAIT FOR stalls when the target LSN is just
  past a WAL page header.** About 1 in 2000 waits hit the full timeout.
  `normalize_insert_lsn` moves such LSNs back to the page boundary. After the fix: 0
  timeouts in about 83k waits. Timeouts are now logged with their LSN.
- **2026-09-26**: Prepared the pgsql-hackers report on the page-boundary stall in `repro/`:
  a standalone psql reproducer (`repro.sh` + `find_boundary.sql`, retries until no background
  WAL interferes; shows all 4 modes time out, including `primary_flush` on a single node),
  background (`PGSQL_HACKERS_REPORT.md`), and a plain-text reply (`pgsql-hackers-reply.txt`)
  for the existing thread "test: avoid redundant standby catchup in 049_wait_for_lsn",
  where Xuneng Zhou posted the same root cause and patches on 2026-09-22. Added
  `--raw-insert-lsn` (disables the workaround). Measured: without the workaround a stalled
  wait runs to the full 20s timeout. With 1 client, 5,616 transactions completed in 60s,
  against 11,364 with it. **Not sent yet**: the user will send it.
- **2026-09-29**: Added `repro/REPRODUCE_WITH_WAITFORLSN.md`: how to reproduce the
  page-boundary stall with the program (`--raw-insert-lsn`, or the documented code edits, which were
  checked to compile), `REPRO:` markers in the code (`grep -n REPRO src/*.rs`), and a
  `boundary_lsns` counter (summary line "page-header LSNs", CSV columns `raw_insert_lsn`,
  `boundary_lsns`). Startup check now always uses a corrected LSN. Timeout messages include
  a WAL position snapshot (`diagnose_timeout`).
- **2026-09-29, OPEN: a second, rarer stall that the fix does not cover.** All 4 tasks timed out
  (3s) on `0/12C283A0`, an ordinary commit end, not a page header. Snapshot: primary
  flush=write=insert=target, but standby receive=`0/12C28000` (a page boundary). So the WAL
  was flushed but not streamed for 3s. Suspected: a lost walsender wakeup. The walsender reads
  `GetFlushRecPtr()` in `XLogSendPhysical()`, and only later joins `wal_flush_cv` in
  `WalSndWait()`, so a flush + `ConditionVariableBroadcast()` in between wakes nobody. The
  walsender then sleeps until standby feedback, a keepalive or the next flush.
  Seen 2 times in about 140k waits (4 tasks), then 0 in about 246k waits (8 × 60s).
  **2026-09-30: captured with `walsender sent`:** target `0/18E8F8E8`, primary flush = target,
  walsender sent = standby receive = `0/18E8F6A8` (mid-page). So the walsender didn't send
  flushed WAL for 3s: it's the walsender side, not the standby or `WAIT FOR`. The exact race
  (read flush ptr, then PrepareToSleep) is still a hypothesis. Next: confirm the race in the
  source/with instrumentation, and hunt on EC2. Not in the pgsql-hackers draft yet.
- **2026-09-30**: Re-tested the page-boundary reproduction from a clean start: reproduced in 3/3
  60s raw runs (32/8/8 timeouts), control run with the fix had 0. The copy-paste steps are at the
  top of `repro/REPRODUCE_WITH_WAITFORLSN.md`.
- **2026-09-26**: Found that `wal_writer_flush_after=0` makes T2 as fast as T1 (see
  Findings). Added the `wal_writer_flush_after` CSV column,
  `set_wal_writer_delay.sh <delay> [flush_after]`, and `WWD="delay:flush_after"` in the matrix.
- **Next**: provision EC2 (see *EC2 test plan*), run `scripts/run_matrix.sh`, record
  results here.

## Objective

Evaluate the `WAIT FOR LSN` command introduced in PostgreSQL 19 as an
application-level alternative to paying for synchronous commit on every
transaction.

We compare two ways for an application to decide "this transaction is complete":

| Test | Primary setting | When the app considers the txn complete |
|------|-----------------|-----------------------------------------|
| **T1 – baseline** (`--mode sync`) | `synchronous_commit = on` (no `synchronous_standby_names`, so this is a **local WAL flush** only) | As soon as `COMMIT` returns on the primary |
| **T2 – WAIT FOR** (`--mode waitfor`) | `synchronous_commit = off` (async commit, primary acks before its WAL is flushed) | After `COMMIT` returns on the primary **and** `WAIT FOR LSN '<commit lsn>'` returns `success` on the standby |

Questions to answer:
1. Throughput (TPS) and commit latency (p50/p95/p99/max) of T1 vs T2.
2. How much time goes into each part of T2 (primary commit, fetching the LSN, standby wait).
3. How many connections each pool needs, and whether the standby pool becomes the bottleneck (a waiting session holds its connection for the whole wait).
4. Read-your-writes correctness: after the wait succeeds, is the row always visible on the standby (`standby_replay`), and what do the different `MODE`s cost?
5. What happens with timeouts, a lagging standby, a standby restart and a standby promotion (`not in recovery`).

## Architecture

```
                 ┌────────────────────── Rust app (tokio) ───────────────────────┐
                 │                                                               │
                 │  N worker tasks                                               │
                 │    ├─ primary_pool (deadpool-postgres) ──► INSERT / COMMIT    │
                 │    │                                       SELECT commit LSN  │
                 │    └─ standby_pool (deadpool-postgres) ──► WAIT FOR LSN ...   │
                 │                                            (optional verify)  │
                 └───────────────┬───────────────────────────────┬───────────────┘
                                 │                               │
                        ┌────────▼────────┐  streaming  ┌────────▼────────┐
                        │  pg19-primary   │────────────►│  pg19-standby   │
                        │  host port 5433 │  physical   │  host port 5434 │
                        └─────────────────┘  replication└─────────────────┘
```

- **Two separate deadpool pools**: one to the primary (read-write), one to the standby
  (read-only), sized separately. The primary connection goes back to its pool
  **before** the standby wait starts, so replication lag never holds primary connections.
- Asynchronous streaming replication only: `synchronous_standby_names` is empty. The only
  thing that changes between T1 and T2 is `synchronous_commit` and whether the app waits
  on the standby. The app sets `synchronous_commit` on every primary session through the
  libpq startup `options` (`-c synchronous_commit=...`), so one cluster serves both tests.
  `--synchronous-commit` overrides it (e.g. for a `remote_apply` baseline).

## PostgreSQL 19 `WAIT FOR` – reference

```sql
WAIT FOR LSN 'lsn' [ WITH ( option [, ...] ) ]
-- option:
--   MODE 'standby_replay' | 'standby_write' | 'standby_flush' | 'primary_flush'
--   TIMEOUT 'timeout'        -- integer ms, or a string with units, e.g. '100ms', '5s'
--   NO_THROW                 -- return a status instead of raising an error on timeout
```

- Returns one row with a `status` column: `success`, `timeout`, `not in recovery`.
- Modes: `standby_replay` (default; the change is **visible** on the standby),
  `standby_flush` (**durable** on the standby), `standby_write` (written, not yet fsynced),
  `primary_flush` (runs on the primary and waits for its local flush).
- It must be a **top-level statement**: not inside a function, procedure or `DO` block. A
  snapshot or lock held on the standby could block the replay being waited for.
- The LSN is a literal, not a bind parameter. The app validates it (`is_valid_lsn`) and
  sends it with `simple_query`.
- Docs: https://www.postgresql.org/docs/19/sql-wait-for.html

Checked by hand on `postgres:19beta4`:
- Standby, `NO_THROW`, target not reached → `timeout`. Without `NO_THROW` it raises
  `ERROR: timed out while waiting for target LSN FF/00000000 to be replayed; current standby_replay LSN 0/03002DC0`.
- Primary, default mode → `not in recovery`. Primary, `MODE 'primary_flush'` → `success`.
- Standby, inside `BEGIN ISOLATION LEVEL REPEATABLE READ; SELECT 1;` →
  `ERROR: WAIT cannot be executed while the current transaction holds a snapshot`.
  After a `SELECT` under READ COMMITTED it is allowed (no snapshot is held between statements).
- `TIMEOUT '0ms'` means **wait forever** (it's not a single check). `--wait-timeout` must be > 0.
- **Page-boundary stall (important for any WAIT FOR user):** if the last record ends exactly at
  a WAL page boundary, `pg_current_wal_insert_lsn()` returns boundary + page header size
  (24, or 40 at a segment start). Standby positions stay at the boundary until more WAL
  arrives, so with no further writes `WAIT FOR` only ends at the timeout. Observed: all 4 tasks
  timed out on `0/0CAE2018` (page offset 24). Handled by `normalize_insert_lsn` (DESIGN.md D2).
- A promoted standby answers `not in recovery` immediately (about 1ms). The app sees a
  burst of fast "failures" and has to route away or back off itself.

## Which LSN to wait for (important)

With `synchronous_commit = off`, the commit record may **not be flushed** on the primary
when `COMMIT` returns. So `pg_current_wal_flush_lsn()` taken after commit can be *behind*
our commit record, and waiting for it could return `success` too early.

The app therefore runs `SELECT pg_current_wal_insert_lsn()` **after** `COMMIT` returns, on
the **same primary connection**. That position is at or past the end of our commit record,
so it is a safe (slightly conservative) target. It costs one extra round trip, recorded as
`lsn_fetch`. The result then goes through `normalize_insert_lsn`, which fixes the
page-boundary case (see the WAIT FOR reference above, and DESIGN.md D2). Never read the LSN in the same implicit transaction as the write: a
multi-statement `simple_query` runs as one transaction, so the LSN would come from before
the commit.

Implications:
- A physical walsender only ships WAL that the primary has already **flushed**. With async
  commit that flush is done by the WAL writer, so `wal_writer_delay` (default 200ms)
  sets the minimum T2 latency. **Confirmed locally**: see *Findings so far*.
- So a `standby_flush` success means the commit is on disk on **both** primary and
  standby: T2 is at least as durable as T1 (local flush only). **Verified in the PG source**:
  `walsender.c` `XLogSendPhysical()` sends only up to `GetFlushRecPtr()`, with a comment
  that unflushed WAL must never reach a standby.
- In T2 a primary crash *before* the wait succeeds can lose a transaction the primary
  already acked. The app must not report success until the wait succeeds.

## Findings so far (local, WSL2, single host; not representative for fsync costs)

10-second smoke runs, 32 tasks, pools 16/32, `postgres:19beta4`:

| Run | TPS | total p50 / p99 (ms) | WAIT FOR p50 / p99 (ms) | verify missing |
|-----|----:|------|------|----|
| T1 sync, `--fetch-lsn --verify` | 2434 | 12.6 / 26.6 | – | **23223 of 24393 (95%)** |
| T2 `standby_replay`, wal_writer_delay=200ms | 115 | 203.5 / 418.3 | 198.1 / 410.6 | 0 |
| T2 `standby_replay`, wal_writer_delay=10ms | 1527 | 23.0 / 37.1 | 16.6 / 28.5 | 0 |

- T2 latency is dominated by the WAL writer cycle: the wait is about 1–2× `wal_writer_delay`.
  With the default 200ms, T2 is unusable for OLTP. `wal_writer_delay` is the key tuning knob.
- Without WAIT FOR, a read on the standby straight after commit almost always misses the
  row. With WAIT FOR (`standby_replay`) it never did.
- Standby stopped for 5s: WAIT FOR/connection errors while it's down, then automatic
  recovery once it's back (deadpool drops the broken connections).
- Standby promoted: every wait returns `not in recovery` right away.
- **`wal_writer_flush_after=0` is the key T2 setting** (from reading `xlog.c`
  `XLogSetAsyncXactLSN`: every async commit then wakes the WAL writer, which flushes right
  away). 32 tasks, `standby_replay`, 8s runs:

  | Setting | TPS | total p50 / p99.9 (ms) |
  |---------|----:|------------------------|
  | T2 delay=200ms, flush_after=1MB (defaults) | 104 | 392 / 417 |
  | T2 delay=200ms, flush_after=0 | 2706 | 10.9 / 212 |
  | T2 delay=10ms, flush_after=0 | 2830 | 10.9 / 20 |
  | T1 sync (`--fetch-lsn`) | 2634 | 12.0 / 19 |

  With `flush_after=0` and a small delay, T2 matches or beats T1 on this box while adding
  read-your-writes and a flush on both servers. The tail is still bounded by
  `wal_writer_delay`, because the WAL writer flushes only whole pages unless the delay has
  passed. With few tasks (1–4) T2 is slower than T1: less batching, and the wait round trip
  dominates.
- Pool usage (32 tasks, wal_writer_delay=10ms): in T2 the standby pool is the one that
  fills up. It averaged 23.6 of 32 in use because each task holds a standby connection for
  the whole wait, while an 8-connection primary pool averaged only 4.2 in use (up to 23
  tasks queued briefly). In T1 the 16-connection primary pool was 99.6% used, with about
  16 tasks waiting on average. Pool sizing rule of thumb for T2: standby pool is about
  tasks × (wait time / total time), so it grows with `wal_writer_delay`.

## Development environment (Docker)

- Image `postgres:19beta4` (the newest PG19 tag on Docker Hub as of 2026-09-26), set in `.env` (`PG_IMAGE`).
  In this image PGDATA is `/var/lib/postgresql/19/docker`. The volume is mounted at
  `/var/lib/postgresql`.
- `docker-compose.yml` (project name `waitforlsn`):
  - `pg19-primary` (host port 5433): `docker/primary-init.sh` creates the `replicator` role,
    the physical slot `standby1_slot`, the pg_hba replication line, and sets
    `wal_writer_delay` / `wal_writer_flush_after` via `ALTER SYSTEM` (from `.env`). Using
    `ALTER SYSTEM`, not `-c`, keeps them changeable at runtime
    (`scripts/set_wal_writer_delay.sh`). `sql/schema.sql` creates `wfl_test`.
    Healthcheck uses `-h 127.0.0.1` so it doesn't turn healthy during the init-time
    socket-only server.
  - `pg19-standby` (host port 5434): `docker/standby-entrypoint.sh` runs
    `pg_basebackup -R -X stream -S standby1_slot` on first start, appends a
    `primary_conninfo` with `application_name=standby1`, then execs the stock entrypoint.
    `PRIMARY_HOST` / `PRIMARY_REPL_PORT` in `.env` point it at a remote primary (EC2).
- Port 5432 is taken on the dev box by a local PostgreSQL, hence 5433/5434.
- `make up | down | reset | status | psql-primary | psql-standby | build`.

## Rust application (`src/`)

Same crates and style as `../deadpool` (`deadpool-postgres 0.14`, `tokio-postgres 0.7`,
tokio multi-thread), plus `clap`, `hdrhistogram`, `humantime`.

- `cli.rs`: all options (`--mode sync|waitfor`, `--primary/--standby` or
  `PRIMARY_DSN/STANDBY_DSN`, `--tasks`, `--primary-pool`, `--standby-pool`, `--duration`,
  `--txns`, `--wait-mode`, `--wait-timeout`, `--synchronous-commit`, `--verify`,
  `--fetch-lsn`, `--payload-size`, `--truncate`, `--report-interval`, `--output`,
  `--label`, `--threads`, `--pool-sample-interval` (default 10ms)).
- `workload.rs`: per-txn flow. `primary_step`: acquire → autocommit
  `INSERT ... RETURNING id` → optional `SELECT pg_current_wal_insert_lsn()::text` →
  connection released. `standby_step`: acquire → `WAIT FOR LSN ... NO_THROW` (waitfor) →
  optional verify `SELECT 1 FROM wfl_test WHERE id=$1`. `parse_lsn` / `format_lsn` /
  `normalize_insert_lsn` handle the page-boundary fix (page and segment size are read from the
  primary at startup). A txn counts as *completed* only
  on commit (sync) or `success` (waitfor). Also has `is_valid_lsn`, `wait_sql` (unit
  tested) and `pg_err`/`pool_err`, which give SQLSTATE and message instead of
  tokio-postgres' bare "db error".
- `metrics.rs`: per-task HDR histograms (µs) for total, primary_acquire, commit, lsn_fetch,
  standby_acquire, wait_for_lsn and verify, plus status/verify/error counters. Merged at
  the end. The live atomics are only for progress lines. `PoolUsage`/`UsageAcc` hold pool
  usage samples: `in_use = size - available` (connections checked out) and `waiting`
  (tasks blocked in `pool.get()`), as now/avg/max, both per report interval (reset by the
  reporter) and for the whole run.
- `main.rs`: builds pools (Fast recycling, 30s wait timeout, 10s create timeout); checks
  the primary isn't in recovery and the standby is; creates the table; does a startup
  WAIT FOR so the schema is on the standby; warms both pools; prints the config
  (server version, effective `synchronous_commit`, `wal_writer_delay`); runs the tasks;
  prints the summary table; optionally appends a CSV line. A `pool_sampler` task polls
  `Pool::status()` of both pools every `--pool-sample-interval`. The progress reporter uses
  its own monitoring connection to show TPS and `pg_stat_replication` lag, plus one line
  per pool (in_use now/max_size, avg, max; waiting now/avg/max). The summary has a
  "pool usage" table (avg/max in use, avg utilisation %, avg/max waiting). CSV columns:
  `{primary,standby}_{inuse_avg,inuse_max,waiting_avg,waiting_max}`. Ctrl-C stops
  cleanly and still reports.

## Test matrix (`scripts/run_matrix.sh`)

Implemented: for each `WWD` value (`wal_writer_delay`, set at runtime) × `TASKS` value:
T1 (`--fetch-lsn`, only with the first WWD) and T2 for each `WAIT_MODES`. The primary
pool is `min(tasks, PRIMARY_POOL_MAX=32)` and the standby pool is `tasks`. Everything is
appended to one CSV in `results/`. Env: `DURATION TASKS WWD WAIT_MODES PRIMARY_POOL_MAX OUT BIN EXTRA`.

Still manual / to add later:
- Standby pool sizing sweep (find when `standby_acquire` / standby `waiting` dominates;
  the pool usage columns in the CSV show this directly).
- Network latency (`tc netem`), which needs `NET_ADMIN` and `iproute2` in the container, or run on the host.
- Failure scenarios during a run (stop standby, long query on standby, `pg_promote()`).
- Extra baselines with real sync replication: `synchronous_standby_names='standby1'`
  + `--synchronous-commit remote_write|on|remote_apply` in `--mode sync`.
- `--verify` with `standby_flush`/`standby_write` (the row may legitimately be missing,
  which is interesting to quantify).

## EC2 test plan

- Three instances in one VPC: **primary**, **standby**, **app** (so the client doesn't
  steal CPU from the DBs). Same AZ first, then cross-AZ for realistic network latency.
- DB hosts: data on a dedicated EBS gp3/io2 volume (point Docker's data root or the
  named volumes at it) so fsync costs are real. Security group: 5433 (primary, also used
  for replication from the standby) and 5434 open to the VPC.
- Primary host: `docker compose up -d --wait pg19-primary`.
- Standby host: `.env` `PRIMARY_HOST=<primary private IP>`, `PRIMARY_REPL_PORT=5433`;
  `docker compose up -d --no-deps --wait pg19-standby`.
- App host: Rust toolchain + `postgresql-client` (for `set_wal_writer_delay.sh`), export
  `PRIMARY_DSN`/`STANDBY_DSN`, `cargo build --release`, `scripts/run_matrix.sh "<label>"`.
- Record in results: instance types, EBS type/IOPS, AZ layout, PG image tag, and the CSV.

## Layout

```
waitforlsn/
├── CLAUDE.md, README.md
├── PARAMETERS.md       # user reference: every option, output field, CSV column, script variable
├── DESIGN.md           # overall flow, architecture, decisions D1–D13 (also the rustdoc front page)
├── repro/              # pgsql-hackers report (reproducer, write-up, email draft) +
│                       # REPRODUCE_WITH_WAITFORLSN.md (reproduce with the Rust program)
├── .env, .gitignore, Makefile, docker-compose.yml
├── docker/primary-init.sh, docker/standby-entrypoint.sh
├── sql/schema.sql
├── scripts/run_matrix.sh, scripts/set_wal_writer_delay.sh
├── Cargo.toml, Cargo.lock
├── src/main.rs, src/cli.rs, src/workload.rs, src/metrics.rs
└── results/            # CSV output (*.csv gitignored)
```

## Conventions

- Match the style of `../deadpool/src/main.rs`: explicit, well-commented, beginner-readable Rust.
- Every item gets a `///` doc comment (enforced by `make lint`). Explain the *why*, and
  reference the decision ID (`D2`, …) when code applies a decision from DESIGN.md. A new
  cross-cutting decision gets a new D-number in DESIGN.md §4.
- Run `make lint doc test` before considering a change done.
- Keep this CLAUDE.md updated (Status, Findings) whenever behaviour or results change,
  and PARAMETERS.md whenever options, output or CSV columns change.
- Don't hardcode credentials beyond the dev defaults in `.env`. Don't commit `target/`.
- When recording results, include: PG image tag, `wal_writer_delay`, pool sizes, task
  count, duration, wait mode, and host description (the CSV has most of these).
