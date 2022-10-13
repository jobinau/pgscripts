#!/bin/bash
#===============================================================================
#
#          FILE:  pg_hugepages
# 
#         USAGE:  ./pg_hugepages 
# 
#   DESCRIPTION:  Get the huge pages for PostgreSQL, compare to physical
# 
#       OPTIONS:  Any options on the command line will be passed to psql
#  REQUIREMENTS:  ---
#          BUGS:  ---
#         NOTES:  Expects 'which' utility : sudo yum install which
#        AUTHOR:  Kirk L. Roybal (), Kirk@timescale.com
#       COMPANY:  Timescale
#       VERSION:  1.0
#       CREATED:  08/27/2022 05:11:49 PM WEST
#      REVISION:  ---
#===============================================================================

function die(){

    retval=$1
    shift
    echo -e "$@"
    exit $retval
}

pmap_opts=""
pmap="$(which pmap)"

[[ $pmap == "" ]] && die 3 "Can't find pmap" || pmap="$pmap $pmap_opts" 

psql=$(which psql)
[[ $psql == "" ]] && die 4 "Can't find the psql utility."

sudo=$(which sudo)
[[ $sudo == "" ]] && die 5 "Can't find the sudo utility."

data_dir=$($psql $@ -qtAc "SELECT current_setting('data_directory');")
[[ $data_dir == "" ]] && die 1 "Can't find the data directory."

pg_huge_pages=$($psql $@ -qtAc "SELECT current_setting('huge_pages')")
pg_huge_page_size=$($psql $@ -qtAc "SELECT current_setting('huge_page_size')")

pg_pid=$($sudo head -n 1 $data_dir/postmaster.pid)
[[ ${pg_pid// /} == "" ]] && die 2 "Can't get the postmaster process id"

echo -e "Data Directory: $data_dir"
echo -e "PG huges_pages: $pg_huge_pages"
echo -e "PG huge_page_size: $pg_huge_page_size"
echo -e "Postmaster PID: $pg_pid"

pg_buf=$($sudo $pmap ${pg_pid})
[[ $pg_buf == "" ]] && {
    echo -e "$sudo $pmap ${pg_pid}"
    $sudo $pmap ${pg_pid}
    die 3 "Can't figure out the pg memory allocation"
}
pg_buf=$(echo -e "$pg_buf" | awk '/rw-s/ && /zero/ {print $2}')

echo -e "PG Buffers: $pg_buf"
grep ^Hugepagesize /proc/meminfo | sed -e 's/[[:space:]]*//g' -e 's/:/: /'
echo -en "Huge Pages Allocated: " 
ls /sys/kernel/mm/hugepages/
echo -en "Transparent Huge Pages: "
cat /sys/kernel/mm/transparent_hugepage/enabled