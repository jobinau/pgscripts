
##On All PG Nodes
sudo yum -y  install repmgr13*

## On the primary node
psql << EOF
ALTER SYSTEM SET archive_mode = on;
ALTER SYSTEM SET archive_command = '/bin/true';
ALTER SYSTEM SET shared_preload_libraries = 'repmgr';
EOF

psql << EOF
create user repmgr;
ALTER USER repmgr WITH SUPERUSER;
create database repmgr with owner repmgr;
EOF


##DONT USE : Replace the ocurrance
sed "s/^local.*all.*all.*/\
local   replication     repmgr                             trust\n\
host   replication      repmgr        127.0.0.1\/32        trust\n\
host    replication     repmgr        172.16.0.0\/16        trust\n\
local   repmgr          repmgr                             trust\n\
host    repmgr          repmgr        127.0.0.1\/32         trust\n\
host    repmgr          repmgr        172.16.0.0\/16       trust\
/" $PGDATA/pg_hba.conf

##USE THIS : Append after the first line
sed -i "/^local.*all.*all.*/a\
local   replication     repmgr                             trust\n\
host    replication     repmgr        127.0.0.1/32         trust\n\
host    replication     repmgr        172.16.0.0/16        trust\n\
local   repmgr          repmgr                             trust\n\
host    repmgr          repmgr        127.0.0.1/32         trust\n\
host    repmgr          repmgr        172.16.0.0/16       trust\
" $PGDATA/pg_hba.conf

## OLD BUT USELESS
cat << EOF >> $PGDATA/pg_hba.conf
local   replication     repmgr                                        trust
host    replication     repmgr        127.0.0.1/32            trust
host    replication     repmgr        172.16.0.0/16        trust
local   repmgr          repmgr                                       trust
host    repmgr          repmgr        127.0.0.1/32           trust
host    repmgr          repmgr        172.16.0.0/16       trust
EOF

sudo systemctl restart postgresql-$PGVER

sudo cp /etc/repmgr/13/repmgr.conf /etc/repmgr/13/repmgr.conf.bk

sudo cat << EOF > repmgr.conf 
cluster='failovertest'
node_id=${HOSTNAME: -1}
node_name=${HOSTNAME}
conninfo='host=${HOSTNAME} user=repmgr dbname=repmgr connect_timeout=2'
data_directory='$PGDATA'
failover=automatic
promote_command='${PGBIN%/}/repmgr standby promote -f /var/lib/pgsql/repmgr.conf --log-to-file'
follow_command='${PGBIN%/}/repmgr standby follow -f /var/lib/pgsql/repmgr.conf --log-to-file --upstream-node-id=%n'
repmgrd_service_start_command='sudo /usr/bin/systemctl start repmgr-${PGVER}.service'
repmgrd_service_stop_command='sudo /usr/bin/systemctl stop repmgr-${PGVER}.service'
EOF

sudo cp repmgr.conf  /etc/repmgr/13/

$PGBIN/repmgr -f /etc/repmgr/13/repmgr.conf primary register

$PGBIN/repmgr -f /etc/repmgr/13/repmgr.conf cluster show


#Setup Replication on all nodes USING repmgr

#==========Standby side=============
sudo cat << EOF > repmgr.conf 
cluster='failovertest'
node_id=${HOSTNAME: -1}
node_name=${HOSTNAME}
conninfo='host=${HOSTNAME} user=repmgr dbname=repmgr connect_timeout=2'
data_directory='/var/lib/pgsql/13/data/'
failover=automatic
promote_command='/usr/pgsql-13/bin/repmgr standby promote -f /var/lib/pgsql/repmgr.conf --log-to-file'
follow_command='/usr/pgsql-13/bin//repmgr standby follow -f /var/lib/pgsql/repmgr.conf --log-to-file --upstream-node-id=%n'
repmgrd_service_start_command='sudo /usr/bin/systemctl start repmgr${PGVER}.service'
repmgrd_service_stop_command='sudo /usr/bin/systemctl stop repmgr${PGVER}.service'
EOF

sudo cp repmgr.conf /etc/repmgr/$PGVER/

##Stop and clean up the standby instance
sudo systemctl stop postgresql-$PGVER
echo $PGDATA
rm -rf $PGDATA
ls -al $PGDATA

--Do a dryrun onthe staandby
$PGBIN/repmgr -h pg0 -U repmgr -d repmgr -f /etc/repmgr/$PGVER/repmgr.conf standby clone --dry-run

--if no problem in dryrun do the actual data copy
$PGBIN/repmgr -h pg0 -U repmgr -d repmgr -f /etc/repmgr/$PGVER/repmgr.conf standby clone 

##Start server
sudo systemctl start postgresql-$PGVER
or
pg_ctl -D /var/lib/pgsql/13/data start

##register the standby
#$PGBIN/repmgr -h pg0 -U repmgr -d repmgr -f /etc/repmgr/13/repmgr.conf standby register
$PGBIN/repmgr -f /etc/repmgr/13/repmgr.conf standby register

##Check wether the daemon can be started
$PGBIN/repmgr -f /etc/repmgr/$PGVER/repmgr.conf daemon start --dry-run

##Create necessary directories
sudo mkdir -p /run/repmgr
sudo chown postgres:postgres -R /run/repmgr 

##Start the deamon on primary
$PGBIN/repmgr -f /etc/repmgr/$PGVER/repmgr.conf daemon start

#Check the daemon status
$PGBIN/repmgr -f /etc/repmgr/13/repmgr.conf daemon status

--Check cluster events
$PGBIN/repmgr -f /etc/repmgr/13/repmgr.conf cluster event


------------Pausing the cluster
$PGBIN/repmgr -f /etc/repmgr/$PGVER/repmgr.conf service pause

$PGBIN/repmgr -f /etc/repmgr/$PGVER/repmgr.conf daemon pause
$PGBIN/repmgr -f /etc/repmgr/$PGVER/repmgr.conf daemon status


#####################Examples##################################

#Pausing the service
$ repmgr -f /etc/repmgr/13/repmgr.conf service pause
NOTICE: node 1 (node1) paused
NOTICE: node 2 (pg2) paused
NOTICE: node 3 (pg1) paused

