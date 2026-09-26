-- Run on the PRIMARY (psql, autocommit). Commits small transactions until
-- pg_current_wal_insert_lsn() lands exactly one short page header (24 bytes) past a
-- WAL page boundary, i.e. the last WAL record ended exactly at the boundary.
-- Prints the insert LSN, the true end of the last record and the records around it, then
-- runs WAIT FOR ... MODE 'primary_flush' on that LSN in the SAME session.
-- Everything after the hit happens in this one session on purpose: new connections can
-- trigger opportunistic pruning (PRUNE_ON_ACCESS), which writes WAL and hides the problem.
-- psql variable :timeout (e.g. -v timeout=3s) sets the WAIT FOR timeout.
\set ON_ERROR_STOP on
CREATE EXTENSION IF NOT EXISTS pg_walinspect;
DROP TABLE IF EXISTS wfl_repro;
CREATE TABLE wfl_repro (id int, pad text) WITH (autovacuum_enabled = off);
-- clean up dead catalog tuples so nothing prunes (and writes WAL) after the hit, and
-- restart the checkpoint timer so no timed checkpoint starts during the test
VACUUM;
CHECKPOINT;

DO $$
DECLARE
    blk     int := current_setting('wal_block_size')::int;
    lsn     pg_lsn;
    i       int := 0;
BEGIN
    LOOP
        -- random payload length so record ends move around the page
        INSERT INTO wfl_repro VALUES (i, repeat('x', (random() * 200)::int));
        COMMIT;
        lsn := pg_current_wal_insert_lsn();
        EXIT WHEN (lsn - '0/0'::pg_lsn) % blk = 24;
        i := i + 1;
        IF i > 200000 THEN RAISE EXCEPTION 'no page-boundary hit in % commits', i; END IF;
    END LOOP;
    RAISE NOTICE 'hit after % commits', i + 1;
END $$;

SELECT pg_current_wal_insert_lsn()                            AS insert_lsn,
       pg_current_wal_insert_lsn() - 24                       AS page_boundary,
       pg_current_wal_flush_lsn()                             AS flush_lsn,
       (pg_current_wal_insert_lsn() - '0/0'::pg_lsn) % 8192   AS insert_lsn_page_offset
\gset

\echo '--- last WAL records before the insert position (end_lsn of the last one = page boundary)'
SELECT start_lsn, end_lsn, resource_manager, record_type, record_length
FROM pg_get_wal_records_info(:'page_boundary'::pg_lsn - 256, :'page_boundary')
ORDER BY start_lsn DESC LIMIT 3;

\echo '--- insert position vs flush position on the primary'
SELECT :'insert_lsn' AS pg_current_wal_insert_lsn, :'flush_lsn' AS pg_current_wal_flush_lsn,
       :'insert_lsn_page_offset' AS insert_lsn_page_offset;

\echo '--- same session: WAIT FOR the insert LSN on the primary'
SELECT clock_timestamp()::time(3) AS wait_start \gset
WAIT FOR LSN :'insert_lsn' WITH (MODE 'primary_flush', TIMEOUT :'timeout', NO_THROW);
SELECT :'wait_start' AS wait_start, clock_timestamp()::time(3) AS wait_end,
       pg_current_wal_insert_lsn() AS insert_lsn_now, pg_current_wal_flush_lsn() AS flush_lsn_now;

\echo INSERT_LSN=:insert_lsn
\echo BOUNDARY=:page_boundary
