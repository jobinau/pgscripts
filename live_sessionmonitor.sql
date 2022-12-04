--Jobin's experimental live session waitevent monitoring query 
--Please feed me with feedback
SELECT pid,string_agg( w.wait_event ||':'|| w.cnt,',') waits FROM 
(SELECT pid,COALESCE(wait_event,'CPU') wait_event,count(*) cnt FROM 
(SELECT pid,wait_event
FROM generate_series(1,1000) AS g
LEFT JOIN LATERAL pg_stat_get_activity(NULLIF(pg_sleep(0.001)::text||g,g::text)::INT) on TRUE WHERE state != 'idle' and pid != pg_backend_pid())
pg_pid_wait GROUP BY 1,2) as w
GROUP BY 1;