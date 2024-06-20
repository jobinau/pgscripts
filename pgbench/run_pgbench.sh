#!/bin/bash

# LIST="$(ls SYSBENCH/*csv)"
THREADS='64 96 128 256'

for u in $THREADS
do
    for v in read_only read_write write_only
    do
        for w in intel scaleflux
        do
            echo $u $v $w
        done
    done
done

psql -c "checkpoint"
pgbench -T 20 -P 1 --protocol=prepared -f insert_only_pgbench.sql > final.log
psql -c "checkpoint"

    for i in {1..5}
    do
        psql -c "checkpoint"
        pgbench -T 20 -P 1 --protocol=prepared -f insert_only_pgbench.sql > final.log
        psql -c "checkpoint"
        sleep 1
    done
