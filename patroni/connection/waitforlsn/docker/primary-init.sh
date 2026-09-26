#!/bin/bash
# Runs once, from /docker-entrypoint-initdb.d, when the primary's PGDATA is first initialised.
# Creates the replication role, allows replication connections and pre-creates the
# physical slot the standby will use (so WAL is retained even before the standby connects).
set -euo pipefail

psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" <<SQL
CREATE ROLE replicator WITH REPLICATION LOGIN PASSWORD '${REPL_PASSWORD}';
SELECT pg_create_physical_replication_slot('standby1_slot');
-- Via ALTER SYSTEM (not "-c" on the command line) so test scripts can change them at
-- runtime with ALTER SYSTEM + pg_reload_conf(); both are SIGHUP settings.
ALTER SYSTEM SET wal_writer_delay = '${WAL_WRITER_DELAY:-200ms}';
ALTER SYSTEM SET wal_writer_flush_after = '${WAL_WRITER_FLUSH_AFTER:-1MB}';
SQL

echo "host replication replicator all scram-sha-256" >> "$PGDATA/pg_hba.conf"
