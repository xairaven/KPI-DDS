//! Connection to the service's own PostgreSQL database.

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;
use tracing::{info, warn};

/// Connections kept open in the pool; one service instance needs only a few.
const MAX_CONNECTIONS: u32 = 5;
const RETRY_DELAY: Duration = Duration::from_secs(2);

/// Connects to the database, waiting until it is reachable, and applies migrations.
pub async fn connect(url: &str) -> Result<PgPool, sqlx::migrate::MigrateError> {
    let pool = loop {
        match PgPoolOptions::new()
            .max_connections(MAX_CONNECTIONS)
            .connect(url)
            .await
        {
            Ok(pool) => break pool,
            Err(error) => {
                warn!(%error, delay = ?RETRY_DELAY, "Database unavailable, retrying");
                tokio::time::sleep(RETRY_DELAY).await;
            },
        }
    };
    info!("Connected to database");

    // Migrations are embedded into the binary at compile time from ./migrations.
    sqlx::migrate!("./migrations").run(&pool).await?;
    info!("Database migrations applied");
    Ok(pool)
}
