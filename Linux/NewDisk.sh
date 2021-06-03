# Script for adding a new disk as a drive - Jobin
#findout the new disk of 50GB, Change the regex with size change
newdisk=`sudo fdisk -l | grep -e '^Disk.*\W5[0-9].*Gi*B' | awk '{split($2,a,":"); print a[1]}'`
echo $newdisk
#Create A partition in the disk.
sed -e 's/\s*\([\+0-9a-zA-Z]*\).*/\1/' << EOF | sudo fdisk ${newdisk}
  o # clear the in memory partition table
  n # new partition
  p # primary partition
  1 # partition number 1
    # default - start at beginning of disk 
    # default size of partition
  w # save
  q # quit
EOF
#List the Partitions - Check point 1
sudo fdisk ${newdisk} -l

# p1 will be added to first partition name. for example, /dev/nvme1n1p1
newdrive=$newdisk"p1"
#Format the partiton as EXT4.
sudo mkfs.ext4 $newdrive
#Mount the new partition
sudo mkdir /data
sudo newdrive=$newdrive bash -c 'cat <<EOF >>/etc/fstab
$newdrive /data ext4   noatime,data=writeback,barrier=0,nobh
EOF'
sudo mount /data

#PostgreSQL user provisioning with home in /home/postgres and with sudo privillage.
sudo groupadd postgres
sudo useradd -r -g postgres postgres
sudo mkdir /home/postgres
sudo usermod -m -d /home/postgres -s /bin/bash postgres
sudo getent passwd|grep postgres   ##Verify
sudo chown postgres:postgres /home/postgres
sudo sh -c 'echo "%postgres ALL=(ALL) NOPASSWD: ALL" > /etc/sudoers.d/postgres'
sudo chmod 0440 /etc/sudoers.d/postgres
sudo mkdir /home/postgres/.ssh
sudo cp ~/.ssh/authorized_keys /home/postgres/.ssh/
sudo chown -R postgres:postgres /home/postgres
sudo su - postgres
chmod 700 ~/.ssh && chmod 600 ~/.ssh/*
restorecon -R ~/.ssh
