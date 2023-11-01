#!/usr/bin/bash
# This script is to setup ETCD on Ubuntu machines
# ssh pgu1 , ssh pgu2 should work without password
# PRE REQUISITE : 
#    1. All hostnames should be available in /etc/hosts
#    2. etcd should be already installed on all nodes

#Please edit the curNode and otherNodes
curNode=pgu0    #The current node (first node) of the cluster
otherNodes=(pgu1 pgu2)   #Other nodes of the cluster

export PG0HOST=`cat /etc/hosts | grep $curNode | awk '{print $1}'`
cat > etcd.conf  << ENDOFFILE
ETCD_NAME=$curNode
ETCD_INITIAL_CLUSTER="$curNode=http://$PG0HOST:2380"
ETCD_INITIAL_CLUSTER_TOKEN="devops_token"
ETCD_INITIAL_CLUSTER_STATE="new"
ETCD_INITIAL_ADVERTISE_PEER_URLS="http://$PG0HOST:2380"
ETCD_DATA_DIR="/var/lib/etcd/postgres.etcd"
ETCD_LISTEN_PEER_URLS="http://$PG0HOST:2380"
ETCD_LISTEN_CLIENT_URLS="http://$PG0HOST:2379,http://localhost:2379"
ETCD_ADVERTISE_CLIENT_URLS="http://$PG0HOST:2379"
ENDOFFILE
sudo mv etcd.conf /etc/default/etcd
sudo systemctl restart etcd
sudo systemctl enable etcd
sudo systemctl status etcd
etcdctl member list

for host in "${otherNodes[@]}"
do
    echo $host
    sleep 5
    export HOSTIP=`cat /etc/hosts | grep $host | awk '{print $1}'`
    etcdctl member add $host http://$HOSTIP:2380 | tee ${host}etcd.conf
    sleep 1
cat >> ${host}etcd.conf  << ENDOFFILE
ETCD_INITIAL_CLUSTER_TOKEN="devops_token"
ETCD_INITIAL_ADVERTISE_PEER_URLS="http://$HOSTIP:2380"
ETCD_DATA_DIR="/var/lib/etcd/postgres.etcd"
ETCD_LISTEN_PEER_URLS="http://$HOSTIP:2380"
ETCD_LISTEN_CLIENT_URLS="http://$HOSTIP:2379,http://localhost:2379"
ETCD_ADVERTISE_CLIENT_URLS="http://$HOSTIP:2379"
ENDOFFILE
    sed -i -e '1,2d' ${host}etcd.conf
    scp ${host}etcd.conf postgres@$host:etcd.conf
    ssh -t postgres@$host 'sudo mv etcd.conf /etc/default/etcd; sudo systemctl restart etcd; sudo systemctl enable etcd; sudo systemctl status etcd'
    sleep 1
    etcdctl member list
done
