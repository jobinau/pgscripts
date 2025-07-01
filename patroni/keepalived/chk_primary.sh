#!/bin/bash

RET=`/usr/bin/curl -s -o /dev/null -w "%{http_code}" http://localhost:8008/primary`

if [[ $RET -eq "200" ]]
then
   exit 0
fi

exit 1
