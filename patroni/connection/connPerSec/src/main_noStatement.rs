use tokio_postgres::{NoTls, Error};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<(), Error> {
    // --- Configuration ---
    let db_url = "host=/var/run/postgresql user=postgres password=vagrant dbname=postgres";
    //use host=localhost OR host=/var/run/postgresql to switch between TCP and local socket
    let num_tasks = 20;            // Concurrent workers
    let test_duration = Duration::from_secs(60);
    // ---------------------

    let success_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));
    let start_time = Instant::now();

    println!("Starting Connection Churn Test...");
    println!("Threads: {}, Duration: {:?}", num_tasks, test_duration);

    let mut handles = Vec::new();

    for _ in 0..num_tasks {
        let success = Arc::clone(&success_count);
        let failure = Arc::clone(&error_count);
        let url = db_url.to_string();

        let handle = tokio::spawn(async move {
            let task_start = Instant::now();

            while task_start.elapsed() < test_duration {
                // Attempt to connect (TCP + PG Handshake)
                match tokio_postgres::connect(&url, NoTls).await {
                    Ok((client, connection)) => {
                        // Spawn the connection driver to handle the background socket work
                        tokio::spawn(async move {
                            if let Err(_) = connection.await {
                                // Connection closed or failed
                            }
                        });

                        // We successfully reached a 'Ready' state
                        success.fetch_add(1, Ordering::Relaxed);

                        // Explicitly drop the client to initiate a clean disconnect
                        drop(client);
                    }
                    Err(_) => {
                        failure.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        let _ = handle.await;
    }

    let elapsed = start_time.elapsed().as_secs_f64();
    let total_success = success_count.load(Ordering::Relaxed);
    let total_fail = error_count.load(Ordering::Relaxed);

    println!("\n--- Benchmark Results ---");
    println!("Total Elapsed:     {:.2}s", elapsed);
    println!("Successful Conns:  {}", total_success);
    println!("Failed Conns:      {}", total_fail);
    println!("Connections/sec:   {:.2}", total_success as f64 / elapsed);

    Ok(())
}