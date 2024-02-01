sudo apt install -y curl wget
##Environment Variables
export PGVER=$1
export PG=postgresql$PGVER
export PATH=$PATH:/usr/lib/postgresql/$PGVER/bin
##Edit the bashrc
echo 'export PGBIN=/usr/lib/postgresql/'"$PGVER"'/bin/' >> ~/.bashrc
#sed -i 's/PATH=.*/&:\/usr\/lib\/postgresql\/'"$PGVER"'\/bin/' ~/.bashrc
echo 'export PATH=$PGBIN:$PATH' >> ~/.bashrc
echo 'export PGDATA=/var/lib/postgresql/'"$PGVER"'/main/' >> ~/.bashrc

#OS type (Ubuntu or Debian)
OSTYPE=`cat /etc/*release | grep "^ID=" | cut -b 4-`
if [ "$OSTYPE" = "debian" ]; then
    echo "Debian Detected"
else
    echo "Ubuntu Detected"
fi

##Installation
sudo apt-get install -y curl ca-certificates gnupg
wget --quiet -O - https://www.postgresql.org/media/keys/ACCC4CF8.asc | sudo apt-key add -
sudo sh -c 'echo "deb http://apt.postgresql.org/pub/repos/apt/ $(lsb_release -cs)-pgdg main" > /etc/apt/sources.list.d/pgdg.list'
sudo apt-get update
sudo apt search postgresql-$PGVER
sudo apt install -y postgresql-$PGVER

##Parameter modifications
sed -i '/listen_addresses/c\listen_addresses = \x27*\x27' /etc/postgresql/$PGVER/main/postgresql.conf
sed -i '/max_wal_senders/c\max_wal_senders = 5' /etc/postgresql/$PGVER/main/postgresql.conf
sed -i '/^#logging_collector/c\logging_collector = on' /etc/postgresql/$PGVER/main/postgresql.conf
sed -i '/^#hot_standby/c\hot_standby = on' /etc/postgresql/$PGVER/main/postgresql.conf

##Direct command
/usr/lib/postgresql/$PGVER/bin/pg_ctl status -D /var/lib/postgresql/$PGVER/main -o '--config-file=/etc/postgresql/$PGVER/main/postgresql.conf'

