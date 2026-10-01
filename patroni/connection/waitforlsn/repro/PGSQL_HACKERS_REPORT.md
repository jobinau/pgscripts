# WAIT FOR LSN stalls on `pg_current_wal_insert_lsn()` at a WAL page boundary

Background and evidence for raising this on pgsql-hackers. The email itself is
`pgsql-hackers-reply.txt` (plain text, ready to send). The reproducer is `repro.sh` +
`find_boundary.sql`.

## 1. Summary

When the last WAL record ends exactly at a WAL page boundary, `pg_current_wal_insert_lsn()`
returns the boundary **plus the next page's header size**: 24 bytes, or 40 on the first page
of a segment. No WAL record can end there. `WAIT FOR LSN` compares the target with
end-of-record positions (replay end, walreceiver write/flush, primary flush). Those stop
at the boundary until the *next, unrelated* record is written. So the wait sleeps until
some other WAL happens to arrive, or until its timeout.

The PG19 documentation for `WAIT FOR` recommends exactly this function for the
`synchronous_commit = off` case, so applications following the docs are affected.

## 2. Status upstream (checked 2026-09-26): reply, don't open a new thread

- **Thread "test: avoid redundant standby catchup in 049_wait_for_lsn"** (pgsql-hackers).
  - It started as a patch by Xuneng Zhou to speed up the TAP test `049_wait_for_lsn`,
    which Alexander Korotkov (the `WAIT FOR` committer) pushed.
  - On **2026-09-19** Xuneng reported "unexpected fragility". On **2026-09-22** he posted
    the root cause (the same page-boundary behaviour) and two patches:
    - `v1-0001-Expose-the-WAL-insertion-end-position-to-SQL.patch` exposes the existing
      `GetXLogInsertEndRecPtr()` as a SQL function.
    - `v1-0002-Use-WAL-insertion-end-positions-in-WAIT-examples-.patch` updates the
      `WAIT FOR` documentation examples.
  - No committer response was visible when checked.
  - Archive: <http://www.mail-archive.com/pgsql-hackers@lists.postgresql.org/msg239688.html>
    (root cause and patches) and
    <http://www.mail-archive.com/pgsql-hackers@lists.postgresql.org/msg239501.html>.
