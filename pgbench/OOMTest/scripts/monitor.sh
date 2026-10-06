#!/usr/bin/env bash
# Samples the database container's cgroup memory and the pod state once per
# second. Usage: scripts/monitor.sh <pod-name> [namespace]
POD=${1:?pod name}; NS=${2:-pgo}
while true; do
  MEM=$(kubectl -n "$NS" exec "$POD" -c database -- \
        sh -c 'echo $(( $(cat /sys/fs/cgroup/memory.current)/1048576 ))MiB oom_kill=$(grep oom_kill /sys/fs/cgroup/memory.events | cut -d" " -f2)' 2>/dev/null || echo "exec-failed")
  ST=$(kubectl -n "$NS" get pod "$POD" -o jsonpath='{range .status.containerStatuses[?(@.name=="database")]}ready={.ready} restarts={.restartCount} last={.lastState.terminated.reason}/{.lastState.terminated.exitCode}{end}')
  ROLE=$(kubectl -n "$NS" get pod "$POD" -o jsonpath='{.metadata.labels.postgres-operator\.crunchydata\.com/role}')
  echo "$(date +%T) $POD role=$ROLE mem=$MEM $ST"
  sleep 1
done
