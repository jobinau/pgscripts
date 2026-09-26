#!/bin/bash
# Change wal_writer_delay (and optionally wal_writer_flush_after) on the primary at runtime.
# Both are SIGHUP settings, so no restart is needed.
# Usage: scripts/set_wal_writer_delay.sh <wal_writer_delay> [wal_writer_flush_after]
#   scripts/set_wal_writer_delay.sh 10ms          # only the delay
#   scripts/set_wal_writer_delay.sh 200ms 0       # flush_after=0: every async commit wakes the WAL writer
# Uses psql with $PRIMARY_DSN if psql is installed, otherwise docker exec into pg19-primary.
set -euo pipefail
VAL="${1:?usage: $0 <wal_writer_delay, e.g. 10ms> [wal_writer_flush_after, e.g. 0 or 1MB]}"
FLUSH_AFTER="${2:-}"
PRIMARY_DSN="${PRIMARY_DSN:-host=localhost port=5433 user=postgres password=postgres dbname=postgres}"

if command -v psql >/dev/null; then
    PSQL=(psql "$PRIMARY_DSN")
else
    PSQL=(docker exec -i pg19-primary psql -U postgres)
fi
SQL=(-c "ALTER SYSTEM SET wal_writer_delay = '${VAL}'")
if [ -n "$FLUSH_AFTER" ]; then
    SQL+=(-c "ALTER SYSTEM SET wal_writer_flush_after = '${FLUSH_AFTER}'")
fi
"${PSQL[@]}" -qAt -v ON_ERROR_STOP=1 "${SQL[@]}" -c "SELECT pg_reload_conf()" >/dev/null
sleep 0.5
echo "wal_writer_delay = $("${PSQL[@]}" -qAtc 'SHOW wal_writer_delay'), wal_writer_flush_after = $("${PSQL[@]}" -qAtc 'SHOW wal_writer_flush_after')"
