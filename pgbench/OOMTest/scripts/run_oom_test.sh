#!/usr/bin/env bash
# Runs the memory-heavy pgbench workload against the current primary and
# collects evidence of the OOM kill into a timestamped results directory.
#   Usage: scripts/run_oom_test.sh [sql-script] [clients] [duration-seconds]
# pgbench runs inside the primary's database container as user postgres,
# against the default database (postgres) and schema (public).
set -u
NS=${NS:-pgo}
SQL=${1:-oom_sort.sql}; CLIENTS=${2:-4}; DURATION=${3:-60}
cd "$(dirname "$0")/.."
[ -f "scripts/$SQL" ] || { echo "scripts/$SQL not found"; exit 1; }
OUT=results/run-$(date +%Y%m%d-%H%M%S); mkdir -p "$OUT"

P=$(kubectl -n "$NS" get pods -l postgres-operator.crunchydata.com/role=primary -o jsonpath='{.items[0].metadata.name}')
echo "Primary: $P" | tee "$OUT/summary.txt"
RESTARTS_BEFORE=$(kubectl -n "$NS" get pod "$P" -o jsonpath='{.status.containerStatuses[?(@.name=="database")].restartCount}')
kubectl -n "$NS" exec "$P" -c database -- patronictl list > "$OUT/patronictl_before.txt" 2>&1

scripts/monitor.sh "$P" "$NS" > "$OUT/monitor.log" 2>&1 & MON=$!
sleep 3
echo "Running pgbench: $SQL, $CLIENTS clients, ${DURATION}s" | tee -a "$OUT/summary.txt"
# The SQL script is streamed over stdin, so nothing has to be copied into the pod.
# pgbench lives in the same cgroup and is killed together with Postgres:
# expect "command terminated with exit code 137".
kubectl -n "$NS" exec -i "$P" -c database -- \
  pgbench -n -f /dev/stdin -c "$CLIENTS" -j "$CLIENTS" -T "$DURATION" -P 5 \
  < "scripts/$SQL" > "$OUT/pgbench.log" 2>&1
echo "pgbench exit code: $?" | tee -a "$OUT/summary.txt"
sleep 30   # let Patroni finish recovery/promotion
kill $MON

kubectl -n "$NS" get pod "$P" -o jsonpath='{.status.containerStatuses[?(@.name=="database")].lastState}' > "$OUT/container_last_state.json"
kubectl -n "$NS" logs "$P" -c database --previous --tail=20 > "$OUT/patroni_previous.log" 2>&1
kubectl -n "$NS" logs "$P" -c database --tail=200 > "$OUT/patroni_current.log" 2>&1
kubectl -n "$NS" exec "$P" -c database -- patronictl list > "$OUT/patronictl_after.txt" 2>&1
kubectl -n "$NS" exec "$P" -c database -- patronictl history > "$OUT/patronictl_history.txt" 2>&1
kubectl -n "$NS" get pods -l postgres-operator.crunchydata.com/data=postgres -L postgres-operator.crunchydata.com/role > "$OUT/pods_after.txt"

RESTARTS_AFTER=$(kubectl -n "$NS" get pod "$P" -o jsonpath='{.status.containerStatuses[?(@.name=="database")].restartCount}')
REASON=$(kubectl -n "$NS" get pod "$P" -o jsonpath='{.status.containerStatuses[?(@.name=="database")].lastState.terminated.reason}')
NEWP=$(kubectl -n "$NS" get pods -l postgres-operator.crunchydata.com/role=primary -o jsonpath='{.items[0].metadata.name}')
{
  echo "database container restarts: $RESTARTS_BEFORE -> $RESTARTS_AFTER"
  echo "last termination reason:     ${REASON:-<none>}"
  echo "primary after test:          $NEWP"
} | tee -a "$OUT/summary.txt"
echo "Evidence saved in $OUT"
