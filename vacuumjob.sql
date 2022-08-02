WITH cur_vaccs AS (SELECT split_part(split_part(substring(query from '.*\..*'),'.',2),' ',1) as tab FROM pg_stat_activity WHERE query like 'autovacuum%')
SELECT 'VACUUM FREEZE "'|| n.nspname ||'"."'|| c.relname ||'";'
 FROM pg_class c
 JOIN pg_namespace n ON c.relnamespace = n.oid
 LEFT JOIN pg_class t ON c.reltoastrelid = t.oid and t.relkind = 't'
 WHERE c.relkind in ('r','m') AND NOT EXISTS (SELECT * FROM cur_vaccs WHERE tab = c.relname)
ORDER BY GREATEST(age(c.relfrozenxid),age(t.relfrozenxid)) DESC
LIMIT 100;
\gexec
