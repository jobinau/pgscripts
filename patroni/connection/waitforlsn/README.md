# WAIT FOR LSN evaluation (PostgreSQL 19)

Compares two ways for an application to decide that a transaction is complete:

- **T1 `--mode sync`**: `synchronous_commit=on` (local WAL flush). Done when `COMMIT` returns.
- **T2 `--mode waitfor`**: `synchronous_commit=off`. Done when `WAIT FOR LSN '<commit lsn>'`
  returns `success` on the standby.

The app keeps two deadpool-postgres pools (primary and standby). See [CLAUDE.md](CLAUDE.md)
for the full idea, design notes and findings, and [PARAMETERS.md](PARAMETERS.md) for
every program option, the output format and the helper scripts' settings.

## Local environment

```bash
make up          # postgres:19beta4 primary (localhost:5433) + streaming standby (localhost:5434)
make status      # pg_stat_replication
make reset       # drop both volumes and rebuild
make build       # cargo build --release
```

## Run

```bash
# T1: sync commit (+ fetch LSN so the round trips match T2; --verify shows stale reads on the standby)
target/release/waitforlsn --mode sync --fetch-lsn --verify --tasks 32 --duration 60s

# T2: async commit + WAIT FOR on the standby
target/release/waitforlsn --mode waitfor --wait-mode standby_flush --tasks 32 --duration 60s

# Append a summary line to a CSV
target/release/waitforlsn --mode waitfor --output results/runs.csv --label "wwd=10ms"

target/release/waitforlsn --help     # all options (full reference: PARAMETERS.md)
```

Progress lines and the summary show pool usage for both pools: connections in use
(now/avg/max of `max_size`) and tasks waiting for a connection. Sampled every
`--pool-sample-interval` (default 10ms).

Connection strings come from `--primary` / `--standby` or `PRIMARY_DSN` / `STANDBY_DSN`.

`wal_writer_delay` sets the floor of T2 latency. Change it at runtime with:

```bash
scripts/set_wal_writer_delay.sh 10ms        # wal_writer_delay only
scripts/set_wal_writer_delay.sh 10ms 0      # plus wal_writer_flush_after=0 (fastest T2, see DESIGN.md D2)
```

## Source documentation

```bash
make doc         # HTML docs in target/doc/waitforlsn/index.html (front page = DESIGN.md)
make lint        # clippy; fails if any item lacks a doc comment
make test        # unit tests
```

[DESIGN.md](DESIGN.md) explains the overall flow and the reasoning behind each design decision.

## Full matrix

```bash
scripts/run_matrix.sh "my label"
# env overrides: DURATION=60s TASKS="1 8 32 64 128" WWD="10ms 50ms 200ms"
#                WAIT_MODES="standby_write standby_flush standby_replay" EXTRA="--verify" OUT=results/x.csv
```

## EC2 (three hosts: primary, standby, app)

```bash
# primary host
docker compose up -d --wait pg19-primary

# standby host (.env: PRIMARY_HOST=<primary private IP>, PRIMARY_REPL_PORT=5433)
docker compose up -d --no-deps --wait pg19-standby

# app host (needs Rust + postgresql-client for set_wal_writer_delay.sh)
export PRIMARY_DSN="host=<primary ip> port=5433 user=postgres password=postgres dbname=postgres"
export STANDBY_DSN="host=<standby ip> port=5434 user=postgres password=postgres dbname=postgres"
cargo build --release && scripts/run_matrix.sh "ec2 <instance type>"
```
