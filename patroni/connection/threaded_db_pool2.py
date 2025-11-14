#!/usr/bin/python3

#####################################################################
# Demonstrates a connection pool can multiplex hundreds of threads onto a very few database connections +
# Application side connection routing +
# Pool is prepared at the begining of application +
# Pool automatically recreated if there is swichover
#####################################################################

import threading
import time
import psycopg2
from psycopg2 import pool

# --- Configuration ---
# Replace these with your actual PostgreSQL connection details.
DSN = "postgresql://postgres:vagrant@pg0,pg1,pg2/postgres?application_name=testapp&target_session_attrs=read-write"

# --- Database Setup Instructions ---
# Before running this script, please connect to your PostgreSQL database
# and run the following SQL commands to create a sample table and data.
#
# CREATE TABLE employees (
#     id SERIAL PRIMARY KEY,
#     name VARCHAR(100),
#     department VARCHAR(50)
# );
#
# INSERT INTO employees (name, department) VALUES
# ('Alice', 'Engineering'),
# ('Bob', 'Marketing'),
# ('Charlie', 'Sales'),
# ('David', 'Engineering'),
# ('Eve', 'HR');
#

# Global connection pool and semaphore variables
connection_pool = None
pool_semaphore = None

def worker_thread_task(thread_id):
    """
    This function represents the task performed by each thread.
    It tries to acquire a connection slot with a timeout. If successful,
    it performs a query, prints the result, and returns the connection.
    """
    print(f"Thread-{thread_id}: Starting...")
    while True:
        # Try to acquire a semaphore with a 2-second timeout. 
        # Semaphore is required only for timeout. without which thread will be waiting forever.
        # This simulates waiting for a free connection slot with a limit.
        got_slot = pool_semaphore.acquire(timeout=2)

        if got_slot:
            conn = None
            try:
                # 1. We have a slot, now get a connection from the pool.
                # This call should be fast as the semaphore ensures a connection is likely available.
                conn = connection_pool.getconn()

                with conn.cursor() as cur:
                    # 2. Execute a query
                    cur.execute("SELECT id, name, department FROM employees ORDER BY random() LIMIT 1;")

                    # 3. Fetch and print the result
                    employee = cur.fetchone()
                    if employee:
                        print(f"Thread-{thread_id}: Fetched -> ID: {employee[0]}, Name: {employee[1]}, Dept: {employee[2]}")
                    else:
                        print(f"Thread-{thread_id}: No employees found.")

            except psycopg2.Error as e:
                print(f"Thread-{thread_id}: Database error: {e}")
                # If there's a DB error, the connection might be broken.
                # The pool should close it rather than putting it back.
                if conn:
                    connection_pool.putconn(conn, close=True)
                conn = None  # Ensure it is not put back again in finally

            finally:
                # 4. Return the connection to the pool for reuse
                if conn:
                    connection_pool.putconn(conn)
                # 5. ALWAYS release the semaphore slot if we acquired it
                pool_semaphore.release()
                print(f"Thread-{thread_id}: Released connection and slot.")
        else:
            # This block executes if the semaphore acquisition timed out
            print(f"Thread-{thread_id}: !! TIMED OUT waiting for a free connection. Retrying... !!")

        # Wait a moment before the next attempt
        time.sleep(1)


if __name__ == "__main__":
    print("--- Psycopg2 Threaded Connection Pool Demo ---")
    print("Starting program. Press Ctrl+C to exit.")

    try:
        # To demonstrate timeouts, we'll set max connections lower than the number of threads.
        max_connections = 2 # Pool size, Maximum number of database connections from the pool
        num_threads = 400  # Number of application worker threads to spawn

        print(f"Initializing pool with max {max_connections} connections for {num_threads} threads.")

        # Initialize the connection pool
        connection_pool = psycopg2.pool.ThreadedConnectionPool(
            minconn=2,
            maxconn=max_connections,
            dsn=DSN
        )

        # Create a semaphore to control access to the pool, matching the pool's max size
        pool_semaphore = threading.Semaphore(max_connections)

        threads = []

        # Create and start the worker threads
        for i in range(num_threads):
            thread = threading.Thread(target=worker_thread_task, args=(i + 1,))
            thread.daemon = True  # Allows main program to exit even if threads are running
            threads.append(thread)
            thread.start()

        # Keep the main thread alive to allow worker threads to run
        while True:
            time.sleep(1)

    except psycopg2.OperationalError as e:
        print(f"\n[ERROR] Could not connect to the database: {e}")
        print("Please check your DB_CONFIG and ensure the database is running.")
    except KeyboardInterrupt:
        print("\nExiting program...")
    finally:
        # Gracefully close all connections in the pool
        if connection_pool:
            connection_pool.closeall()
            print("All database connections have been closed.")

