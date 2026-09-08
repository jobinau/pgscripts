#!/bin/bash
# resolve_relfilenodes.sh -- map base/<dboid>/<relfilenode> paths to relations.
#
# Reads data-directory-relative paths on stdin, groups them by database OID and
# asks the running cluster what each relfilenode is. Run it against the NEW
# (PG 18) cluster: pg_upgrade preserves relfilenodes, so the lookup works there
# and the OIDs still match the old data directory layout.
#
# Usage:
#  printf '%s\n' base/16410/17206 base/17316/17342 base/17316/17351 | ./resolve_relfilenodes.sh -p 5432

#   ./resolve_relfilenodes.sh -p 5433 < paths.txt
#   grep -oE 'base/[0-9]+/[0-9_a-z.]+' report.txt | ./resolve_relfilenodes.sh -p 5433
#  
# **Clubbing with pg_upgrade_link_check.py**
# Step1. Store the output of pg_upgrade_link_check to a CSV file:
# python3 pg_upgrade_link_check.py /home/postgres/pg16datadir/ /home/postgres/pg18datadir/ --csv /tmp/links.csv
# **Step2.  Pipe the relevant column from the CSV file to this script:**
# awk -F, '($2=="COPY" || $2=="MISSING") && $4=="" {print $3}' /tmp/links.csv | ./resolve_relfilenodes.sh -p 5432

# Any arguments are passed straight to psql (-p, -h, -U, ...).

set -euo pipefail

PSQL=(psql "$@")
tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT

while read -r path; do
    path=${path#/}
    [[ $path == base/* ]] || continue
    db=$(cut -d/ -f2 <<<"$path")
    file=$(cut -d/ -f3 <<<"$path")
    node=${file%%.*}                     # drop segment number (16384.1)
    for suffix in _fsm _vm _init; do     # drop fork suffix
        node=${node%$suffix}
    done
    [[ $db =~ ^[0-9]+$ && $node =~ ^[0-9]+$ ]] || continue
    echo "$node" >> "$tmpdir/$db"
done

shopt -s nullglob
files=("$tmpdir"/*)
if [[ ${#files[@]} -eq 0 ]]; then
    echo "no base/<dboid>/<relfilenode> paths found on stdin" >&2
    exit 1
fi

for f in "${files[@]}"; do
    db=$(basename "$f")
    nodes=$(sort -un "$f" | paste -sd,)
    dbname=$("${PSQL[@]}" -d postgres -Atqc \
        "SELECT datname FROM pg_database WHERE oid = $db" || true)

    if [[ -z $dbname ]]; then
        echo "== database OID $db: no such database in this cluster =="
        echo "   relfilenodes: $nodes"
        echo
        continue
    fi

    echo "== database $dbname (OID $db) =="
    "${PSQL[@]}" -d "$dbname" -c "
        SELECT c.relfilenode,
               CASE c.relkind
                   WHEN 'r' THEN 'table'      WHEN 'i' THEN 'index'
                   WHEN 'S' THEN 'sequence'   WHEN 't' THEN 'toast'
                   WHEN 'm' THEN 'matview'    WHEN 'p' THEN 'partitioned table'
                   ELSE c.relkind::text
               END                                        AS kind,
               n.nspname || '.' || c.relname               AS relation,
               COALESCE(pi.nspname || '.' || parent.relname, '')
                                                           AS belongs_to,
               c.reltuples::bigint                         AS reltuples,
               pg_size_pretty(pg_relation_size(c.oid))     AS size
        FROM pg_class c
        JOIN pg_namespace n  ON n.oid = c.relnamespace
        LEFT JOIN pg_index i ON i.indexrelid = c.oid
        LEFT JOIN pg_class t ON t.reltoastrelid = c.oid
        LEFT JOIN pg_class parent
               ON parent.oid = COALESCE(i.indrelid, t.oid)
        LEFT JOIN pg_namespace pi ON pi.oid = parent.relnamespace
        WHERE c.relfilenode IN ($nodes)
        ORDER BY 2, 3;"

    # relfilenodes that no longer exist in this cluster
    "${PSQL[@]}" -d "$dbname" -Atqc "
        SELECT 'not found in pg_class: ' || string_agg(x::text, ', ')
        FROM unnest(ARRAY[$nodes]) AS x
        WHERE NOT EXISTS (SELECT 1 FROM pg_class WHERE relfilenode = x)
        HAVING count(*) > 0;"
    echo
done
