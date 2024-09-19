#!/usr/bin/python3
import psycopg2

try:
    conn = psycopg2.connect("host=localhost dbname=postgres user=user1 password=user1" )
    cur = conn.cursor()

    query = "SELECT * FROM t1"
    cur.execute(query)
    rows = cur.fetchall()

    for row in rows:
        print(row)

except psycopg2.Error as e:
    print("Error:", e)

finally:
    cur.close()
    conn.close()
