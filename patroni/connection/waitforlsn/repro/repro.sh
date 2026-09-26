#!/bin/bash
# Reproduce: WAIT FOR LSN <pg_current_wal_insert_lsn()> stalls when the last WAL record
# ends exactly at a WAL page boundary.
#
# Needs a PostgreSQL 19 primary + streaming standby on an otherwise idle cluster.
# Defaults use the waitforlsn Docker containers; override PSQL_PRIMARY / PSQL_STANDBY, e.g.
#   PSQL_PRIMARY="psql -h primary -U postgres" PSQL_STANDBY="psql -h standby -U postgres" ./repro.sh
#
# Background WAL (bgwriter's running-xacts record, every 15s after activity; checkpoints;
# opportunistic pruning) releases the waiters, which is also why the problem is
# intermittent. Each attempt therefore checks that no WAL was written while it waited,
# and the script retries (up to $ATTEMPTS times) until it gets a clean attempt.
set -euo pipefail
cd "$(dirname "$0")"
PSQL_PRIMARY="${PSQL_PRIMARY:-docker exec -i pg19-primary psql -U postgres}"
PSQL_STANDBY="${PSQL_STANDBY:-docker exec -i pg19-standby psql -U postgres}"
TIMEOUT="${TIMEOUT:-2s}"
ATTEMPTS="${ATTEMPTS:-5}"
P() { $PSQL_PRIMARY -X -v ON_ERROR_STOP=1 "$@"; }
S() { $PSQL_STANDBY -X -v ON_ERROR_STOP=1 "$@"; }

echo "== server: $(P -Atc 'SELECT version()')"

for attempt in $(seq 1 "$ATTEMPTS"); do
    echo; echo "################ attempt $attempt ################"

    echo "== 1. primary: commit until pg_current_wal_insert_lsn() is 24 bytes past a page boundary,"
    echo "      then, in the same session, WAIT FOR it with MODE 'primary_flush' (TIMEOUT $TIMEOUT)"
    OUT=$(P -q -v timeout="$TIMEOUT" < find_boundary.sql 2>&1)
    echo "$OUT" | grep -v '^INSERT_LSN=\|^BOUNDARY=\|pg_walinspect'
    INSERT_LSN=$(echo "$OUT" | sed -n 's/^INSERT_LSN=//p')
    BOUNDARY=$(echo "$OUT" | sed -n 's/^BOUNDARY=//p')

    echo "== 2. standby: WAIT FOR the insert LSN $INSERT_LSN in each mode, in parallel (TIMEOUT $TIMEOUT)"
    for mode in standby_write standby_flush standby_replay; do
        ( r=$(S -Atc "WAIT FOR LSN '$INSERT_LSN' WITH (MODE '$mode', TIMEOUT '$TIMEOUT', NO_THROW)")
          printf '   %-15s: %s\n' "$mode" "$r" ) &
    done
    wait

    echo "== 3. positions after the waits (page boundary = $BOUNDARY)"
    RECV=$(S -Atc "SELECT pg_last_wal_receive_lsn()")
    REPLAY=$(S -Atc "SELECT pg_last_wal_replay_lsn()")
    echo "   standby receive=$RECV replay=$REPLAY"

    if [ "$RECV" = "$BOUNDARY" ] && [ "$REPLAY" = "$BOUNDARY" ]; then
        echo "   clean attempt: no WAL was written after the boundary while waiting"
        break
    fi
    echo "   background WAL arrived during the waits (positions moved past the boundary); retrying"
    [ "$attempt" -eq "$ATTEMPTS" ] && { echo "no clean attempt after $ATTEMPTS tries"; exit 1; }
done

echo; echo "== 4. standby: WAIT FOR the page boundary $BOUNDARY instead: immediate success"
for mode in standby_write standby_flush standby_replay; do
    printf '   %-15s: %s\n' "$mode" "$(S -Atc "WAIT FOR LSN '$BOUNDARY' WITH (MODE '$mode', TIMEOUT '$TIMEOUT', NO_THROW)")"
done

echo; echo "== 5. a waiter on $INSERT_LSN is released only when unrelated WAL is written"
( S -Atc "SELECT 'standby waiter started ' || clock_timestamp()::time(3)" \
       -c "WAIT FOR LSN '$INSERT_LSN' WITH (MODE 'standby_replay', TIMEOUT '30s', NO_THROW)" \
       -c "SELECT 'standby waiter returned ' || clock_timestamp()::time(3)" | sed 's/^/   /' ) &
sleep 3
P -Atc "INSERT INTO wfl_repro VALUES (-1, 'unrelated')" \
     -c "SELECT '   unrelated INSERT committed on primary at ' || clock_timestamp()::time(3)" | grep -v '^INSERT'
wait
