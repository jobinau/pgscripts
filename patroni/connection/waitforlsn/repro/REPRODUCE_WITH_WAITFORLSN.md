# Reproducing the WAIT FOR page-boundary stall with `waitforlsn`

The Rust program normally **corrects** the target LSN, so it doesn't hit the upstream
problem described in `PGSQL_HACKERS_REPORT.md` (DESIGN.md D2). This page explains how to
bring the problem back on purpose: to show it to someone, to re-measure it (e.g. on EC2),
or to check whether a PostgreSQL fix makes the correction unnecessary.

There are two ways:

| | How | When to use |
|---|---|---|
| **A. No code edits** | run with `--raw-insert-lsn` | normally: demos, measurements, EC2 |
| **B. Code edits** | remove the correction in `src/workload.rs` | to strip the workaround for good, or to test an upstream fix with the same binary behaviour as "before the fix" |

## Quick manual reproduction (copy-paste, tested 2026-09-30)

```bash
# 0. go to the project
cd ~/pgscripts/patroni/connection/waitforlsn

# 1. start primary (:5433) + streaming standby (:5434), build the program
make up
make build

# 2. fast async-commit flushing: many commits per second, so page boundaries are hit often
./scripts/set_wal_writer_delay.sh 10ms 0

# 3. REPRODUCE: fix disabled, 4 clients, 60s, 3s WAIT FOR timeout
./target/release/waitforlsn --mode waitfor --wait-mode standby_replay \
    --tasks 4 --primary-pool 4 --standby-pool 4 \
    --duration 60s --wait-timeout 3s --report-interval 0s \
    --raw-insert-lsn

# 4. CONTROL: identical, but with the fix (the default)
./target/release/waitforlsn --mode waitfor --wait-mode standby_replay \
    --tasks 4 --primary-pool 4 --standby-pool 4 \
    --duration 60s --wait-timeout 3s --report-interval 0s

# 5. restore PostgreSQL's default WAL writer settings
./scripts/set_wal_writer_delay.sh 200ms 1MB
```

**What to look for in step 3.** It reproduced in 3 of 3 runs:
```text
[task 0] note: REPRO: insert LSN 0/185EA018 is just past a WAL page header (boundary 0/185EA000); ...
   ... the same note for the other tasks: they all wait for the same LSN ...
[task 1] wait_for_lsn error: timeout waiting for LSN 0/185EA018 (target=0/185EA018 |
   standby receive=0/185EA000 replay=0/185EA000 | primary flush=0/185EA000 ... walsender sent=standby1:0/185EA000)
   ... the same timeout for the other tasks, 3 s later ...
WAIT FOR status     : success=16361 timeout=32 ...
page-header LSNs    : 36  (waited for as-is: --raw-insert-lsn)
```
- A `note: REPRO: … just past a WAL page header` line (LSN ending in `018`), followed about 3 s
  later by `timeout waiting for LSN` for **the same LSN** on all tasks.
- In the snapshot, everything (primary flush, walsender sent, standby receive/replay) sits
  at the page boundary (`…000`). The target is 24 bytes further on, and nothing gets there.
- Summary: `timeout` > 0, and `page-header LSNs` > 0.

**Step 4 (control)** should show `timeout=0` even though `page-header LSNs` is still > 0
(they are corrected), with noticeably higher tps.

Measured on 2026-09-30 (local Docker, 19beta4):

| Run | page-header LSNs | timeouts | tps |
|---|---:|---:|---:|
| raw run 1 | 36 | 32 | 272.6 |
| raw run 2 | 4 | 8 | 454.9 |
| raw run 3 | 8 | 8 | 449.9 |
| fix active | 56 (corrected) | 0 | 507.6 |

If a raw run happens to show `timeout=0`, just run it again or use `--duration 5m`. Each
page-header LSN only stalls when every client is waiting on it at the same moment.

---

Every place in the code that matters for this is marked with a `REPRO:` comment:

```bash
grep -n REPRO src/*.rs
```

---

## 1. Background in one paragraph

