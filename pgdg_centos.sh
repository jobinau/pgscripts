###############################################################################
##  Installation script for CentOS/Redhat on x86_64, Arm64
##  Author : Jobin Augustine 
##  Ref : https://www.postgresql.org/download/linux/redhat/
###############################################################################

##Validate the input
export FULLVER=$1
if [[ $FULLVER =~ ^[+-]?[0-9]+\.?[0-9]*$ ]]; then 
   export PGVER=${FULLVER%.*}
else
   echo "Please input a valid version"
   exit 1
fi
if [ $USER != "postgres" ]; then
    read -p "Installation as \"postgres\" user with sudo privillage is recommended. Would you like to continue installation as \"$USER\" user? " -n 1 -r
    if [[ $REPLY =~ ^[Nn]$ ]]
    then
        echo "\nExiting the Installation"
        exit 1
    fi
fi
echo "Starting Installation"
##use appropriate version number like 9.6 or 9.5
#export PG=postgresql$PGVER
##Remove "." from older versions
export PG=postgresql`echo "$PGVER" | tr -d .`
export OSVER=`cat /etc/*release | grep "VERSION_ID=" | cut -b 13`
if [ $OSVER -ge "8" ]; then
    CMD=dnf
    sudo dnf -qy module disable postgresql
else
    CMD=yum
fi
ARCH=`uname -i`
echo "https://download.postgresql.org/pub/repos/yum/$PGVER/redhat/rhel-$OSVER-$ARCH"
sudo $CMD -y install https://download.postgresql.org/pub/repos/yum/reporpms/EL-$OSVER-$ARCH/pgdg-redhat-repo-latest.noarch.rpm

if [ $PGVER == $FULLVER ]; then
    sudo $CMD install -y $PG-server --nogpgcheck
else
    sudo $CMD install -y $PG-server-$FULLVER --nogpgcheck
fi
##sudo $CMD groupinstall -y postgresqldbserver$PGVER
##Set Enviornment Variables
export PATH=$PATH:/usr/pgsql-$PGVER/bin
sed -i 's/PATH=.*/&:\/usr\/pgsql-'"$PGVER"'\/bin/' ~/.bash_profile
## following can keep the PGBIN path to starting of the PATH
## sed -r 's/(PATH=)(.+)/\1\/usr\/pgsql-'"$PGVER"'\/bin:\2/' ~/.bash_profile
echo 'export PGVER='$PGVER >> ~/.bash_profile
echo 'export PGBIN=/usr/pgsql-'"$PGVER"'/bin/' >> ~/.bash_profile
echo 'export PGDATA=/var/lib/pgsql/'"$PGVER"'/data/' >> ~/.bash_profile
echo 'export PATH=$PGBIN:$PATH' >> ~/.bash_profile

#Contrib
#sudo $CMD install -y $PG-contrib
export PGDATA=/var/lib/pgsql/$PGVER/data/
#Enable checksum
export PGSETUP_INITDB_OPTIONS="--data-checksums"
sudo systemctl enable postgresql-$PGVER

sudo PGSETUP_INITDB_OPTIONS=$PGSETUP_INITDB_OPTIONS /usr/pgsql-$PGVER/bin/postgresql*-setup initdb
sed -i '/listen_addresses/c\listen_addresses = \x27*\x27' /var/lib/pgsql/$PGVER/data/postgresql.conf
sed -i -e '/# IPv4 local connections:/a host    replication     replicator      192.168.50.0/24        md5\nhost    all     all      192.168.50.0/24        md5' $PGDATA/pg_hba.conf
sudo systemctl start postgresql-$PGVER
psql -c "ALTER USER postgres WITH PASSWORD 'vagrant'"

psql -c "ALTER SYSTEM SET WAL_LEVEL=logical"
sudo systemctl restart postgresql-$PGVER
