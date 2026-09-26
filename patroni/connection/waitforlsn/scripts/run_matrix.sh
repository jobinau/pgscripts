#!/bin/bash
# Run the T1/T2 test matrix and append one CSV line per run.
#   scripts/run_matrix.sh "ec2 c6i.2xlarge x3"
# Tunables (env): PRIMARY_DSN STANDBY_DSN DURATION TASKS WWD WAIT_MODES PRIMARY_POOL_MAX OUT BIN EXTRA
set -euo pipefail
cd "$(dirname "$0")/.."

LABEL="${1:-}"
export PRIMARY_DSN="${PRIMARY_DSN:-host=localhost port=5433 user=postgres password=postgres dbname=postgres}"
export STANDBY_DSN="${STANDBY_DSN:-host=localhost port=5434 user=postgres password=postgres dbname=postgres}"
DURATION="${DURATION:-60s}"
TASKS="${TASKS:-1 8 32 64 128}"
WWD="${WWD:-10ms 50ms 200ms}"                       # wal_writer_delay values for T2
WAIT_MODES="${WAIT_MODES:-standby_write standby_flush standby_replay}"
PRIMARY_POOL_MAX="${PRIMARY_POOL_MAX:-32}"
OUT="${OUT:-results/matrix-$(date +%Y%m%d-%H%M%S).csv}"
BIN="${BIN:-target/release/waitforlsn}"
EXTRA="${EXTRA:-}"                                 # extra args for every run, e.g. "--verify"

mkdir -p "$(dirname "$OUT")"
[ -x "$BIN" ] || cargo build --release

run() {
    echo ">>> $*"
    # shellcheck disable=SC2086
    "$BIN" --duration "$DURATION" --report-interval 0s --output "$OUT" $EXTRA "$@" \
        | sed -n '/== Summary/,$p'
    sleep 3   # let replication / checkpoints settle between runs
}

first=1
for wwd in $WWD; do
    scripts/set_wal_writer_delay.sh "$wwd"
    for t in $TASKS; do
        pp=$(( t < PRIMARY_POOL_MAX ? t : PRIMARY_POOL_MAX ))
        sp=$t   # every waiting task holds a standby connection for the whole wait
        trunc=""
        [ $first -eq 1 ] && trunc="--truncate" && first=0

        # T1 doesn't depend on wal_writer_delay: run it only with the first value
        if [ "$wwd" = "${WWD%% *}" ]; then
            run --mode sync --fetch-lsn --tasks "$t" --primary-pool "$pp" --standby-pool "$sp" \
                --label "$LABEL" $trunc
            trunc=""
        fi
        for wm in $WAIT_MODES; do
            run --mode waitfor --wait-mode "$wm" --tasks "$t" --primary-pool "$pp" --standby-pool "$sp" \
                --label "$LABEL" $trunc
            trunc=""
        done
    done
done
echo "results: $OUT"
