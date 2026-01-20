#!/bin/bash
######################################################################
#  Run pgbench for 1,110,000 transactions with 6 clients,
# and after 45 minutes, abruptly terminate the PostgreSQL server
######################################################################

# Configuration
DB_NAME="postgres"
TIMEOUT_MINUTES=60

echo "Starting pgbench with 2 clients and 1,110,000 transactions..."

# 1. Start pgbench in the background
pgbench -c 6 -t 11100000 $DB_NAME &
PGBENCH_PID=$!

# 2. Launch a monitor process that waits 45 minutes
(
    sleep $((TIMEOUT_MINUTES * 60))
    
    echo "45 minutes reached. Locating PostgreSQL PID..."
    
    # Find the main PostgreSQL postmaster PID
    # We use head -n 1 to ensure we only get the parent process
    POSTGRES_PID=$(pgrep -f "postgres -D" | head -n 1)

    if [ -z "$POSTGRES_PID" ]; then
        # Fallback search if the data directory path isn't in the process name
        POSTGRES_PID=$(pgrep -x "postgres" | head -n 1)
    fi

    if [ ! -z "$POSTGRES_PID" ]; then
        echo "Abruptly terminating PostgreSQL (PID: $POSTGRES_PID) with kill -9..."
        kill -9 $POSTGRES_PID
        echo "PostgreSQL terminated."
    else
        echo "Error: Could not find PostgreSQL process."
    fi
) &

# Wait for pgbench to finish (or be cut off by the DB failure)
wait $PGBENCH_PID

echo "Script execution completed."
