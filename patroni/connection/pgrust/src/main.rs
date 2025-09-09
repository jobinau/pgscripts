//use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod};
use tokio_postgres::{Config,NoTls};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    //let connect_string = "host=localhost port=5432 user=postgres password=vagrant dbname=postgres";
    //let (client, connection) = tokio_postgres::connect(&connect_string, NoTls).await?;

    let cfg: Config = "host=localhost,node0 port=5432 user=postgres password=vagrant dbname=postgres"
                  .parse()?;
    let (client, connection) = cfg.connect(NoTls).await?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("connection error: {}", e);
        }
    });

    println!("Hello world");
    // Now we can execute a simple statement that just returns its parameter.
    let rows = client.query("SELECT * FROM emp LIMIT 5", &[]).await?;

  for r in rows {
    let id: i64 = r.get(0);
    let value: String = r.get(1);
    println!("{0} {1}", id, value); 
  }
  Ok(())
}