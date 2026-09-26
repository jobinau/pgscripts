# `waitforlsn` parameter reference

This page covers every parameter of the `waitforlsn` program and of the helper scripts in
`scripts/`. For the idea behind the tests, see [CLAUDE.md](CLAUDE.md). For a quick start,
see [README.md](README.md).

```
target/release/waitforlsn [OPTIONS]
```

- All options are optional. Running with no options does a 30-second T2 (`waitfor`) run
  against the local Docker containers.
- **Durations** always need a unit: `500ms`, `30s`, `2m`, `1h`, `1m30s`, `"1m 30s"`. A
  bare number such as `90` is rejected.
- On/off options (`--verify`, `--fetch-lsn`, `--truncate`) take no value. Give the option
  to turn it on.

## How a run works (so the parameters make sense)

1. Two connection pools are created: one to the **primary** and one to the **standby**.
   The standby pool is only created when the run needs it (`--mode waitfor` or `--verify`).
2. Startup checks: the primary must *not* be in recovery and the standby *must* be. The
   table `wfl_test` is created if missing (and emptied with `--truncate`). A first
   `WAIT FOR LSN` confirms the table has reached the standby. Then every pool connection is
   opened up front, so connection setup isn't measured.
3. `--tasks` worker tasks start. Each one runs transactions back to back, with no pause
   between them:
   - **primary step**: take a connection from the primary pool → `INSERT ... RETURNING id`
     (autocommit, so the INSERT is the whole transaction) → optionally
     `SELECT pg_current_wal_insert_lsn()` → give the connection back.
   - **standby step** (only if the standby pool exists): take a standby connection →
     `WAIT FOR LSN '<lsn>' WITH (MODE ..., TIMEOUT ..., NO_THROW)` in `waitfor` mode →
     optionally read the row back (`--verify`) → give the connection back.
4. The run stops at `--duration`, after `--txns` transactions, or on Ctrl-C, whichever
   comes first. Transactions already in progress are finished, then the summary is printed.

A transaction counts as **completed** when:
- `sync` mode: the INSERT committed (and the verify read, if enabled, ran without error).
- `waitfor` mode: `WAIT FOR` returned `success`. A `timeout` or `not in recovery` status
  means the transaction is **not** completed. It was committed on the primary, but the
  application can't treat it as done.

Only completed transactions count towards TPS and the `total` latency.

---

## Test selection

### `--mode <sync|waitfor>`
Default: `waitfor`

Which test to run.

| Value | Test | Primary session setting | When a transaction is complete |
|-------|------|-------------------------|--------------------------------|
| `sync` | T1, baseline | `synchronous_commit = on` | when `COMMIT` returns on the primary |
| `waitfor` | T2 | `synchronous_commit = off` | when `WAIT FOR LSN` returns `success` on the standby |

With no synchronous standby configured (`synchronous_standby_names = ''`, as in the
Docker setup), `on` means "WAL flushed to the primary's local disk". The primary's
`synchronous_commit` can be overridden with `--synchronous-commit`.

### `--synchronous-commit <value>`
Default: not set (`on` for `sync`, `off` for `waitfor`)

Forces the `synchronous_commit` value used by every primary session, in either mode.
It accepts any value PostgreSQL does: `on`, `off`, `local`, `remote_write`, `remote_apply`.
The value is set when each connection starts (libpq `options=-c synchronous_commit=...`),
and the run header prints the effective value read back from the server.

Use it for extra baselines, e.g. real synchronous replication:
```bash
# first on the primary: ALTER SYSTEM SET synchronous_standby_names = 'standby1'; SELECT pg_reload_conf();
waitforlsn --mode sync --synchronous-commit remote_apply
```
Note: `remote_write`, `remote_apply` (and `on`, for replication) only differ from `local`
if `synchronous_standby_names` is set on the primary.

---

## Connections

### `--primary <connection string>`
Environment variable: `PRIMARY_DSN`
Default: `host=localhost port=5433 user=postgres password=postgres dbname=postgres`

