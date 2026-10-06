# OOMTest: reproducing an OOM kill of a PostgreSQL pod (Percona Operator for PostgreSQL)

This guide shows how to make the Linux OOM killer terminate the PostgreSQL
container of a Percona Operator for PostgreSQL cluster on demand. It also
shows how to collect evidence of the kill and what the operator and Patroni
do afterwards.

The test drives the PostgreSQL **primary** past its container memory limit
using pgbench with a custom SQL script.

## 1. How it works

| Ingredient | Setting used | Why |
|---|---|---|
| Container memory limit | `limits.memory: 2Gi` on the `database` container | Gives the pod a cgroup v2 `memory.max`. Without a limit the pod is *BestEffort* and can only die from node-level memory exhaustion. |
| Pod swap | `memory.swap.max = 0` (kubelet default) | Memory pressure turns into an OOM kill instead of swapping. |
| `work_mem` | `1GB` | One sort can keep ~750 MB in memory instead of spilling to temp files. |
| `hash_mem_multiplier` | `4` | One hash aggregate may use up to 4 GB before spilling. |
| Workload | 3–4 concurrent pgbench clients, each running a full in-memory sort or hash aggregate over `pgbench_accounts` (scale 50, 5M rows) | 3–4 × 750–960 MB of private backend memory > 2 GiB. |
| `memory.oom.group = 1` | Set by kubelet on cgroup v2 (Kubernetes ≥ 1.28) | When the kernel OOM-kills **any** process in the container, it kills **every** process in it (postmaster, Patroni, all backends). The container exits with code 137 and reason `OOMKilled`, and kubelet restarts it. |

`work_mem` is a *per-operation, per-backend* limit, not a global one. That is
why a few concurrent queries can use far more memory than `shared_buffers`
plus the "expected" footprint. This is the root cause of most real-world
PostgreSQL OOMs on Kubernetes.

## 2. Tested environment

| Component | Version |
|---|---|
| Kubernetes | k3s v1.30.13 (k3d, 3 nodes) on WSL2, cgroup v2 |
| Percona Operator for PostgreSQL | 2.8.2 |
| PostgreSQL | Percona Server for PostgreSQL 17.7.1 (`percona-distribution-postgresql:17.7-2`) |
| Cluster | `cluster1` in namespace `pgo`: 3 instances, 3 pgBouncer, pgBackRest repo host |
| Patroni DCS settings | `ttl: 30`, `loop_wait: 10` (defaults) |

The commands below assume namespace `pgo` and cluster `cluster1`. Adjust
them if yours differ.

## 3. Prerequisites

- `kubectl` access to the cluster.
- A running `perconapgcluster` (any size; at least 2 instances if you want to see failover).
- About 1 GB of free space on the data volume for the pgbench data.
- Run all commands from this directory.

```bash
NS=pgo
CLUSTER=cluster1
primary() { kubectl -n $NS get pods -l postgres-operator.crunchydata.com/role=primary -o jsonpath='{.items[0].metadata.name}'; }
```

> **Label note:** operator 2.x uses `postgres-operator.crunchydata.com/role=primary`.
> The older `role=master` selector returns nothing.

## 4. Step-by-step reproduction

### Step 0: Verify connectivity

```bash
kubectl -n $NS get pods
kubectl -n $NS get perconapgcluster $CLUSTER
echo "Primary: $(primary)"

# SQL check from inside the primary
kubectl -n $NS exec $(primary) -c database -- psql -Atc "select version(), pg_is_in_recovery()"
```

To connect from your workstation, port-forward **to the pod**.
`svc/cluster1-ha` has no selector (Patroni manages its endpoints), so
`kubectl port-forward svc/cluster1-ha` fails.

```bash
kubectl -n $NS port-forward pod/$(primary) 15432:5432 &
PGPASSWORD=$(kubectl -n $NS get secret $CLUSTER-pguser-$CLUSTER -o jsonpath='{.data.password}' | base64 -d) \
  psql -h 127.0.0.1 -p 15432 -U $CLUSTER -d $CLUSTER -c 'select 1'
```

### Step 1: Put a memory limit on the PostgreSQL container

```bash
kubectl -n $NS patch perconapgcluster $CLUSTER --type=json -p='[
  {"op":"add","path":"/spec/instances/0/resources",
   "value":{"requests":{"memory":"2Gi"},"limits":{"memory":"2Gi"}}}]'
```

The operator does a rolling restart of all instance pods, replicas first and
then the primary through a switchover. It takes about 2–3 minutes. Watch it
with:

```bash
kubectl -n $NS get pods -l postgres-operator.crunchydata.com/data=postgres -L postgres-operator.crunchydata.com/role -w
```

Check the limit on the (possibly new) primary:

```bash
kubectl -n $NS exec $(primary) -c database -- sh -c '
  echo memory.max=$(cat /sys/fs/cgroup/memory.max)
  echo memory.swap.max=$(cat /sys/fs/cgroup/memory.swap.max)
  echo memory.oom.group=$(cat /sys/fs/cgroup/memory.oom.group)
  cat /sys/fs/cgroup/memory.events'
```

