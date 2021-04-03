\set nbranches :scale
\set ntellers 10 * :scale
\set naccounts 100000 * :scale
\set aid random(1,100000 * :scale)
\set bid random(1, 1 * :scale)
\set tid random(1, 10 * :scale)
\set delta random(-5000, 5000)
BEGIN;
--INSERT INTO pgbench_accounts (aid, abalance, created_on) SELECT :aid, abalance + :delta, now() FROM pgbench_accounts WHERE aid = :aid ORDER BY created_on DESC LIMIT 1;
--SELECT abalance FROM pgbench_accounts WHERE aid = :aid;
INSERT INTO pgbench_accounts (  aid, bid, abalance, created_on) VALUES (:aid, :bid, :delta, CURRENT_TIMESTAMP);
--INSERT INTO pgbench_history (tid, bid, aid, delta, mtime) VALUES (:tid, :bid, :aid, :delta, CURRENT_TIMESTAMP);
END;
