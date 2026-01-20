#!/bin/bash
###################################################################################
# This script iterates over a set of predefined values of checkpoint_timeout
# and performs pgbench workload and then record the WAL statistics after each run.
###################################################################################

# 1. Define the array directly using parentheses and spaces
values=(300 900 1800 3600)

# 2. Loop through the array
echo "Starting iteration..."
for i in "${values[@]}"; do
    # Perform your action here
    echo "Processing value: $i"
    psql -c "ALTER SYSTEM SET checkpoint_timeout=${i}"
    psql -c "SELECT pg_reload_conf()"
    sleep 1
    psql -c "SHOW checkpoint_timeout"
    psql -c "SELECT pg_stat_reset_shared('bgwriter')" -c "SELECT pg_stat_reset_shared('io')" -c "SELECT pg_stat_reset_shared('wal')"
    psql -c "select * from pg_stat_wal"
    (sleep 900 && psql -X -f gather.sql > "out_while${i}.tsv") &
    time pgbench -c 2 -t 1110000
    psql -X -f gather.sql > out_after${i}.tsv
done
echo "Done!"
