--Table Attribute collations
WITH defcoll AS (
   SELECT datcollate AS coll
   FROM pg_database
   WHERE datname = current_database()
)
SELECT n.nspname "schema",t.relname "table", a.attname "attribute", c.collname "Spec Collation",
       CASE WHEN c.collname = 'default'
            THEN defcoll.coll
            ELSE c.collname
       END AS "Effective Collation"
FROM pg_attribute AS a
   CROSS JOIN defcoll
   LEFT JOIN pg_collation AS c ON a.attcollation = c.oid
   LEFT JOIN pg_class as t ON a.attrelid = t.oid
   LEFT JOIN pg_namespace as n ON n.oid = t.relnamespace
WHERE a.attnum > 0 and c.collname IS NOT NULL
AND n.nspname NOT IN ('pg_catalog','information_schema')
ORDER BY attnum;



--Index Collations
WITH defcoll AS (
   SELECT datcollate AS coll
   FROM pg_database
   WHERE datname = current_database()
)
SELECT i.indrelid::regclass::text "Table",i.indexrelid::regclass::text "Index",indisprimary,icol.pos,
       CASE WHEN c.collname = 'default'
            THEN defcoll.coll
            ELSE c.collname
       END AS collation
FROM pg_index AS i
   LEFT JOIN LATERAL unnest(i.indcollation) WITH ORDINALITY AS icol(coll, pos) ON icol.coll != 0
        --or ON TRUE
   CROSS JOIN defcoll
   LEFT JOIN pg_collation AS c ON c.oid = icol.coll
WHERE icol.pos IS NOT NULL
ORDER BY icol.pos;