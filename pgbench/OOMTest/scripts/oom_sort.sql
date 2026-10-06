-- Full in-memory sort of pgbench_accounts (wide rows incl. 84-byte filler).
-- With work_mem=1GB the sort never spills to disk, so every client holds
-- hundreds of MB of private memory. OFFSET past the end keeps result transfer ~0.
SELECT * FROM pgbench_accounts ORDER BY filler DESC, abalance OFFSET 100000000;
