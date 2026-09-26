#!/bin/bash
# Change wal_writer_delay on the primary at runtime (SIGHUP setting, no restart needed).
# Usage: scripts/set_wal_writer_delay.sh 10ms
# Uses psql with $PRIMARY_DSN if psql is installed, otherwise docker exec into pg19-primary.
set -euo pipefail
VAL="${1:?usage: $0 <delay, e.g. 10ms>}"
PRIMARY_DSN="${PRIMARY_DSN:-host=localhost port=5433 user=postgres password=postgres dbname=postgres}"

if command -v psql >/dev/null; then
    PSQL=(psql "$PRIMARY_DSN")
else
    PSQL=(docker exec -i pg19-primary psql -U postgres)
fi
"${PSQL[@]}" -qAt -v ON_ERROR_STOP=1 \
    -c "ALTER SYSTEM SET wal_writer_delay = '${VAL}'" \
    -c "SELECT pg_reload_conf()" >/dev/null
sleep 0.5
echo "wal_writer_delay = $("${PSQL[@]}" -qAtc 'SHOW wal_writer_delay')"
