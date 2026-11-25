//! PostgreSQL helpers shared by services that own a database.

use sqlx::migrate::{MigrateError, Migrator};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Postgres, Transaction};
use std::time::Duration;
use tracing::{info, warn};
use uuid::Uuid;

/// Connections kept open in the pool; one service instance needs only a few.
const MAX_CONNECTIONS: u32 = 5;
const RETRY_DELAY: Duration = Duration::from_secs(2);

/// Connects to the database, waiting until it is reachable, and applies migrations.
///
/// `migrator` is the service's own `sqlx::migrate!("./migrations")`: the macro embeds
/// the SQL files of the calling crate into its binary at compile time.
pub async fn connect(url: &str, migrator: &Migrator) -> Result<PgPool, MigrateError> {
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

    migrator.run(&pool).await?;
    info!("Database migrations applied");
    Ok(pool)
}

/// Starts a transaction for handling an event, or returns `None` if the event
/// was already handled.
///
/// The event id is recorded in table `processed_events` inside the returned
/// transaction, so "handled" is committed together with the changes the event
/// causes, or not at all (RabbitMQ may deliver an event more than once).
pub async fn begin_event(
    pool: &PgPool, event_id: Uuid,
) -> Result<Option<Transaction<'static, Postgres>>, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let first_time = sqlx::query(
        "INSERT INTO processed_events (event_id) VALUES ($1) ON CONFLICT DO NOTHING",
    )
    .bind(event_id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;

    // Dropping the transaction rolls it back; nothing was changed anyway.
    Ok(first_time.then_some(tx))
}
