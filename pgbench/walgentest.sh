#!/bin/bash

# Output file
LOGFILE="pgbench_dataload.log"

# Clear the log file if it exists
> "$LOGFILE"

# Loop 10 times
for i in {1..10}
do
    echo "Run $i started at $(date)" >> "$LOGFILE"

    psql -p 5432 -c "SELECT pg_stat_reset_shared('bgwriter')" -c "SELECT pg_stat_reset_shared('io')" -c "SELECT pg_stat_reset_shared('wal')"
    # Execute pgbench and append both stdout and stderr
    { time pgbench -c 8 -j 8 -T 3600 } >> "$LOGFILE" 2>&1

    echo "Run $i finished at $(date)" >> "$LOGFILE"
    echo "----------------------------------------" >> "$LOGFILE"

    psql -p 5432 -X -f gather.sql > out_after${i}.tsv
    # Sleep for 2 minutes unless it's the last run
    if [ $i -lt 10 ]; then
        sleep 120
    fi
done

