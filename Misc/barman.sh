#Barman Server : pg2
#Postgresql Server : pg0
###################ON THE BACKUP SERVER (pg2) #######################
sudo yum install -y epel-release

##Install PG repo
curl -LO https://raw.githubusercontent.com/jobinau/pgscripts/main/pgdg_centos.sh
chmod 700 pgdg_centos.sh
./pgdg_centos.sh 1

#Install barman from pgdg-common  
sudo yum install -y barman
#For example : barman  noarch  2.18-1.rhel7 pgdg-common on (feb 2022)
#automatically installs letest libpq5 from pgdg-common  and python libraries and rsync 
#Install PostgreSQL client tools
sudo yum install -y postgresql14
#set path
export PATH=/usr/pgsql-14/bin/:$PATH
echo "PATH=/usr/pgsql-14/bin/:$PATH" >> .bash_profile

#Above installation automatically creates the barman user account
id barman
#uid=998(barman) gid=997(barman) groups=997(barman)
#and home directory of barman will be /var/lib/barman
cat /etc/passwd | grep barman

#set password for barman user
sudo passwd barman

##Add backup configuration
cat > pg0.conf << ENDOFFILE
[pg0]
description =  "Example of PostgreSQL Database (Streaming-Only)"
conninfo = host=pg0 user=barman dbname=postgres password=secret
streaming_conninfo = host=pg0 user=replicator password=vagrant
backup_method = postgres
streaming_archiver = on
slot_name = barman
ENDOFFILE
sudo cp pg0.conf /etc/barman.d/
cat /etc/barman.d/pg0.conf

#Install openssh-server if required
sudo yum install -y openssh-server
sudo systemctl start sshd


#Switch to barman user
sudo su - barman


#establish password-less ssh to database server and user
#NOTE: I had to rerun the ssh-keygen multiple tiems . please investigate next time
ssh-keygen -q -t rsa -N '' -f ~/.ssh/id_rsa <<<y 2>&1 >/dev/null
ssh-copy-id postgres@pg0

#Set path to include PG client tools
echo 'export PATH=/usr/pgsql-14/bin/:$PATH' >> ~/.bash_profile

#Relogin as barman
exit
sudo su - barman



###########ON DATABASE SERVER (pg0) #########################################
#setup password-less ssh to barman user of the backup server
sudo yum install -y rsync
ssh-keygen -q -t rsa -N '' -f ~/.ssh/id_rsa <<<y 2>&1 >/dev/null
ssh-copy-id barman@pg2

#Create a database super user "barman" in the PostgreSQL
createuser -s -P barman
psql -c "ALTER USER barman WITH PASSWORD 'secret'"
#Create streaming replication user if not already existing
psql -c "CREATE ROLE replicator WITH REPLICATION PASSWORD 'vagrant' LOGIN" 

#Edit pg_hba.conf to allow both replication and regular connection from backup server
sed -i "/^local.*all.*all.*/a\
host    replication     replicator      10.130.0.0/16        md5\n\
host    all     all      10.130.0.0/16        md5\n\
" $PGDATA/pg_hba.conf

psql -c "select pg_reload_conf()"

##Update archive_command LATER to rsync the WALs to incoming directory of barman
alter system set archive_command = 'rsync -a %p barman@pg2:/var/lib/barman/pg0/incoming/%f';
#Use the : barman show-server pg0  | grep incoming_wals_directory
#The arrchive mode must be on
ALTER SYSTEM SET archive_mode=on;
sudo systemctl restart postgresql-14 

################ Verifying and Testing ###########################
#From barman server as "barman" user
barman receive-wal --create-slot pg0

#if the WAL archival from the database is enabled, add archiver=on to the configuratioin file
sudo sh -c 'echo "archiver=on" >> /etc/barman.d/pg0.conf'

##Start a forground WAL streaming. Ctrl+C after the testing. This uses the conninfo
barman receive-wal pg0

##Create some load on the DB server
pgbench -i -s 10

##Start the barman cron
barman cron

##Check the configuration
barman show-server pg0

## Check the overal backup configuraiton status. IMPORTANT for troubleshooting
barman check pg0   
##or check all
barman check all

##Check Wal streaming status
barman replication-status pg0

##Take backup 
barman backup pg0

###The backup will be creating a restore point, see the pg log for details
2022-02-10 13:28:37.973 UTC [2719] LOG:  restore point "barman_20220210T132835" created at 0/B000090
2022-02-10 13:28:37.973 UTC [2719] STATEMENT:  SELECT pg_create_restore_point('barman_20220210T132835')

##List available backups
barman list-backup pg0
#pg0 20220210T132835 - Thu Feb 10 13:28:37 2022 - Size: 117.0 MiB - WAL Size: 64.0 MiB

#Restore Backup
barman recover --remote-ssh-command "ssh postgres@pg0" pg0 20220210T132835 /var/lib/pgsql/14/data

##Point in time recovery
#barman recover --target-time "providethetime" --remote-ssh-command "ssh postgres@remote-target-hostname"  <servername from catalog> <restore location>
barman recover --target-time "2017-04-11 13:53:06.035713+08:00"  --remote-ssh-command "ssh postgres@it-postgresql-sing-01v"   it-postgresql-sing-01v   20170411T135306   /usr/local/pgsql/data-restore

##After restore operation, Postgresql can be started up and it will open up (not in recovery mode)



######Troubleshooting###################
ERROR: Cannot connect to server 'pg0'
#Use psql to check the same connection string specified in barman configuratioin. For example,
psql "host=pg0 user=barman dbname=postgres password=secret"

#Log file. By default it will be 
/var/log/barman/

#Use 
barman check pg0
#Symptom 
PostgreSQL: OK => 
Test whether the conninfo is working fine (conninfo = host=pg0 user=barman dbname=postgres password=secret)
#Symptom 
replication slot: FAILED (slot 'barman' not initialised: is 'receive-wal' running?) 
Run : barman cron
#Symptom
backup hangs
Check: postgreSQL log for archive failures
OR : issue checkpoint

#For additional references 
CS0024650/CS0024650_barman/*.conf


"retention_policy": "redundancy 2 b",
retention_policy = REDUNDANCY 2