GOOGLE_URL=https://storage.googleapis.com/etcd
GITHUB_URL=https://github.com/etcd-io/etcd/releases/download
DOWNLOAD_URL=${GOOGLE_URL}
read -e -p "Enter the ETCD version to be Installed :" -i "v3.3.27" ETCD_VER
echo $ETCD_VER
rm -rf /tmp/etcd-download-test && mkdir -p /tmp/etcd-download-test
cd /tmp/
curl -L ${DOWNLOAD_URL}/${ETCD_VER}/etcd-${ETCD_VER}-linux-amd64.tar.gz -o /tmp/etcd-${ETCD_VER}-linux-amd64.tar.gz
tar xzvf /tmp/etcd-${ETCD_VER}-linux-amd64.tar.gz -C /tmp/etcd-download-test --strip-components=1
rm -f /tmp/etcd-${ETCD_VER}-linux-amd64.tar.gz
/tmp/etcd-download-test/etcd --version
ETCDCTL_API=3 /tmp/etcd-download-test/etcdctl version
if [ $? -eq 0 ]; then
    echo OK
else
    echo "Exiting as etcdctl not found"
    exit
fi
cd /tmp/etcd-download-test/
sudo cp etcd /usr/bin/
sudo cp etcdctl /usr/bin/
sudo chmod 755 /usr/bin/etcd* 
sudo groupadd --system etcd
sudo useradd -s /sbin/nologin --system -g etcd etcd
sudo mkdir /var/lib/etcd
sudo chown -R etcd:etcd /var/lib/etcd/
sudo mkdir /etc/etcd
ips=($(hostname -I))
for ip in "${ips[@]}"; do
     echo $ip; 
done
read -e -p "Enter the host ip from above list :" -i $ip HOST_IP
HOSTNAME=`hostname`
cat > etcd.conf  << ENDOFFILE
ETCD_NAME=$HOSTNAME
ETCD_INITIAL_CLUSTER="$HOSTNAME=http://$HOST_IP:2380"
ETCD_INITIAL_CLUSTER_TOKEN="devops_token"
ETCD_INITIAL_CLUSTER_STATE="new"
ETCD_INITIAL_ADVERTISE_PEER_URLS="http://$HOST_IP:2380"
ETCD_DATA_DIR="/var/lib/etcd/postgres.etcd"
ETCD_LISTEN_PEER_URLS="http://$HOST_IP:2380"
ETCD_LISTEN_CLIENT_URLS="http://$HOST_IP:2379,http://localhost:2379"
ETCD_ADVERTISE_CLIENT_URLS="http://$HOST_IP:2379"
ETCD_ENABLE_V2="true"
ENDOFFILE
sudo cp etcd.conf /etc/etcd
curl -LO https://raw.githubusercontent.com/jobinau/pgscripts/main/patroni/etcd.service
sudo mv etcd.service /usr/lib/systemd/system/
sudo systemctl enable etcd