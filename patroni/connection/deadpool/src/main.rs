// Bring the required items into scope -----------------------------------------
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
// deadpool-postgres types: connection-manager, its config, the pool itself, how to recycle a connection, and which async runtime to use.

use tokio_postgres::{Config, NoTls};  // The underlying PostgreSQL driver: configuration object and no-TLS connector.
use std::sync::Arc;  // Atomic-reference-counted pointer so we can share the pool across many tasks without cloning the whole pool struct.
use tokio::time::{sleep, Duration};  // Async timer utilities: sleep for a specified duration.

// The main entry point. `#[tokio::main]` converts `main` into a Tokio runtime nd makes the function `async`. 
// It returns a boxed generic error.
//#[tokio::main] 
#[tokio::main(flavor = "multi_thread", worker_threads = 8)]  //Uncomment if real OS thread is required
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    //-------------------------  1. Build the pool ------------------------------------------------------
    // Parse a single connection-string into `tokio_postgres::Config`. Every key-value pair (host, port, user, password, …) is now inside `cfg`.
    let cfg: Config = "host=localhost,node0 port=5432 user=postgres password=vagrant dbname=postgres target_session_attrs=read-write application_name=myapp"
        .parse()?;

    // Choose how the pool should “clean” a connection before handing it out again.
    let mgr_config = ManagerConfig {
        recycling_method: RecyclingMethod::Fast, // `Fast` only checks `is_closed()`; 
    };

    // Create a `Manager` that knows how to open/close connections using `cfg` and the chosen TLS mode (`NoTls` here).
    let mgr = Manager::from_config(cfg, NoTls, mgr_config);
    let pool = Pool::builder(mgr)
        .max_size(2)          // <-- Just a pool of 2 would be sufficient for 100 concurrent tasks
        .runtime(Runtime::Tokio1)
        .build()?;


    // Wrap the pool in an `Arc` so every task can hold a cheap clone of the
    // *handle* while the pool itself lives in one shared allocation.
    let pool = Arc::new(pool);

    //--------------------------- 2. Spawn 100 tasks, each looping 1000 times ----------------------------
    // Vector that will hold the `JoinHandle` of every spawned task so we can `.await` them all later.
    let mut handles = Vec::with_capacity(100);

    for task_id in 0..100 {   // Launch 100 identical tasks 
        let pool = Arc::clone(&pool);  // clone only the Arc pointer (cheap)

        // `tokio::spawn` creates a concurrent green thread (task) that runs the provided `async` block independently.
        handles.push(tokio::spawn(async move {
            for iter in 0..1000 {  // Each task performs 1 000 iterations 
                /* ------------- 2a. run query with narrow scope (connection returned here) ------------- */
                let rows = {
                    // Try to obtain a connection from the pool (may wait if all are busy). 
                    let client = match pool.get().await { //`pool.get()` returns `Result<Client, PoolError>`.
                        Ok(c) => c, 
                        Err(e) => { // log and skip this iteration
                            eprintln!("[task {task_id}] pool error: {e}");
                            continue;  // go to next iteration
                        }
                    };

                    // Fire the query. We only care about the rows, so we
                    // return them out of the block (rows)
                    match client.query("SELECT * FROM emp LIMIT 5", &[]).await {
                        Ok(rows) => rows,  // return Vec<Row>
                        Err(e) => {  // log and skip this iteration
                            eprintln!("[task {task_id} iter {iter}] query error: {e}");
                            continue;
                        }
                    }
                }; // <-- client dropped → connection back to pool

                /* ------------- 2. print (no connection held) ----------------------- */
                println!("[task {task_id} iter {iter}] got {} rows", rows.len());
                for r in rows {
                    let id: i64 = r.get(0);
                    let value: String = r.get(1);
                    println!("[task {task_id} iter {iter}] {id} {value}");
                }

                /* ------------- 3. sleep without blocking a connection -------------- */
                //sleep(Duration::from_millis(100)).await;  //For testing additional latency at application side
            }  // end of for-loop (1 000 iterations)
            println!("[task {task_id}] finished 1000 iterations");
        }));
    }

    // 3. Wait for all tasks ----------------------------------------------------
    // Iterate over all join handles and `.await` them. This blocks the main task until the last spawned task completes.
    for h in handles {
        let _ = h.await; // ignore the `Result` of the join itself
    }

    Ok(())  //return void
}


