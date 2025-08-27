#!/bin/bash
# This script checks if a PostgreSQL server is healthy.
# It's designed to be used with xinetd for HAProxy health checks.

# Use modern command substitution $(...) and quote variables.
# The `sed` command trims any leading/trailing whitespace from psql's output.
VALUE=$(/usr/pgsql-16/bin/psql -t -h localhost -U postgres -p 5432 -c "select pg_is_in_recovery()" 2> /dev/null | sed -e 's/^[ \t]*//')

# Use a case statement for cleaner logic and to handle different outcomes.
case "$VALUE" in
  "t")
    # Standby server.
    # Use a single printf for an atomic and correct HTTP response.
    printf "HTTP/1.1 206 OK\r\nContent-Type: text/plain\r\nContent-Length: 7\r\nConnection: close\r\n\r\nStandby"
    ;;
  "f")
    # Primary server.
    printf "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 7\r\nConnection: close\r\n\r\nprimary"
    ;;
  *)
    # Database is down or query failed.
    printf "HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\nContent-Length: 7\r\nConnection: close\r\n\r\nDB Down"
    ;;
esac

# We still keep this to gracefully handle the connection closure.
# After seeing "Connection: close", the client will initiate the close,
# which will unblock this 'cat' command.
cat > /dev/null

exit 0
