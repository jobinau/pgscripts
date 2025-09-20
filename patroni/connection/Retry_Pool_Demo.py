#!/usr/bin/env python3

#####################################################################
# Demonstrates a resilient, multi-threaded connection pool that
# uses `tenacity` for retries and supports graceful shutdown.
#####################################################################

import logging
import threading
import time
import psycopg2
from psycopg2 import pool
from tenacity import retry, stop_after_attempt, wait_exponential, RetryError, retry_if_exception_type

# --- Basic Logging Configuration ---
# Using the logging module is better than print() for applications.
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(threadName)s - %(levelname)s - %(message)s',
    datefmt='%Y-%m-%d %H:%M:%S'
)

# --- Configuration ---
# Replace these with your actual PostgreSQL connection details.
DSN = "postgresql://postgres:vagrant@localhost/postgres?application_name=resilient_app"

# --- Dependencies ---
# pip install tenacity psycopg2-binary

# --- Database Setup (if needed) ---
# CREATE TABLE employees (id SERIAL PRIMARY KEY, name VARCHAR(100), department VARCHAR(50));
# INSERT INTO employees (name, department) VALUES ('Alice', 'Engineering'), ('Bob', 'Marketing');

# --- Global Variables ---
connection_pool = None
pool_semaphore = None
# A threading.Event to signal all threads to shut down gracefully.
shutdown_event = threading.Event()


def log_retry_attempt(retry_state):
    """Logs a message before each retry attempt."""
    thread_name = threading.current_thread().name
    logging.warning(
        f"Operation failed. Retrying in {retry_state.next_action.sleep:.2f}s... "
        f"(Attempt {retry_state.attempt_number}) - Reason: {retry_state.outcome.exception()}"
    )


@retry(
    retry=retry_if_exception_type((psycopg2.OperationalError, pool.PoolError, TimeoutError)),
    stop=stop_after_attempt(10), # Reduced attempts for demo clarity
    wait=wait_exponential(multiplier=1, min=1, max=10),
    before_sleep=log_retry_attempt
)
def perform_db_operation_with_retry():
    """
    Acquires a semaphore, gets a connection, and performs a query.
    This entire unit of work is decorated to be resilient.
    """
    got_slot = False
    try:
        # Acquire semaphore with a timeout.
        got_slot = pool_semaphore.acquire(timeout=2)
        conn = None

        if got_slot:
            try:
                conn = connection_pool.getconn()
                with conn.cursor() as cur:
                    cur.execute("SELECT id, name, department FROM employees ORDER BY random() LIMIT 1;")
                    employee = cur.fetchone()
                    if employee:
                        logging.info(f"Fetched -> ID: {employee[0]}, Name: {employee[1]}, Dept: {employee[2]}")
                    else:
                        logging.info("No employees found.")

            except (psycopg2.OperationalError, pool.PoolError) as e:
                if conn:
                    connection_pool.putconn(conn, close=True)
                conn = None
                # Re-raise the exception for tenacity to handle the retry.
                raise e
            finally:
                if conn:
                    connection_pool.putconn(conn)
        else:
            # --- IMPROVEMENT #3: Raise an exception on timeout so tenacity can retry ---
            raise TimeoutError("Semaphore acquisition timed out")

    finally:
        # This block ensures the semaphore is ALWAYS released if it was acquired.
        if got_slot:
            pool_semaphore.release()


def worker_thread_task(thread_id):
    """
    The main loop for each worker thread.
    It continues to perform database operations until a shutdown is signaled.
    """
    logging.info("Starting...")
    # Loop until the main thread sets the shutdown_event.
    while not shutdown_event.is_set():
        try:
            perform_db_operation_with_retry()
        except RetryError as e:
            # This is raised by tenacity if all retries fail for an operation.
            logging.error(f"Operation FAILED permanently after all retries: {e}")
        except Exception as e:
            # Catch any other unexpected errors.
            logging.error(f"An unexpected error occurred: {e}", exc_info=True)

        # Wait for 1 second OR until a shutdown is signaled, whichever comes first.
        # This makes the thread much more responsive to shutdown signals.
        shutdown_event.wait(1)
    logging.info("Shutdown signal received, exiting.")


if __name__ == "__main__":
    logging.info("--- Resilient Psycopg2 Connection Pool Demo ---")

    try:
        max_connections = 2
        num_threads = 50

        logging.info(f"Initializing pool with max {max_connections} connections for {num_threads} threads.")

        connection_pool = psycopg2.pool.ThreadedConnectionPool(
            minconn=2,
            maxconn=max_connections,
            dsn=DSN
        )

        pool_semaphore = threading.Semaphore(max_connections)

        threads = []
        for i in range(num_threads):
            # Naming threads is good practice for debugging.
            thread = threading.Thread(target=worker_thread_task, args=(i + 1,), name=f"Worker-{i+1}")
            threads.append(thread)
            thread.start()

        logging.info(f"Started {num_threads} worker threads. Press Ctrl+C to exit gracefully.")
        
        # --- IMPROVEMENT #4: The main loop now handles KeyboardInterrupt to trigger a graceful shutdown ---
        # Keep the main thread alive.
        while True:
            time.sleep(1)

    except psycopg2.OperationalError as e:
        logging.critical(f"Could not connect to the database: {e}")
        logging.critical("Please check your DSN and ensure the database is running.")
    except KeyboardInterrupt:
        logging.info("Ctrl+C received. Signaling threads to shut down...")
        # Signal all threads to stop their loops.
        shutdown_event.set()
        # Wait for all threads to finish their current task.
        for thread in threads:
            thread.join()
        logging.info("All worker threads have terminated.")
    finally:
        if connection_pool:
            connection_pool.closeall()
            logging.info("All database connections have been closed. Exiting.")