#!/usr/bin/python
############################### HAtester.py #####################################
# 	Version 2.1    Jobin Augustine 2017
# Program to test retry connecton on connection a connection failiure, 
# Ctrl+C will result in  fresh connection to test load balancing.

#INSTALLATION AND RUN.
# This needs  python-psycopg2 to be present in the system
#Edit following connection string, with hostname, port and credentials
connectionString = "host=localhost port=5432 dbname=postgres user=postgres password=vagrant connect_timeout=5"
#./HAtester.py
#Execute Insert statement against table if doDML is true.
#create a table in advance: CREATE TABLE HATEST (TM TIMESTAMP);
doDML = True

#STOP EXECUTION
# Step 1 push the process to background : ctrl+z
# Step 2 : pkill -9 HAtester.py
#   OR  pgrep HAtester.py | xargs kill -9

###############################################################################

import sys,time,psycopg2
def create_conn():
   try:
      conn = psycopg2.connect(connectionString)
   except psycopg2.Error as e:
      print("Unable to connect to database :")
      print(e)
      return
   return conn

if __name__ == "__main__":
   conn = create_conn()
   if conn is not None:
      cur = conn.cursor()
      while True:
         try:
            time.sleep(1)
            if conn is not None:
               cur = conn.cursor()
            else:
               raise Exception("Connection not ready")
            #Check connected to master or Slave
            cur.execute("select pg_is_in_recovery(),inet_server_addr()")
            rows = cur.fetchone()
            if (rows[0] == False):
               print "Working with Master " + rows[1],
               if doDML:
                  cur.execute("INSERT INTO HATEST VALUES(CURRENT_TIMESTAMP) RETURNING TM")
                  if cur.rowcount == 1 :
                     conn.commit()
                     tmrow = str(cur.fetchone()[0])
                     print 'Inserted ' + tmrow
               else:
                  print " No Attempt to insert data"
            else:
               print "Working with Slave " + rows[1],
               if doDML:
                  cur.execute("SELECT MAX(TM) FROM HATEST")
                  row = cur.fetchone()
                  print " Retrived timestamp from database is " + str(row[0])
               else:
                  print " No Attempt to retrive data"

         except:
            print "Trying to connect"
            if conn is not None:
               conn.close()
            conn = create_conn()
            if conn is not None:
		         cur = conn.cursor()

   conn.close()

