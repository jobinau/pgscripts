
########## Section 1. Sever Certificates , simple steps#########################
cd $PGDATA
rm -f server.crt server.key
#Generate private key "server.key" and a server certificate "server.crt". 
#IMPORTANT : Please make sure that the CN matches the server hostname used by the clients to connect
openssl req -nodes -new -x509 -keyout server.key -out server.crt -subj '/C=XX/L=Default City/O=Default Company Ltd/CN='${HOSTNAME}
chmod 400 server.* 

## In case, the certificate authentication is required, The Sever certificate itself can be treated as "root" certificate
cp server.crt root.crt
psql -c "ALTER SYSTEM SET ssl_ca_file = 'root.crt';"
##

psql -c "ALTER SYSTEM SET ssl=on;"

##Add entry to pg_hba.conf (examples)
hostssl     all     pmm_user        0.0.0.0/0               cert clientcert=verify-full
hostssl     all     all             ::/0      cert clientcert=verify-full

##Restart PostgreSQL
sudo systemctl restart postgresql-$PGVER

#######################################################


############### Section.1b  Sever Certificates (Multistep, alternate to Section 1) ###########
## Simple steps in the previous Section is preferaable
cd $PGDATA
## Let Server Geneate own private key
openssl genrsa -des3 -out server.key 1024
## remove pass phrase from key
openssl rsa -in server.key -out server.key
## Secure the key
chmod 400 server.key
## Geneate self singed certificate using the key
openssl req -new -key server.key -days 3650 -out server.crt -x509 -subj '/C=XX/L=Default City/O=Default Company Ltd/CN=postgres'
## enable SSL
alter system set ssl=on;
## edit hostssl in pg_hba.conf
hostssl    all             all             xxx.xxx.xx.xx/xx            xxxx
## reload
select pg_reload_conf();
##########################################################


################Client Certificates (Under testing)################################
## Allow client to generate own private keys
openssl genrsa -out client.key.pem 4096

## Let Client generates a certificate signing request (CSR) using its private key
USER=pmm_user
openssl req -new -key client.key.pem -out client.csr -subj "/C=IN/ST=Telengana/L=Hyderabad/O=Percona/OU=Service/CN=${USER}/emailAddress=${USER}@percona.com"
## Send it to server host for signing 
scp client.csr pg1:

##Allow the server to sign the certificate with its private key
openssl x509 -req -in client.csr -CA $PGDATA/root.crt -CAkey $PGDATA/server.key -out  client.crt -CAcreateserial

##Client can copy the signed certificate and root certificate
scp postgres@pg1:client.crt .
scp postgres@pg1:/var/lib/pgsql/14/data/root.crt .

## connect from client with certificte authentication
psql "host=pg1 dbname=postgres user=pmm_user sslmode=verify-ca sslcert=client.crt sslkey=client.key.pem sslrootcert=root.crt"
#verify-full requires server certificate to have the CN as the server hostname which will be specified in the client connection
#A workaround is to specify the a different hostname in the /etc/hosts which matches the certificate CN

################Additional Checks##############################
#Check a Certificate content
openssl x509 -in server.crt -text -noout
#Check CSR content
openssl req -in client.csr -text -noout