After a transaction commits, the program asks the primary for
`pg_current_wal_insert_lsn()` and waits for that LSN on the standby. When the last WAL
record ended exactly at an 8 KB WAL page boundary, this function returns
**boundary + 24** (the size of the next page's header; + 40 on the first page of a
16 MB segment). No record ends there. The standby's positions stop at the boundary until
some *other* WAL arrives. If all clients are waiting at that moment, nothing else writes
WAL, and every `WAIT FOR` sleeps until its timeout.

The fix, `normalize_insert_lsn()` in `src/workload.rs`, moves such an LSN back to the
boundary.

---

## 2. Way A: `--raw-insert-lsn` (no code edits)

### 2.1 Prepare

```bash
cd waitforlsn
make up                                   # primary :5433 + standby :5434 (postgres:19beta4)
make build                                # target/release/waitforlsn
scripts/set_wal_writer_delay.sh 10ms 0    # fast async-commit flushing: many txns per second,
                                          # so page boundaries are hit often
```

### 2.2 Run: without the fix, then with it

```bash
# 1) the problem: wait for the raw insert LSN
target/release/waitforlsn --mode waitfor --wait-mode standby_replay \
    --tasks 4 --primary-pool 4 --standby-pool 4 \
    --duration 60s --wait-timeout 3s --report-interval 20s \
    --raw-insert-lsn \
    --output results/repro.csv --label "repro raw"

# 2) the same with the fix (the default)
target/release/waitforlsn --mode waitfor --wait-mode standby_replay \
    --tasks 4 --primary-pool 4 --standby-pool 4 \
    --duration 60s --wait-timeout 3s --report-interval 20s \
    --output results/repro.csv --label "repro fixed"
```

### 2.3 What you should see (19beta4, local Docker)

**Without the fix** (`--raw-insert-lsn`):

```text
LSN page-boundary fix: DISABLED (--raw-insert-lsn)

[task 3] note: REPRO: insert LSN 0/10316018 is just past a WAL page header (boundary 0/10316000); waiting for it as-is (--raw-insert-lsn)
[task 0] note: REPRO: insert LSN 0/10316018 is just past a WAL page header (boundary 0/10316000); ...
[task 2] note: REPRO: insert LSN 0/10316018 ...
[task 1] note: REPRO: insert LSN 0/10316018 ...
[task 3] wait_for_lsn error: timeout waiting for LSN 0/10316018 (...)
[task 2] wait_for_lsn error: timeout waiting for LSN 0/10316018 (...)
[task 1] wait_for_lsn error: timeout waiting for LSN 0/10316018 (...)
[task 0] wait_for_lsn error: timeout waiting for LSN 0/10316018 (...)

== Summary (60.6s) ==
completed txns      : 22090  (364.5 tps)
WAIT FOR status     : success=22090 timeout=20 not_in_recovery=0 other=0
page-header LSNs    : 24  (waited for as-is: --raw-insert-lsn)
...
wait_for_lsn         22110     8.544     4.671    16.279    18.143  1484.799  3006.463
```

How to read it:
- **`note: REPRO: insert LSN … just past a WAL page header`**: the program is about to wait
  for a page-header LSN. All four tasks got the *same* LSN, because they all read the insert
  position after the same last commit.
- **`timeout waiting for LSN 0/10316018`** follows about 3s later for the same LSN, on all
  four tasks. That is the stall: nobody was left to write WAL.
- **Summary:** `timeout=20` against `page-header LSNs: 24`. Most page-header LSNs ended
  in a timeout. A few were released because a still-running client wrote WAL in time.
- **Latency:** `wait_for_lsn` max is 3006 ms, i.e. the full `--wait-timeout`.
- Only the first 10 notes and errors are printed. The counts in the summary are complete.

**With the fix** (same command without `--raw-insert-lsn`):

```text
WAIT FOR status     : success=28537 timeout=0 not_in_recovery=0 other=0
page-header LSNs    : 28  (corrected to the page boundary)
```

The same kind of LSNs occurred (`page-header LSNs: 28`), but they were corrected, so there
are no page-header timeouts and more transactions complete. (See section 5 for a
*different*, rarer stall that can still appear with the fix.)

### 2.4 The CSV (`results/repro.csv`)

Two columns identify reproduction runs:

| Column | Meaning |
|---|---|
| `raw_insert_lsn` | `true` = fix disabled for this run |
| `boundary_lsns` | how many commit LSNs pointed just past a page header |