How to connect to the primary. Use libpq key/value format (as above) or a URL
(`postgresql://user:pass@host:5433/postgres`). The command-line option takes precedence
over the environment variable.

The program adds these settings unless you give them yourself:
- `application_name=wfl_primary` (shows in `pg_stat_activity`)
- `connect_timeout=5`
- `options=-c synchronous_commit=<value>`. This is appended to any `options` you pass.

The progress reporter also opens one extra monitoring connection to the primary (not
from the pool) to read replication lag from `pg_stat_replication`.

The run stops with an error if this server is in recovery (i.e. it's a standby).

### `--standby <connection string>`
Environment variable: `STANDBY_DSN`
Default: `host=localhost port=5434 user=postgres password=postgres dbname=postgres`

How to connect to the standby. Same format and precedence rules as `--primary`.
`application_name=wfl_standby` and `connect_timeout=5` are added if missing.

Only used with `--mode waitfor` or `--verify`. Otherwise no standby connection is made.
The run stops with an error if this server is *not* in recovery.

---

## Load and pool sizing

### `--tasks <n>`
Default: `32`

How many concurrent worker tasks (simulated application clients) to run. Each task runs
one transaction at a time, back to back, with no think time. So this is the concurrency
level of the test. Raise it to find the maximum throughput. Use `1` to measure pure
per-transaction latency with no queuing.

Tasks aren't OS threads. They are lightweight tokio tasks spread over `--threads` threads.

### `--primary-pool <n>`
Default: `16`

Maximum number of connections in the primary pool. All of them are opened at startup.
When all are in use, further tasks queue inside the pool. That shows as `waiting` in
the pool usage output and as `primary_acquire` latency. The primary's `max_connections`
must allow this many connections (plus one for monitoring).

In `waitfor` mode a task only holds the primary connection for the INSERT and the LSN
fetch, not during the wait. So the primary pool can usually be much smaller than `--tasks`.

### `--standby-pool <n>`
Default: `32`

Maximum number of connections in the standby pool. All are opened at startup. Ignored
when the standby isn't used.

In `waitfor` mode each task **holds a standby connection for the whole `WAIT FOR`**. So
the number of connections needed is roughly `tasks × (wait time / total transaction time)`.
It grows with replication delay and `wal_writer_delay`. If this pool is too small, tasks
queue for standby connections. Watch standby `waiting` and `standby_acquire`.
Setting it equal to `--tasks` means the pool never limits the test.

### `--threads <n>`
Default: number of CPUs

Number of tokio worker threads (OS threads) that run the tasks. Normally leave it alone.
Lower it to check whether the client machine itself is the bottleneck, or to share a
machine with the databases.

---

## Run length

### `--duration <duration>`
Default: `30s`, unless `--txns` is given (then no time limit)

How long to run. No new transactions start after this time. Those already in progress
are finished, so the reported elapsed time can be slightly longer.

### `--txns <n>`
Default: not set

Stop after this many transactions **started** in total, across all tasks. Failed attempts
(errors, `WAIT FOR` timeouts) count too, so `completed` can be lower than `n`. If you give
both `--duration` and `--txns`, the run stops at whichever limit is reached first.

Ctrl-C stops a run early at any time. The summary and CSV line are still written.

---

## WAIT FOR options (`--mode waitfor` only)

### `--wait-mode <standby_replay|standby_flush|standby_write>`
Default: `standby_replay`

The `MODE` passed to `WAIT FOR LSN`, i.e. which point on the standby the application
waits for:

| Value | Returns `success` when the commit is... | Guarantees |
|-------|------------------------------------------|------------|
| `standby_write` | written to the standby's WAL files (not yet fsynced) | survives a standby *PostgreSQL* crash, not an OS crash |
| `standby_flush` | flushed (fsynced) to disk on the standby | durable on the standby |
| `standby_replay` | replayed on the standby | **visible** to queries on the standby (read your writes) |

`standby_write` is the fastest and `standby_replay` the slowest. Only `standby_replay`
guarantees that `--verify` finds the row. (`primary_flush` isn't offered because it
runs on the primary and isn't relevant here.)

### `--wait-timeout <duration>`
Default: `5s`

The `TIMEOUT` passed to `WAIT FOR LSN`: how long the standby may wait for the LSN. If the
time runs out, the server returns the status `timeout` (the program always uses
`NO_THROW`, so this isn't an error). The transaction is counted as **not completed**.
It is committed on the primary but not confirmed. The value is rounded down to whole
milliseconds.

**Warning:** `0s` (or anything under 1ms) means **wait forever** in PostgreSQL. If the
standby stops replaying, tasks hang, and Ctrl-C won't finish until they return.
Always use a positive timeout.

---

## Workload

### `--verify`
Default: off

After a transaction is complete, read the inserted row back on the standby
(`SELECT 1 FROM wfl_test WHERE id = $1`, on the same standby connection) and count whether
it was found. The result appears as `verify (row visible): ok=… missing=…`.

- In `waitfor` mode the check only runs after `success`. With `standby_replay`, `missing`
  should always be 0. With `standby_flush`/`standby_write` some misses are expected (the
  data is on the standby but not yet replayed).
- In `sync` mode there is no wait, so this measures how often an application reading from
  the standby right after committing would **not** see its own write. This option makes
  `sync` mode use the standby pool too.

The verify query's time is included in the `total` latency and shown separately as `verify`.

### `--fetch-lsn`
Default: off (always on in `waitfor` mode)

In `sync` mode, also run `SELECT pg_current_wal_insert_lsn()` after each commit, although
it isn't needed. This gives `sync` the same number of round trips to the primary as
`waitfor`, so a comparison shows the cost of the *wait* itself, not of the extra query.
`scripts/run_matrix.sh` always uses it for T1. It has no effect in `waitfor` mode.

### `--raw-insert-lsn`
Default: off

Wait for the value of `pg_current_wal_insert_lsn()` exactly as returned, without the
page-boundary correction (DESIGN.md D2). **Only for demonstrating the upstream problem**, or
for checking whether a PostgreSQL fix makes the correction unnecessary. With it, some waits
stall until `--wait-timeout` (or until unrelated WAL is written). See
`repro/PGSQL_HACKERS_REPORT.md`.

### `--payload-size <bytes>`
Default: `100`

Size of the text value inserted per transaction. Larger values generate more WAL per
transaction, which puts more load on WAL writing, replication and fsync.

### `--truncate`
Default: off

Empty the `wfl_test` table (`TRUNCATE`) on the primary before the run. The table isn't
otherwise cleaned, so it keeps growing across runs. Use this on the first run of a series,
or for runs that should start from the same table size.

---

## Output

### `--report-interval <duration>`
Default: `5s`

How often to print a progress line while the test runs. `0s` turns progress lines off
(and skips the monitoring connection). Each report shows:

```
[   5.0s] tps=  1528.3 completed=     7641 failed(+0)=0 | standby1 lag=8688 bytes replay_lag=00:00:00.003338
          pool primary  in_use now   3/8   avg    4.4 max   8 | waiting now   0 avg    3.9 max  23
          pool standby  in_use now  24/32  avg   23.6 max  32 | waiting now   0 avg    0.0 max   0
```

- `tps`: completed transactions per second since the previous report.
- `completed`: total completed so far.
- `failed(+n)=m`: errors plus non-`success` wait results, as new since the last report and total.
- After `|`: for each standby in `pg_stat_replication`, how far its replay is behind the
  primary's current WAL position, and PostgreSQL's `replay_lag`.
- `pool` lines: connections in use (now / pool size, average and max since the last report)
  and tasks waiting for a connection (now, average, max). A pool whose `waiting` stays
  above 0 is too small for the load.

### `--pool-sample-interval <duration>`
Default: `10ms`

How often pool usage is sampled for the averages and maximums in the pool lines, the
summary and the CSV. A shorter interval catches short spikes better. The cost is
negligible, so it rarely needs changing.

### `--output <file>`
Default: not set

Append one line with the run's results to this CSV file. The header is written when the
file doesn't exist yet or is empty. Keep one file per series of comparable runs, and start
a new file if the program is upgraded and the columns change. Columns:

| Column | Meaning |
|--------|---------|
| `ts` | Unix timestamp at the end of the run |
| `label` | value of `--label` |
| `server_version` | primary's `server_version` (e.g. `19beta4`) |
| `mode` | `sync` / `waitfor` |
| `sync_commit` | effective `synchronous_commit` of the primary sessions |
| `wal_writer_delay` | primary's `wal_writer_delay` at the start of the run |
| `wal_writer_flush_after` | primary's `wal_writer_flush_after` (`0` = WAL writer woken on every async commit) |
| `wait_mode`, `wait_timeout_ms` | `--wait-mode` (empty in sync mode), `--wait-timeout` in ms |
| `tasks`, `primary_pool`, `standby_pool` | load settings (`standby_pool` = 0 when unused) |
| `verify`, `fetch_lsn`, `payload_size` | workload settings (`fetch_lsn` is `true` in waitfor mode) |
| `elapsed_s`, `completed`, `tps` | run time, completed transactions, completed per second |
| `errors` | errors of any step (connection, SQL) |
| `wait_timeout`, `wait_not_in_recovery` | `WAIT FOR` results other than `success` |
| `verify_missing` | `--verify` reads that didn't find the row |
| `total_mean_ms`, `total_p50_ms`, `total_p95_ms`, `total_p99_ms`, `total_max_ms` | latency of completed transactions, as the application sees it |
| `commit_p50_ms`, `commit_p99_ms` | INSERT/commit round trip on the primary |
| `lsn_fetch_p50_ms` | LSN query after commit (0 if not fetched) |
| `wait_p50_ms`, `wait_p95_ms`, `wait_p99_ms` | `WAIT FOR LSN` round trip |
| `primary_acquire_p99_ms`, `standby_acquire_p99_ms` | time waiting for a pool connection |
| `primary_inuse_avg`, `primary_inuse_max` | primary pool connections in use, whole run |
| `primary_waiting_avg`, `primary_waiting_max` | tasks waiting for a primary connection |
| `standby_inuse_avg` … `standby_waiting_max` | same for the standby pool (empty when unused) |

### `--label <text>`
Default: empty

Free text stored in the CSV `label` column, to tell runs apart later, e.g.
`--label "ec2 r6i.xlarge gp3 same-AZ"`.

### `-h`, `--help` / `-V`, `--version`
Print the built-in help (`-h` short, `--help` long) or the program version.

---

## Summary printed at the end

```
== Summary (60.0s) ==
completed txns      : 91330  (1522.1 tps)
WAIT FOR status     : success=91330 timeout=0 not_in_recovery=0 other=0   (waitfor mode)
verify (row visible): ok=91330 missing=0                                  (--verify)
errors              : 0            (per step with the last message, if any)

pool usage     max_size  in_use avg  in_use max   util avg  waiting avg  waiting max
primary               8         4.2           8      52.8%          4.0           23
standby              32        23.6          32      73.8%          0.0            0

latency (ms)         count      mean       p50       p95       p99     p99.9       max
total ...
```

Latency rows (only non-empty rows are printed):

| Row | What it measures |
|-----|------------------|
| `total` | whole transaction, as the application sees it (completed transactions only) |
| `primary_acquire` | waiting for a connection from the primary pool |
| `commit` | `INSERT ... RETURNING id` round trip, including the commit |
| `lsn_fetch` | `SELECT pg_current_wal_insert_lsn()` after the commit |
| `standby_acquire` | waiting for a connection from the standby pool |
| `wait_for_lsn` | `WAIT FOR LSN` round trip (all results, including timeouts) |
| `verify` | read-back query on the standby |

---

## Helper scripts

### `scripts/set_wal_writer_delay.sh <wal_writer_delay> [wal_writer_flush_after]`
Changes the primary's WAL writer settings at runtime (`ALTER SYSTEM` + reload, no
restart) and prints the new values. PostgreSQL units apply (`10ms`, `200ms`, `1MB`, `0`).

```bash
scripts/set_wal_writer_delay.sh 10ms          # only wal_writer_delay
scripts/set_wal_writer_delay.sh 200ms 0       # also wal_writer_flush_after=0
scripts/set_wal_writer_delay.sh 200ms 1MB     # back to PostgreSQL defaults
```

These two settings mostly determine how fast `waitfor` can be. `wal_writer_flush_after=0`
wakes the WAL writer on every async commit, which gives the lowest median latency, and
`wal_writer_delay` limits the tail (DESIGN.md D2). It uses `psql` with `PRIMARY_DSN` if `psql` is installed, otherwise
`docker exec` into `pg19-primary`.

### `scripts/run_matrix.sh [label]`
Runs the whole test matrix and appends every run to one CSV. The optional argument is
used as `--label`. Settings come from environment variables:

| Variable | Default | Meaning |
|----------|---------|---------|
| `PRIMARY_DSN`, `STANDBY_DSN` | local Docker ports 5433 / 5434 | passed to every run |
| `DURATION` | `60s` | `--duration` of each run |
| `TASKS` | `1 8 32 64 128` | `--tasks` values to test |
| `WWD` | `10ms 50ms 200ms 200ms:0` | WAL writer settings, as `delay` or `delay:flush_after`. `flush_after` defaults to `1MB` (PostgreSQL's default). Each is set with `set_wal_writer_delay.sh` before its runs |
| `WAIT_MODES` | `standby_write standby_flush standby_replay` | `--wait-mode` values for T2 |
| `PRIMARY_POOL_MAX` | `32` | primary pool = `min(tasks, PRIMARY_POOL_MAX)`. Standby pool = `tasks` |
| `OUT` | `results/matrix-<timestamp>.csv` | CSV file |
| `BIN` | `target/release/waitforlsn` | program to run (built if missing) |
| `EXTRA` | empty | extra options for every run, e.g. `EXTRA="--verify --payload-size 1000"` |

For each `WWD` × `TASKS` combination it runs T1 (`--mode sync --fetch-lsn`, only with the
first `WWD` value since T1 doesn't depend on it), then T2 once per `WAIT_MODES` value. The
very first run uses `--truncate`. Total time is about
`(#TASKS × (1 + #WWD × #WAIT_MODES)) × (DURATION + 3s)`, which is about 65 minutes with the defaults.

---

## Examples

```bash
# T1 baseline, 64 clients, 2 minutes, also show how often a standby read would be stale
waitforlsn --mode sync --fetch-lsn --verify --tasks 64 --primary-pool 32 --duration 2m

# T2, durable on the standby, results to CSV
waitforlsn --mode waitfor --wait-mode standby_flush --tasks 64 --primary-pool 16 --standby-pool 64 \
           --duration 2m --output results/runs.csv --label "wwd=10ms flush"

# Pure latency: one client, fixed number of transactions
waitforlsn --mode waitfor --tasks 1 --primary-pool 1 --standby-pool 1 --txns 10000

# Remote hosts through environment variables
export PRIMARY_DSN="host=10.0.1.10 port=5433 user=postgres password=postgres dbname=postgres"
export STANDBY_DSN="host=10.0.1.11 port=5434 user=postgres password=postgres dbname=postgres"
waitforlsn --mode waitfor --duration 5m

# Real synchronous replication baseline (needs synchronous_standby_names='standby1' on the primary)
waitforlsn --mode sync --synchronous-commit remote_apply --tasks 64
```
