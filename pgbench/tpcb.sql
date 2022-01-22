--TPC-B like
--pgbench -i -s 10
--create user pgbench with password 'pgbench';
--ALTER USER pgbench SET log_min_duration_statement = 0;
--ALTER SYSTEM SET log_min_duration_statement=100;
--GRANT SELECT,INSERT,UPDATE ON pgbench_accounts,pgbench_tellers,pgbench_branches,pgbench_history TO pgbench;
--ALTER SYSTEM SET log_line_prefix='%m %u[%p] ';
--SELECT pg_reload_conf();
\set aid random(1, 1000000 * :scale)
\set bid random(1, 10 * :scale)
\set tid random(1, 100 * :scale)
\set delta random(-5000, 5000)
BEGIN;
UPDATE pgbench_accounts SET abalance = abalance + :delta WHERE aid = :aid;
SELECT abalance FROM pgbench_accounts WHERE aid = :aid;
UPDATE pgbench_tellers SET tbalance = tbalance + :delta WHERE tid = :tid;
UPDATE pgbench_branches SET bbalance = bbalance + :delta WHERE bid = :bid;
INSERT INTO pgbench_history (tid, bid, aid, delta, mtime) VALUES (:tid, :bid, :aid, :delta, CURRENT_TIMESTAMP);
COMMIT;
--pgbench -f test.sql -h localhost -U pgbench postgres -n
