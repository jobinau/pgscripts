WITH cur_vaccs AS (SELECT split_part(split_part(substring(query from '.*\..*'),'.',2),' ',1) as tab FROM pg_stat_activity WHERE query like 'autovacuum%')
select 'VACUUM FREEZE "'|| n.nspname ||'"."'|| c.relname ||'";'
  from pg_class c 
  inner join pg_namespace n on c.relnamespace = n.oid
  left join pg_class t on c.reltoastrelid = t.oid and t.relkind = 't'
where c.relkind in ('r','m') 
AND NOT EXISTS (SELECT * FROM cur_vaccs WHERE tab = c.relname)
order by GREATEST(age(c.relfrozenxid),age(t.relfrozenxid)) DESC
limit 100;
\gexec