Expected: `memory.max=2147483648`, `memory.swap.max=0`, `memory.oom.group=1`, `oom_kill 0`.

### Step 2: Let PostgreSQL overcommit memory

```bash
kubectl -n $NS patch perconapgcluster $CLUSTER --type=merge --patch-file manifests/patch-pg-params.yaml
```

All parameters in the patch are reloadable, so no restart is needed. Patroni
applies them within about 10–30 s. Check them:

```bash
kubectl -n $NS exec $(primary) -c database -- psql -c \
 "select name, setting, unit, pending_restart from pg_settings
   where name in ('work_mem','hash_mem_multiplier','maintenance_work_mem','max_parallel_workers_per_gather','shared_buffers')"
```

### Step 3: Start a separate pgbench client pod

The client runs in its own pod so that pgbench's memory is not charged to the
database container.

```bash
kubectl -n $NS create configmap oomtest-scripts --from-file=scripts/ --dry-run=client -o yaml | kubectl apply -f -
kubectl apply -f manifests/pgbench-client.yaml
kubectl -n $NS wait --for=condition=Ready pod/pgbench-client --timeout=120s
```

The pod connects to `cluster1-primary.pgo.svc` with the
`cluster1-pguser-cluster1` credentials. It always follows the current primary.

### Step 4: Load test data

```bash
kubectl -n $NS exec pgbench-client -- pgbench -i -s 50 -q     # ~640 MB table, ~25 s
```

### Step 5 (optional): Measure per-query memory

```bash
kubectl -n $NS exec pgbench-client -- psql \
  -c "explain (analyze, costs off, timing off) SELECT * FROM pgbench_accounts ORDER BY filler DESC, abalance OFFSET 100000000" \
  -c "explain (analyze, costs off, timing off) SELECT aid, filler, count(*) FROM pgbench_accounts GROUP BY aid, filler OFFSET 100000000"
```

Observed:

```
Sort Method: quicksort  Memory: 760701kB              -> ~743 MB per sort backend
HashAggregate ... Batches: 1  Memory Usage: 983057kB  -> ~960 MB per hashagg backend
```

With a baseline of about 0.4–1.3 GiB in the cgroup (much of it reclaimable
page cache), 3 concurrent hash aggregates or 4 concurrent sorts go over 2 GiB.

### Step 6: Trigger the OOM

Automated (recommended). This captures all the evidence into `results/run-<timestamp>/`:

```bash
scripts/run_oom_test.sh oom_sort.sql 4 60       # or: scripts/run_oom_test.sh oom_hashagg.sql 3 60
```

Manual equivalent, in two terminals:

```bash
# terminal 1: watch memory and container state once per second
scripts/monitor.sh $(primary)

# terminal 2: workload
kubectl -n $NS exec pgbench-client -- pgbench -n -f /scripts/oom_sort.sql -c 4 -j 4 -T 60 -P 5
```

The OOM happens within 5–10 seconds. pgbench reports:

```
pgbench: error: client 2 aborted in command 0 (SQL) of script 0; perhaps the backend died while processing
...
number of transactions actually processed: 0
```

### Step 7: Confirm it was an OOM kill

| Where | Command | What you see |
|---|---|---|
| Pod status | `kubectl -n $NS get pod <pod>` | `RESTARTS 1 (Xs ago)` |
| Container last state | `kubectl -n $NS get pod <pod> -o jsonpath='{.status.containerStatuses[?(@.name=="database")].lastState}'` | `"reason":"OOMKilled","exitCode":137` |
| `kubectl describe pod` | `Last State: Terminated` / `Reason: OOMKilled` / `Exit Code: 137` | |
| Kernel log on the node | `dmesg \| grep -i "memory cgroup out of memory"` | `Memory cgroup out of memory: Killed process 30223 (postgres) ... anon-rss:431556kB ... oom_score_adj:869` |
| Patroni log of the killed container | `kubectl -n $NS logs <pod> -c database --previous` | Ends abruptly; no shutdown messages |
| Patroni log after restart | `kubectl -n $NS logs <pod> -c database` | `doing crash recovery in a single user mode`, followed by either `promoted self to leader because I had the session lock` or `running pg_rewind from <new leader>` |
| Cluster topology | `kubectl -n $NS exec <pod> -c database -- patronictl list` / `patronictl history` | A new timeline (TL) in both cases |

> `kubectl get events` does **not** show an `OOMKilled` event. The reason is
> recorded only in the container status. The replicas, however, log
> `Readiness probe failed` events while the primary is down.

## 5. Observed results

The same mechanism gave two different HA outcomes. Which one you get depends
on how fast the container comes back compared with the Patroni leader-lock
TTL (30 s).

### Run 1: restart in place, no failover (`oom_sort.sql`, 4 clients)

All times are UTC. Evidence: `results/run-20261006-113554-manual/`.

