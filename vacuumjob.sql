----------------------------------------------------------------------------------------------------------
-- vacuumjob.sql : Template/example for custom scheduled vacuum job
-- Adjust the LIMIT and criteria as per specific needs
-- Schedule the script for all off-peak times
-- crontab example : 20 11 * * * /full/path/to/psql -X -f /path/to/vacuumjob.sql > /tmp/vacuumjob.out 2>&1
-----------------------------------------------------------------------------------------------------------
\set ECHO all
---Take care of tables which are having old xid which is pending for FREEZE operation. This can cause agreessive autovacuum at peak times.
WITH cur_vaccs AS (SELECT split_part(split_part(substring(query from '.*\..*'),'.',2),' ',1) as tab FROM pg_stat_activity WHERE query like 'autovacuum%')
SELECT 'VACUUM (FREEZE,ANALYZE) "'|| n.nspname ||'"."'|| c.relname ||'";'
 FROM pg_class c
 JOIN pg_namespace n ON c.relnamespace = n.oid
 LEFT JOIN pg_class t ON c.reltoastrelid = t.oid and t.relkind = 't'
 WHERE c.relkind in ('r','m') AND NOT EXISTS (SELECT * FROM cur_vaccs WHERE tab = c.relname)
ORDER BY GREATEST(age(c.relfrozenxid),age(t.relfrozenxid)) DESC
LIMIT 100;
\gexec

--Take care of specific tables if any, which requires special attention because they have heavy dead tuple generations
--VACUUM (FREEZE,ANALYZE) bank_account;

--Take care of tables with high dead tuple precentage, Soon they may become candidate.
WITH cur_vaccs AS (SELECT split_part(split_part(substring(query from '.*\..*'),'.',2),' ',1) as tab FROM pg_stat_activity WHERE query like 'autovacuum%')
SELECT 'VACUUM (FREEZE,ANALYZE) "'|| schemaname ||'"."'|| relname ||'";' FROM pg_stat_user_tables 
  WHERE n_dead_tup::float/nullif(n_live_tup,0)> 0.05 AND NOT EXISTS (SELECT * FROM cur_vaccs WHERE tab = pg_stat_user_tables.relname)
ORDER BY n_dead_tup::float/nullif(n_live_tup,0) DESC
LIMIT 10;
\gexec

