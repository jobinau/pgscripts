



sudo chown -R postgres:postgres /etc/pgbouncer/
cd /etc/pgbouncer/
cat <<EOF | psql -h pg0 -U postgres -q -d postgres
\COPY (SELECT usename, passwd FROM pg_shadow ) TO '/etc/pgbouncer/userlist.txt'  WITH (FORMAT CSV, DELIMITER ' ', FORCE_QUOTE *)
EOF

cat << EOF > /etc/pgbouncer/pgbouncer.ini
[databases]
db1pgbounce = host=pg0 port=5432 dbname=postgres
 
[pgbouncer]
listen_port = 6432
listen_addr = *
auth_type = md5
auth_file = /etc/pgbouncer/userlist.txt
logfile = pgbouncer.log
pidfile = pgbouncer.pid
admin_users = postgres
EOF

##Connect to pgbouncer local database
psql -h localhost -p 6432 pgbouncer

#Connect to remote database
psql -h localhost -p 6432 db1pgbounce