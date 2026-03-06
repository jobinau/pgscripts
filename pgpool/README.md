
##  Store AES encrypted passwrod in `pool_passwd` 
```
sudo touch /etc/pgpool-II/pool_passwd
sudo chown postgres: /etc/pgpool-II/pool_passwd
echo 'some string' > ~/.pgpoolkey
chmod 600 ~/.pgpoolkey
pg_enc -m -k ~/.pgpoolkey -f /etc/pgpool-II/pgpool.conf -u user1 -p
```
```verify
$ cat /etc/pgpool-II/pool_passwd
user1:AESDs5wrD6cHaL1ISuGX2KVoQ==
```

REMEMBER : pgpool can accept the key file with parameter  
  -k, --key-file=KEY_FILE  
 (default: /home/postgres/.pgpoolkey)  
 like : pgpool  -n -f /etc/pgpool-II/pgpool.conf -F /etc/pgpool-II/pcp.conf -k /etc/pgpool-II/.pgpoolkey


## Test connectivity
```
$ PGPASSWORD=user1 psql -h 127.0.0.1 -p 9999 -U user1 -d postgres
psql (18.0)
Type "help" for help.

postgres=>
```