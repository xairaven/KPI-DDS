//! Transactional outbox.
//!
//! An event about a state change must reach RabbitMQ exactly when the change is
//! committed. Publishing directly can't guarantee that: the database and the broker
//! have no common transaction ("dual write"). So the event is written to the
//! `outbox` table in the same transaction as the change, and `relay` publishes
//! unpublished rows afterwards. A crash between publishing and marking a row
//! publishes it again with the same `event_id`, which consumers recognize.

use crate::broker::{PublishError, Publisher};
use crate::events::{Envelope, Event};
use sqlx::types::Json;
use sqlx::{PgConnection, PgPool};
use std::time::Duration;
use thiserror::Error;
use tracing::{error, info};

/// Rows published per round.
const BATCH: i64 = 100;
/// Pause when the outbox is empty.
const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Pause after a failed round.
const RETRY_DELAY: Duration = Duration::from_secs(2);

#[derive(Debug, Error)]
enum RelayError {
    #[error("database: {0}")]
    Database(#[from] sqlx::Error),
    #[error("{0}")]
    Publish(#[from] PublishError),
}

/// Records an event to be published once the current transaction commits.
pub async fn enqueue(db: &mut PgConnection, event: Event) -> Result<(), sqlx::Error> {
    let envelope = Envelope::new(event);
    sqlx::query("INSERT INTO outbox (routing_key, envelope) VALUES ($1, $2)")
        .bind(envelope.routing_key())
        .bind(Json(&envelope))
        .execute(db)
        .await?;
    Ok(())
}

/// Publishes outbox rows forever, in insertion order.
pub async fn relay(pool: PgPool, mut publisher: Publisher) {
    loop {
        match relay_batch(&pool, &mut publisher).await {
            // Nothing to do: wait a bit. Otherwise continue at once with the next batch.
            Ok(0) => tokio::time::sleep(POLL_INTERVAL).await,
            Ok(_) => {},
            Err(error) => {
                error!(%error, "Outbox relay failed, retrying");
                tokio::time::sleep(RETRY_DELAY).await;
            },
        }
    }
}

async fn relay_batch(
    pool: &PgPool, publisher: &mut Publisher,
) -> Result<usize, RelayError> {
    let rows: Vec<(i64, Json<Envelope>)> = sqlx::query_as(
        "SELECT id, envelope FROM outbox WHERE published_at IS NULL ORDER BY id LIMIT $1",
    )
    .bind(BATCH)
    .fetch_all(pool)
    .await?;

    for (id, Json(envelope)) in &rows {
        publisher.publish(envelope).await?;
        sqlx::query("UPDATE outbox SET published_at = now() WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        info!(
            routing_key = envelope.routing_key(),
            event_id = %envelope.event_id,
            "Published"
        );
    }
    Ok(rows.len())
}