Compare them with `wait_timeout` and `total_max_ms`:

```bash
cut -d, -f2,20,44,45 results/repro.csv     # label, wait_timeout, raw_insert_lsn, boundary_lsns
```

### 2.5 Tuning the reproduction

| Change | Effect |
|---|---|
| `--tasks 1` | Every page-header LSN stalls (there is no other client to release it). The clearest demo, fewer events per minute |
| `--tasks 4` | A good balance: frequent events, and most of them stall all clients |
| `--tasks 32` or more | Stalls become rare: other clients keep writing WAL and release the waiters. The problem is still there, but hidden |
| `--wait-timeout 20s` | Shows how long a stall lasts on its own (in our runs, the full 20s) |
| `--wait-timeout 3s` | More stall events per run, and faster runs |
| `--wait-mode standby_write / standby_flush / standby_replay` | All three stall. `primary_flush` is not offered by the program, but it stalls too (see `repro.sh`) |
| `scripts/set_wal_writer_delay.sh 200ms 1MB` | PostgreSQL defaults: far fewer transactions per second, so fewer boundary hits per minute |
| `--duration 5m` | More events for statistics |

Afterwards, restore the defaults: `scripts/set_wal_writer_delay.sh 200ms 1MB`.

---

## 3. Way B: code edits

What the flag does can also be done in the code. These are the edits, from the most
important to optional.

### Edit 1 (required): wait for the raw LSN in `primary_step`

File `src/workload.rs`, function `primary_step`, block marked
`// ---- Page-boundary fix (DESIGN.md D2) ----`.

**Current code (fix active unless `--raw-insert-lsn`):**
```rust
        let corrected = normalize_insert_lsn(pos, ctx.wal_block_size, ctx.wal_segment_size);
        if corrected != pos {
            st.boundary_lsns += 1;
        }
        if ctx.raw_insert_lsn {
            // REPRO: wait for the raw insert LSN (the behaviour the PG19 docs suggest).
            ...
            Some(raw)
        } else {
            // Normal: wait for the corrected target, which never stalls and is never too early.
            Some(format_lsn(corrected))
        }
```

**Edited to reproduce (fix removed, counting kept so you still see the events):**
```rust
        let corrected = normalize_insert_lsn(pos, ctx.wal_block_size, ctx.wal_segment_size);
        if corrected != pos {
            st.boundary_lsns += 1;   // still counted, so timeouts can be matched to these
        }
        Some(raw)                    // REPRO: correction deliberately not used;
                                     // wait for pg_current_wal_insert_lsn() as-is
```

This is exactly what the program did before the fix, and what the PostgreSQL 19 `WAIT FOR`
documentation example does.

### Edit 2 (optional): the startup check in `src/main.rs`

In `run()`, the block after `// Make sure the schema change has reached the standby`
always corrects the LSN, even with `--raw-insert-lsn`. **Leave it corrected.** Right after
`CREATE TABLE` nothing else may write WAL, so a raw page-header LSN there would make the
program hang for the startup check's 30s before measuring anything. That demonstrates the
same bug, but in a confusing place.

If you really want the original behaviour everywhere, replace
```rust
        let lsn = workload::format_lsn(workload::normalize_insert_lsn(
            pos, info.wal_block_size, info.wal_segment_size));
```
with
```rust
        let _ = pos;
        let lsn = raw;   // REPRO: original behaviour, may stall startup for up to 30s
```

### Edit 3 (to test an upstream fix): use the new server function

If PostgreSQL adds a function that returns the end of the last inserted record (e.g. the
one from Xuneng Zhou's `v1-0001-Expose-the-WAL-insertion-end-position-to-SQL.patch`; use
the name from the committed version), change `LSN_SQL` in `src/workload.rs`:

```rust
// before
pub const LSN_SQL: &str = "SELECT pg_current_wal_insert_lsn()::text";
// after (function name as committed upstream)
pub const LSN_SQL: &str = "SELECT <new_insert_end_lsn_function>()::text";
```

Then run with `--raw-insert-lsn` (or with Edit 1). Expected with a correct upstream fix:
- `page-header LSNs : 0`, since the server never returns such LSNs any more;
- no timeouts caused by page headers;
- the same throughput as the fixed program.

