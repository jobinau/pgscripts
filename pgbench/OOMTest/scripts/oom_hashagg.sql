-- Hash aggregate on a unique key: one hash entry per row.
-- Allowed up to work_mem * hash_mem_multiplier (1GB * 4) before spilling.
SELECT aid, filler, count(*) FROM pgbench_accounts GROUP BY aid, filler OFFSET 100000000;