| Time | Event |
|---|---|
| 06:05:58 | pgbench starts; cgroup memory 1.08 GiB → 1.75 GiB within 2 s |
| 06:06:01 | Kernel cgroup OOM; whole container killed (`OOMKilled`, exit 137) |
| 06:06:04 | kubelet restarts the `database` container immediately (first restart, no back-off) |
| 06:06:04–09 | Patroni still holds the leader lock and runs **crash recovery in single-user mode** |
| 06:06:09 | Postgres starts read-only ("starting as readonly because i had the session lock") |
| 06:06:11 | `promoted self to leader because I had the session lock`; timeline 2 → 3 |
| | Replicas follow timeline 3. **Total write outage ≈ 10 s, same primary.** |

### Run 2: failover to a replica (`oom_hashagg.sql`, 3 clients)

Evidence: `results/run-20261006-113810/`.

| Time | Event |
|---|---|
| 06:08:16 | pgbench starts; cgroup memory reaches 2047 MiB at 06:08:18 |
| 06:08:19 | Kernel cgroup OOM, `OOMKilled` exit 137 (second restart of this container) |
| 06:08:19–42 | Container stays down for **23 s**: kubelet's CrashLoopBackOff delay (10 s on the 2nd restart) plus teardown |
| 06:08:42.5 | The leader lock expires (TTL 30 s after the last renewal). Replica `nnjz` acquires it: `promoted self to leader by acquiring session lock`; timeline 3 → 4 |
| 06:08:42.7 | The old primary's container starts, sees `Lock owner: nnjz`, runs single-user crash recovery, then **`pg_rewind` from the new leader**, and rejoins as a streaming replica |
| | **Failover happened; primary moved to another pod.** |

### Why the outcome varies

- kubelet restart back-off doubles on every restart: 0 s, 10 s, 20 s, 40 s,
  and so on, up to 5 min. It resets only after the container has run for
  10 minutes.
- If container restart plus crash recovery finishes before the Patroni
  `ttl` (30 s) runs out, the same pod keeps the leader role. If not, a
  replica is promoted.
- Repeated OOMs within 10 minutes therefore make failover almost certain,
  and eventually the pod shows `CrashLoopBackOff`.
- Replication is asynchronous by default. With a write workload, commits that
  had not reached a replica before the kill are lost on failover, and
  `pg_rewind` discards them on the old primary.

## 6. Tuning the reproduction

| Want | Change |
|---|---|
| A faster or more certain OOM | More clients (`-c 6`), larger scale (`-s 100`), or a lower limit (`1Gi`) |
| No OOM (control run) | Revert `work_mem` (Step 9); sorts spill to `pgsql_tmp` and memory stays flat |
| To force a failover | Run the test twice within 10 min (back-off pushes restart past the TTL), or lower `patroni.dynamicConfiguration.ttl` |
| To avoid a failover | Raise `ttl` / `retry_timeout` (trade-off: slower detection of real failures) |
| Mixed OLTP + OOM | Run normal `pgbench -c 10 -T 300` in parallel to observe client errors and lost transactions |
| Kill only one backend instead of the container | Kubernetes ≥ 1.32 kubelet `singleProcessOOMKill: true` (cgroup v2) leaves `memory.oom.group=0`. The kernel then kills one backend and the postmaster performs its own crash-restart ("server process was terminated by signal 9") without a container restart. Not tested here (k3s 1.30). |

Notes:
- Raising `shared_buffers` also counts toward the cgroup once pages are
  touched, but it needs a restart and makes the result less predictable.
  `work_mem` is the cleaner knob.
- `memory.current` includes page cache. A baseline of 1.0–1.3 GiB after
  loading data is mostly reclaimable cache. The kernel reclaims it before
  OOM-killing, so the effective headroom is larger than it looks.
- While the container is down, `kubectl exec` into it fails. The monitor
  prints `exec-failed` for those samples, which marks the outage window.

## 7. Files

```
README.md                       this guide
manifests/pgbench-client.yaml   standalone pgbench client pod (mounts scripts/ via ConfigMap)
manifests/patch-pg-params.yaml  work_mem / hash_mem_multiplier etc. (Patroni dynamic config)
manifests/revert-pg-params.yaml removes the above (back to defaults)
scripts/oom_sort.sql            pgbench script: full in-memory sort (~743 MB/backend)
scripts/oom_hashagg.sql         pgbench script: in-memory hash aggregate (~960 MB/backend)
scripts/monitor.sh              1 s sampler: cgroup memory, oom_kill counter, restarts, role
scripts/run_oom_test.sh         runs workload + monitor, collects evidence into results/
results/                        evidence from the runs described in section 5
```

## 8. Cleanup / revert

```bash
kubectl -n $NS exec pgbench-client -- pgbench -i -I d     # drop pgbench tables
kubectl -n $NS delete pod pgbench-client
kubectl -n $NS delete configmap oomtest-scripts

# restore PostgreSQL defaults (reload only)
kubectl -n $NS patch perconapgcluster $CLUSTER --type=merge --patch-file manifests/revert-pg-params.yaml

# remove the memory limit (rolling restart)
kubectl -n $NS patch perconapgcluster $CLUSTER --type=json -p='[{"op":"remove","path":"/spec/instances/0/resources"}]'
```

In production, keep a memory limit, but size `work_mem`, `max_connections`,
parallel workers and `shared_buffers` so that the worst case fits under it.
Better still, route connections through pgBouncer with a bounded pool size.
