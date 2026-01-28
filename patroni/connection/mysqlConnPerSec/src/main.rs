use mysql_async::{Opts, Conn};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // --- Configuration ---
    // Format: mysql://user:password@host:port/database
    let database_url = "mysql://root:vagrant@localhost:3306/mysql";
    let num_tasks = 20;            
    let test_duration = Duration::from_secs(10);
    // ---------------------

    // Parse the URL into connection options once
    let opts = Opts::from_url(database_url)?;

    let success_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));
    let start_time = Instant::now();

    println!("Starting MySQL Connection Churn Test...");
    println!("Workers: {}, Duration: {:?}", num_tasks, test_duration);

    let mut handles = Vec::new();

    for _ in 0..num_tasks {
        let success = Arc::clone(&success_count);
        let failure = Arc::clone(&error_count);
        let opts_clone = opts.clone();

        let handle = tokio::spawn(async move {
            let task_start = Instant::now();

            while task_start.elapsed() < test_duration {
                // In MySQL, we create a new connection object for every iteration
                match Conn::new(opts_clone.clone()).await {
                    Ok(conn) => {
                        success.fetch_add(1, Ordering::Relaxed);
                        
                        // Explicitly drop the connection to close the socket
                        drop(conn); 
                    }
                    Err(e) => {
                        failure.fetch_add(1, Ordering::Relaxed);
                        println!("Error: {:?}", e);
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
