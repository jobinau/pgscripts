BEGIN;
CREATE TEMPORARY TABLE tt_pgbench_accounts (LIKE pgbench_accounts) ON COMMIT DROP;
INSERT INTO tt_pgbench_accounts SELECT * FROM pgbench_accounts;
COMMIT;