.PHONY: up down reset status psql-primary psql-standby build test doc doc-open lint

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

test:
	cargo test --release

# Source documentation: DESIGN.md is the front page, then every module/function/struct.
# --document-private-items is needed because a binary crate has no public API.
doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items
	@echo "open target/doc/waitforlsn/index.html"

doc-open:
	cargo doc --no-deps --document-private-items --open

# Fails if any function, struct, field, const or module lacks a doc comment.
lint:
	cargo clippy --release -- -D warnings -W clippy::missing_docs_in_private_items