If both hold, the client-side correction (`normalize_insert_lsn`, the SQL function in
`PGSQL_HACKERS_REPORT.md` §6) can be removed.

### What does not need editing

- **Unit tests** (`make test`) test `normalize_insert_lsn` directly and still pass after
  Edit 1. The function stays in the code; it's just not used.
- **CSV and summary:** `boundary_lsns` / `page-header LSNs` keep counting after Edit 1. The
  `raw_insert_lsn` column shows `false` unless you also pass the flag, so note in `--label`
  that the binary was edited.

After editing: `make test build` (not `make lint`, see below), then run the commands from
section 2.2. Without `--raw-insert-lsn` the program now behaves like the raw run.

Expected compiler warnings after Edit 1 (harmless, and the reason `make lint` fails, since it
treats warnings as errors):
```text
warning: field `raw_insert_lsn` is never read
warning: method `log_note` is never used
```
(Checked: Edit 1 compiles with exactly these two warnings.)

To undo: if the unedited state is committed, `git checkout src/workload.rs src/main.rs`.
Check `git status` first, because this also discards any other uncommitted changes in those
files. Otherwise revert the edit by hand, using the "Current code" block above.

---

## 4. Cross-check without the Rust program

`repro/repro.sh` reproduces the same problem with psql only, deterministically. It is the
reproducer attached to the pgsql-hackers email:

```bash
repro/repro.sh
```

It shows all four modes timing out, **including `primary_flush` on the primary alone**, and
that waiting for the boundary succeeds immediately.

---

## 5. A different, rarer stall that the fix does not cover (under investigation)

With the fix active, a rare timeout can still occur, e.g. all 4 tasks on `0/12C283A0`.
Those LSNs are **not** page-header LSNs (`page-header LSNs` doesn't count them, and the
offset is in the middle of a page). Each timeout message includes a snapshot of positions,
taken right after the timeout by `diagnose_timeout()` in `src/workload.rs`:

```text
timeout waiting for LSN 0/12C283A0 (target=0/12C283A0 |
   standby receive=0/12C28000 replay=0/12C27FC0 |
   primary flush=0/12C283A0 write=0/12C283A0 insert=0/12C283A0 walsender sent=...)
```

How to read the snapshot:

| Observation | Meaning |
|---|---|
| primary `flush` < target | the primary never flushed the commit: a WAL writer issue |
| `flush` ≥ target, `walsender sent` < target | flushed but not sent: the walsender wasn't woken |
| `sent` ≥ target, standby `receive` < target | sent but not received or processed by the standby |
| standby `replay` ≥ target | reached, but the `WAIT FOR` waiter wasn't woken |

In the case above the primary had flushed the target, but the standby had received only up
to the page boundary `0/12C28000`. That points at the primary → standby streaming step, not
at `WAIT FOR`. A suspected cause is a lost walsender wakeup (see CLAUDE.md, Status
2026-09-29).

**Status (2026-09-30):** seen twice in about 140k waits, then 0 times in about 246k waits (8
runs of 60s, 4 tasks, fix active), then **captured again with `walsender sent`** during a raw
run (the LSN wasn't a page-header one):

```text
timeout waiting for LSN 0/18E8F8E8 (target=0/18E8F8E8 | standby receive=0/18E8F6A8 replay=0/18E8F6A8 |
   primary flush=0/18E8F8E8 write=0/18E8F8E8 insert=0/18E8F8E8 walsender sent=standby1:0/18E8F6A8)
```

The primary had flushed the target, but **the walsender had sent only up to `…F6A8`**, and
the standby had received exactly that. So the walsender didn't send flushed WAL for 3 s. This
rules out the standby and the `WAIT FOR` wakeup, and fits the suspected lost walsender wakeup.
The position is mid-page, so it isn't the page-size send chunking. The exact race is still a
hypothesis (see CLAUDE.md). To keep hunting, repeat the "with the fix"
command from 2.2 (a longer `--duration`, e.g. `30m`, is simplest) and look for
`timeout waiting for LSN` lines whose LSN is **not** reported as a page-header LSN. Longer
runs on EC2 are the next chance to catch it.
