.PHONY: up down reset status psql-primary psql-standby build

up:
	docker compose up -d --wait

down:
	docker compose down

# Destroy both data volumes and rebuild from scratch
reset:
	docker compose down -v
	docker compose up -d --wait

status:
	docker compose ps
	docker exec pg19-primary psql -U postgres -xc "select application_name, state, sent_lsn, write_lsn, flush_lsn, replay_lsn, write_lag, flush_lag, replay_lag from pg_stat_replication"

psql-primary:
	docker exec -it pg19-primary psql -U postgres

psql-standby:
	docker exec -it pg19-standby psql -U postgres

build:
	cargo build --release