- Related commitfest entry "Bugfixes for WAIT FOR LSN" (<https://commitfest.postgresql.org/60/6662>)
  is about deadlocks during recovery. It is committed and does not cover this issue.
- The same pitfall has already been fixed in other places:
  - `walsender.c` uses `GetXLogInsertEndRecPtr()` instead of `GetXLogInsertRecPtr()` twice.
    One comment says the latter "can return a position past the page header when the last
    record ends at a page boundary, which can never match a record end and would
    needlessly stall the subscriber's wait".
  - `d927b4bd97c` "Fix WAL flush LSN used by logical walsender during shutdown" (Fujii Masao, 2026-03-16).
  - `b1f14c96720` "Use GetXLogInsertEndRecPtr in gistGetFakeLSN" (Tomas Vondra, 2026-03-13).

**So the problem is known to one developer through a test. This report adds:**

1. **Impact on an application workload**, not just test runtime. Closed-loop clients stall
   completely, and each stall is a committed transaction that the application can't confirm.
2. **The docs angle.** `doc/src/sgml/ref/wait.sgml` recommends `pg_current_wal_insert_lsn()`,
   which argues for fixing this before 19.0 (possibly as a PG 19 open item).
3. **A standalone psql reproducer** that also shows `primary_flush` failing on a single server.
4. **A suggestion:** make `WAIT FOR` itself treat "boundary + page header" as the boundary,
   which protects existing users regardless of how they obtain the LSN.
5. **A SQL-level workaround** for users until then.

## 3. Mechanism (REL_19_STABLE `a630000c59c`)

| Where | What |
|-------|------|
| `xlogfuncs.c:335` | `pg_current_wal_insert_lsn()` calls `GetXLogInsertRecPtr()` |
| `xlog.c:9649` | `GetXLogInsertRecPtr()` → `XLogBytePosToRecPtr(CurrBytePos)`, which returns the position where the *next record would start*, i.e. after the page header at a boundary |
| `xlog.c:1935–1942` | `XLogBytePosToEndRecPtr()`: "if the position is at a page boundary, returns a pointer to the beginning of the page (ie. before page header) … used when converting a pointer to the end of a record" |
| `xlog.c:9665` | `GetXLogInsertEndRecPtr()` already exists and uses the End variant |
| `xlogwait.c:105` | `GetCurrentLSNForWaitType()`: replay → `GetXLogReplayRecPtr()` (= `lastReplayedEndRecPtr`); write/flush → walreceiver positions (floored at replay); primary_flush → `GetFlushRecPtr()`. All are end-of-record positions |
| `walsender.c:1954`, `:2900` | existing code that avoids `GetXLogInsertRecPtr()` for exactly this reason |
| `doc/src/sgml/ref/wait.sgml:378–397` | docs recommend `pg_current_wal_insert_lsn()` with `synchronous_commit = off` |

```text
         WAL page N                     │ WAL page N+1
 ... | INSERT | COMMIT (ends here) ─────┤[page header 24 B]│ next record ...
                                        ▲                  ▲
                          end-of-record │                  │ pg_current_wal_insert_lsn()
            (flush, write, replay stop) │                  │ (= where the next record starts)
                                    boundary          boundary + 24
```

Waiting for `boundary + 24` is only satisfied once a record *after* it has been written,
flushed or replayed.

## 4. Evidence

### 4.1 Application workload (waitforlsn benchmark in this repo)

PostgreSQL 19beta4 (Docker, WSL2, primary and standby on one host), asynchronous streaming.
Each client: autocommit INSERT → `SELECT pg_current_wal_insert_lsn()` →
`WAIT FOR LSN ... NO_THROW` on the standby.

- 40s runs, 4 clients, `TIMEOUT 3s`: `standby_flush` 8 timeouts / 15,722 waits,
  `standby_replay` 8 / 17,038, `standby_write` 0 / 24,207.
- Logged run: all 4 clients timed out on `0/0CAE2018` (page offset 24).
- 60s runs, `TIMEOUT 20s`, `standby_replay`, `wal_writer_flush_after=0`, `wal_writer_delay=10ms`.
  `--raw-insert-lsn` means the workaround is disabled:

  | clients | target | completed | timeouts | max wait |
  |--------:|--------|----------:|---------:|---------:|
  | 1 | raw insert LSN | 5,616 | 1 | 20.56 s |
  | 1 | corrected | 11,364 | 0 | 0.48 s |
  | 4 | raw insert LSN | 12,683 | 4 | 20.56 s |
  | 4 | corrected | 30,466 | 0 | 0.02 s |

- With the correction: 0 timeouts in about 83k waits (4 × 40s runs, `standby_flush` + `standby_replay`).

To re-run (e.g. on EC2, or on a newer 19 build to check a fix), see
`REPRODUCE_WITH_WAITFORLSN.md` for the full procedure and how to read the output:
```bash
scripts/set_wal_writer_delay.sh 10ms 0
target/release/waitforlsn --mode waitfor --tasks 4 --primary-pool 4 --standby-pool 4 \
    --duration 60s --wait-timeout 20s --raw-insert-lsn     # without the workaround
target/release/waitforlsn --mode waitfor --tasks 4 --primary-pool 4 --standby-pool 4 \
    --duration 60s --wait-timeout 20s                      # with the workaround
```

### 4.2 Standalone reproducer

```bash
repro/repro.sh                                  # uses the Docker containers
PSQL_PRIMARY="psql -h p -U postgres" PSQL_STANDBY="psql -h s -U postgres" repro/repro.sh
```

1. `find_boundary.sql` (primary, one session): VACUUM + CHECKPOINT (so no background WAL
   is due), commit random-sized rows until `pg_current_wal_insert_lsn() % 8192 = 24`, show
   the last records with `pg_walinspect`, then `WAIT FOR ... MODE 'primary_flush'` in the
   same session. New sessions are avoided because they can trigger opportunistic pruning,
   which writes WAL.
2. `repro.sh`: the three standby waits run in parallel, then the script checks that the
   standby positions are still at the boundary (i.e. no background WAL interfered), and
   retries otherwise. It then shows that waiting for the boundary succeeds, and that an
   unrelated INSERT releases a waiter.

Clean run on 19beta4 (first attempt):
```text
NOTICE:  hit after 191 commits
 start_lsn  |  end_lsn   | resource_manager | record_type | record_length
 0/0EE37FD8 | 0/0EE38000 | Transaction      | COMMIT      |            34
 pg_current_wal_insert_lsn | pg_current_wal_flush_lsn | insert_lsn_page_offset
 0/0EE38018                | 0/0EE38000               | 24
--- same session: WAIT FOR the insert LSN on the primary
 timeout        (14:40:24.964 → 14:40:26.966, insert/flush unchanged)
== 2. standby: WAIT FOR the insert LSN 0/0EE38018 in each mode, in parallel (TIMEOUT 2s)
   standby_replay : timeout
   standby_flush  : timeout
   standby_write  : timeout
== 3. standby receive=0/0EE38000 replay=0/0EE38000 → clean attempt
== 4. WAIT FOR the page boundary 0/0EE38000: success (all modes)
== 5. waiter started 14:40:30.448, unrelated INSERT committed 14:40:33.480,
      waiter returned 14:40:33.482 (success)
```

Things learned while building the reproducer (useful if someone asks):
- The first attempts were "successful" because other WAL arrived: `PRUNE_ON_ACCESS`
  records from new sessions, a timed checkpoint's `CHECKPOINT_REDO`, and the bgwriter's
  running-xacts record. That's also why the problem is intermittent in real systems.
- A boundary hit takes 191–1008 commits with random row sizes.

## 5. Possible fixes (as proposed in the email)

1. **Xuneng's 0001/0002**: a SQL function returning `GetXLogInsertEndRecPtr()`, and docs
   using it. Clean, but only helps users who switch to the new function.
2. **Make `WAIT FOR` tolerant**: if `target % wal_block_size == SizeOfXLogShortPHD`
   (or `target % wal_segment_size == SizeOfXLogLongPHD`), wait for the boundary instead.
   This is equivalent for all modes, because no record ends in (boundary, boundary + header],
   and it protects every caller.
3. **At minimum for 19.0**: a caveat in `wait.sgml`, or a changed example.

Changing `pg_current_wal_insert_lsn()` itself is not proposed. It is long-established
(monitoring, lag calculations), and "where the next record starts" is a legitimate
meaning for an insert position.

## 6. Workaround for users (tested on 19beta4)

```sql
CREATE FUNCTION wal_insert_end_lsn(l pg_lsn DEFAULT pg_current_wal_insert_lsn())
RETURNS pg_lsn LANGUAGE sql STABLE AS $$
  SELECT CASE
    WHEN (l - '0/0'::pg_lsn) % pg_size_bytes(current_setting('wal_segment_size'))
         < current_setting('wal_block_size')::int                      -- first page of a segment
    THEN CASE WHEN (l - '0/0'::pg_lsn) % current_setting('wal_block_size')::int = 40 THEN l - 40 ELSE l END
    ELSE CASE WHEN (l - '0/0'::pg_lsn) % current_setting('wal_block_size')::int = 24 THEN l - 24 ELSE l END
  END
$$;
-- 0/0EE38018 → 0/0EE38000, 0/0D000028 → 0/0D000000, 0/0D000018 → unchanged, others unchanged
```

The Rust app does the same thing in `workload::normalize_insert_lsn` (DESIGN.md D2).

## 7. How to send

1. **Check the thread first** for replies after 2026-09-22 (a newer patch version, or a
   committer's decision), and adjust section 4 of the email if needed.
2. Reply **in the thread** so the message is threaded correctly. The PostgreSQL archive
   (<https://www.postgresql.org/list/pgsql-hackers/>) has a **"Resend email"** link on each
   message that sends a copy to your inbox. Reply-all to Xuneng's 2026-09-22 message from there.
3. Plain text only, no HTML. Keep the `Subject: Re: …` unchanged. Attach `repro.sh` and
   `find_boundary.sql`, or paste them inline.
4. Optional but helpful: re-run `repro.sh` on the latest 19 build (beta/RC, or built from
   `REL_19_STABLE`) and say which build you used. If Xuneng's patches apply, test them
   with the reproducer: the new function should make every mode succeed.
5. The email is signed "Jobin Augustine / Percona". Adjust the signature if you prefer.
