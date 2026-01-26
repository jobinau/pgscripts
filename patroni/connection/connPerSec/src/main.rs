use tokio_postgres::{NoTls, Error};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<(), Error> {
    // --- Configuration ---
    let db_url = "host=localhost user=postgres password=vagrant dbname=postgres";
    let num_tasks = 10; // Number of concurrent workers
    let test_duration = Duration::from_secs(10);
    // ---------------------

    let total_conns = Arc::new(AtomicU64::new(0));
    let start_time = Instant::now();

    println!("Starting benchmark with {} tasks for {:?}...", num_tasks, test_duration);

    let mut handles = Vec::new();

    for _ in 0..num_tasks {
        let counter = Arc::clone(&total_conns);
        let url = db_url.to_string();
        
        let handle = tokio::spawn(async move {
            let task_start = Instant::now();
            while task_start.elapsed() < test_duration {
                // 1. Connect
                let (client, connection) = match tokio_postgres::connect(&url, NoTls).await {
                    Ok(res) => res,
                    Err(_) => continue, // In a stress test, some handshakes may fail
                };

                // The connection object performs the actual handshake and data transfer
                tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        eprintln!("Connection error: {}", e);
                    }
                });

                // 2. Execute a minimal statement
                if client.query("SELECT 1", &[]).await.is_ok() {
                    counter.fetch_add(1, Ordering::Relaxed);
                }

                // 3. Disconnect happens automatically when 'client' is dropped here
            }
        });
        handles.push(handle);
    }

    // Wait for all tasks to complete
    for handle in handles {
        let _ = handle.await;
    }

    let elapsed = start_time.elapsed().as_secs_f64();
    let final_count = total_conns.load(Ordering::Relaxed);

    println!("--- Results ---");
    println!("Total successful connections: {}", final_count);
    println!("Average Connections Per Second (CPS): {:.2}", final_count as f64 / elapsed);

    Ok(())
}
