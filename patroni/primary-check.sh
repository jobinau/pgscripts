#!/bin/bash

PG_MONITOR_USER=haproxy
PG_MONITOR_PASS=haproxy
PG_MONITOR_DB=postgres

PG_PSQL=/usr/bin/psql

VIP=$1
VPT=$2
RIP=$3

if [ "$4" == "" ]; then
  RPT=$VPT
else
  RPT=$4
fi

STATUS=$(PGPASSWORD="$PG_MONITOR_PASS" $PG_PSQL -qtAX -c "select pg_is_in_recovery()" -h "$RIP" -p "$RPT" --dbname="$PG_MONITOR_DB" --username="$PG_MONITOR_USER")

if [ "$STATUS" == "f" ]; then
  # Master
  exit 0
else
  exit 1
fi
