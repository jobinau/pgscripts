# WAIT FOR LSN evaluation (PostgreSQL 19)

## Status (keep this section current)

- **2026-09-26**: Docker environment, Rust app and matrix scripts are implemented and
  tested locally (WSL2, single host). T1, T2, standby stop/start and promotion all work.
  First local numbers are in *Findings so far*.
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
- A promoted standby answers `not in recovery` immediately (about 1ms). The app sees a
  burst of fast "failures" and has to route away or back off itself.

## Which LSN to wait for (important)

With `synchronous_commit = off`, the commit record may **not be flushed** on the primary
when `COMMIT` returns. So `pg_current_wal_flush_lsn()` taken after commit can be *behind*
our commit record, and waiting for it could return `success` too early.

The app therefore runs `SELECT pg_current_wal_insert_lsn()` **after** `COMMIT` returns, on
the **same primary connection**. That position is at or past the end of our commit record,
so it is a safe (slightly conservative) target. It costs one extra round trip, recorded as
`lsn_fetch`. Never read the LSN in the same implicit transaction as the write: a
multi-statement `simple_query` runs as one transaction, so the LSN would come from before
the commit.

Implications:
- A physical walsender only ships WAL that the primary has already **flushed**. With async
  commit that flush is done by the WAL writer, so `wal_writer_delay` (default 200ms)
  sets the minimum T2 latency. **Confirmed locally**: see *Findings so far*.
- So a `standby_flush` success should mean the commit is on disk on **both** primary and
  standby, which would make T2 arguably *more* durable than T1 (local flush only). Check
  this against the PG19 source (`walsender.c`, `XLogSendPhysical` → `GetFlushRecPtr`)
  before stating it in the results.
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
  `--label`, `--threads`).
- `workload.rs`: per-txn flow. `primary_step`: acquire → autocommit
  `INSERT ... RETURNING id` → optional `SELECT pg_current_wal_insert_lsn()::text` →
  connection released. `standby_step`: acquire → `WAIT FOR LSN ... NO_THROW` (waitfor) →
  optional verify `SELECT 1 FROM wfl_test WHERE id=$1`. A txn counts as *completed* only
  on commit (sync) or `success` (waitfor). Also has `is_valid_lsn`, `wait_sql` (unit
  tested) and `pg_err`/`pool_err`, which give SQLSTATE and message instead of
  tokio-postgres' bare "db error".
- `metrics.rs`: per-task HDR histograms (µs) for total, primary_acquire, commit, lsn_fetch,
  standby_acquire, wait_for_lsn and verify, plus status/verify/error counters. Merged at
  the end. The live atomics are only for progress lines.
- `main.rs`: builds pools (Fast recycling, 30s wait timeout, 10s create timeout); checks
  the primary isn't in recovery and the standby is; creates the table; does a startup
  WAIT FOR so the schema is on the standby; warms both pools; prints the config
  (server version, effective `synchronous_commit`, `wal_writer_delay`); runs the tasks;
  prints the summary table; optionally appends a CSV line. The progress reporter uses its
  own monitoring connection to show TPS and `pg_stat_replication` lag. Ctrl-C stops
  cleanly and still reports.

## Test matrix (`scripts/run_matrix.sh`)

Implemented: for each `WWD` value (`wal_writer_delay`, set at runtime) × `TASKS` value:
T1 (`--fetch-lsn`, only with the first WWD) and T2 for each `WAIT_MODES`. The primary
pool is `min(tasks, PRIMARY_POOL_MAX=32)` and the standby pool is `tasks`. Everything is
appended to one CSV in `results/`. Env: `DURATION TASKS WWD WAIT_MODES PRIMARY_POOL_MAX OUT BIN EXTRA`.

Still manual / to add later:
- Standby pool sizing sweep (find when `standby_acquire` dominates).
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
- Keep this CLAUDE.md updated (Status, Findings) whenever behaviour or results change.
- Don't hardcode credentials beyond the dev defaults in `.env`. Don't commit `target/`.
- When recording results, include: PG image tag, `wal_writer_delay`, pool sizes, task
  count, duration, wait mode, and host description (the CSV has most of these).
