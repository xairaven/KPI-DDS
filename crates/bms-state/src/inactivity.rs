//! Detection of hives that went silent.
//!
//! Communication Control does not report a hive that crashed (no ping mechanism),
//! its telemetry simply stops. Such hives are marked `inactive` here; the next
//! telemetry from the hive makes it `moving` or `stopped` again.

use crate::monitoring;
use chrono::Utc;
use sqlx::PgPool;
use std::time::Duration;
use tokio::time::MissedTickBehavior;
use tracing::{error, warn};

/// How often silent hives are looked for.
const CHECK_INTERVAL: Duration = Duration::from_secs(5);

/// Marks hives silent for longer than `timeout` as inactive, forever.
pub async fn run(pool: PgPool, timeout: Duration, settings: monitoring::Settings) {
    let mut ticker = tokio::time::interval(CHECK_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;
        match mark_inactive(&pool, timeout, settings).await {
            Ok(hive_ids) => {
                for hive_id in hive_ids {
                    warn!(%hive_id, ?timeout, "Hive went silent, marked inactive");
                }
            },
            // A failed check is simply repeated on the next tick.
            Err(error) => error!(%error, "Inactivity check failed"),
        }
    }
}

/// Returns ids of the hives that have just become inactive.
async fn mark_inactive(
    pool: &PgPool, timeout: Duration, settings: monitoring::Settings,
) -> Result<Vec<String>, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let hive_ids: Vec<String> = sqlx::query_scalar(
        "UPDATE hives SET status = 'inactive'
         WHERE status IN ('moving', 'stopped')
           AND last_seen_at < now() - make_interval(secs => $1)
         RETURNING hive_id",
    )
    .bind(timeout.as_secs_f64())
    .fetch_all(&mut *tx)
    .await?;

    // An inactive hive takes no part in proximity checks: resolve its alerts.
    let now = Utc::now();
    for hive_id in &hive_ids {
        monitoring::refresh_hive(&mut tx, settings, hive_id, now).await?;
    }

    tx.commit().await?;
    Ok(hive_ids)
}
