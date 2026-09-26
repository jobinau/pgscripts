#!/bin/bash
# Standby entrypoint: on first start clone the primary with pg_basebackup, then hand
# over to the stock docker-entrypoint.sh (which skips initdb because PGDATA is populated).
set -euo pipefail

: "${PGDATA:?PGDATA must be set}"
: "${PRIMARY_HOST:?PRIMARY_HOST must be set}"
PRIMARY_REPL_PORT="${PRIMARY_REPL_PORT:-5432}"

if [ ! -s "$PGDATA/PG_VERSION" ]; then
    echo "standby: PGDATA empty, cloning from ${PRIMARY_HOST}:${PRIMARY_REPL_PORT}"
    mkdir -p "$PGDATA"
    chown -R postgres:postgres "$(dirname "$PGDATA")"
    chmod 700 "$PGDATA"

    until gosu postgres pg_isready -q -h "$PRIMARY_HOST" -p "$PRIMARY_REPL_PORT"; do
        echo "standby: waiting for primary..."
        sleep 1
    done

    gosu postgres env PGPASSWORD="$REPL_PASSWORD" pg_basebackup \
        -h "$PRIMARY_HOST" -p "$PRIMARY_REPL_PORT" -U replicator \
        -D "$PGDATA" -R -X stream -S standby1_slot -c fast -P

    # -R already wrote standby.signal + primary_conninfo; override primary_conninfo so it
    # carries an application_name (shows up in pg_stat_replication on the primary).
    echo "primary_conninfo = 'host=${PRIMARY_HOST} port=${PRIMARY_REPL_PORT} user=replicator password=${REPL_PASSWORD} application_name=standby1'" \
        >> "$PGDATA/postgresql.auto.conf"
fi

exec docker-entrypoint.sh "$@"
