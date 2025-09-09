# Connection pooler using deadpool-postgres.
hundreds of connections will be picking a connection from the pool and executing the queries
## Build
```
cargo build --release
```
## Run
```
time target/release/rustpg  > /dev/shm/out.txt
```

### Check the threads running on CPU core
```
ps -L -o pid,tid,psr,pcpu,comm -p `pidof rustpg`
```