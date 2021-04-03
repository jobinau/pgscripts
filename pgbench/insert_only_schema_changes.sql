ALTER TABLE pgbench_accounts
	ADD created_on timestamptz,
	DROP CONSTRAINT pgbench_accounts_pkey;

---Either create unique index
CREATE UNIQUE INDEX ON pgbench_accounts(aid, created_on DESC);

--Normal index
CREATE INDEX ON pgbench_accounts(created_on);
CREATE INDEX ON pgbench_accounts USING BRIN(created_on);