//! Deconfliction saga (orchestration): separates two hives that came too close.
//!
//! ```text
//! started ──(engine: plan, send move)──> command_sent ──(hive.moved)──> moving
//!    │                                        │                           │
//!    │                     (telemetry: hives farther apart than separation_m)
//!    │                                        └──────────> completed <────┘
//!    └──(no safe point / deadline expired)──> failed
//! ```
//!
//! Reactions to events (`on_*`) run inside the transaction of the event. Actions
//! towards Communication Control and deadlines are handled by the engine (`run`):
//! the saga is persisted *before* the HTTP command, so after a crash the command
//! is simply sent again (sending a hive to the same point twice changes nothing).
//! Every transition is journaled in `saga_steps` and published as `saga.*`.

use crate::cc_client::{Area, CcClient};
use crate::planner::{self, Zone};
use bms_common::events::{Event, ProximityAlert, SagaStatus, SagaUpdate};
use bms_common::geo::GeoPoint;
use bms_common::outbox;
use sqlx::{PgConnection, PgPool};
use std::time::Duration;
use tokio::time::MissedTickBehavior;
use tracing::{error, info, warn};

/// How often the engine looks for work.
const TICK: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy)]
pub struct Settings {
    /// Distance from the other hive the moved hive is sent to, meters.
    pub safe_distance_m: f64,
    /// Hives farther apart than this are separated, meters.
    pub separation_m: f64,
    /// Time to plan, send the command and get it delivered (`hive.moved`).
    pub command_timeout: Duration,
    /// Time for the hives to move apart once the command was delivered.
    pub separation_timeout: Duration,
}

#[derive(Debug, Clone, sqlx::FromRow)]
#[expect(clippy::struct_field_names, reason = "Named like the database column")]
struct Saga {
    saga_id: i64,
    alert_id: i64,
    hive_id: String,
    other_hive_id: String,
    status: SagaStatus,
    attempt: i16,
    moving_hive_id: Option<String>,
}

macro_rules! saga_columns {
    () => {
        "saga_id, alert_id, hive_id, other_hive_id, status, attempt, moving_hive_id"
    };
}

/// `alert.proximity`: starts a saga unless one of the hives is already in one.
pub async fn on_proximity(
    db: &mut PgConnection, alert: &ProximityAlert, settings: Settings,
) -> Result<(), sqlx::Error> {
    let busy: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sagas WHERE finished_at IS NULL
                          AND (hive_id IN ($1, $2) OR other_hive_id IN ($1, $2)))",
    )
    .bind(&alert.hive_id)
    .bind(&alert.other_hive_id)
    .fetch_one(&mut *db)
    .await?;
    if busy {
        info!(
            alert_id = alert.alert_id,
            "A saga for these hives is already running"
        );
        return Ok(());
    }

    let saga: Option<Saga> = sqlx::query_as(concat!(
        "INSERT INTO sagas (alert_id, hive_id, other_hive_id, status, deadline, started_at)
         VALUES ($1, $2, $3, 'started', now() + make_interval(secs => $4), now())
         ON CONFLICT (alert_id) DO NOTHING
         RETURNING ",
        saga_columns!()
    ))
    .bind(alert.alert_id)
    .bind(&alert.hive_id)
    .bind(&alert.other_hive_id)
    .bind(settings.command_timeout.as_secs_f64())
    .fetch_optional(&mut *db)
    .await?;

    match saga {
        Some(saga) => {
            let note = format!("Hives {:.0} m apart", alert.distance_m);
            record(db, &saga, &note).await
        },
        // The same alert delivered again: its saga already exists.
        None => Ok(()),
    }
}

/// `hive.moved`: the command reached the hive.
pub async fn on_hive_moved(
    db: &mut PgConnection, hive_id: &str, settings: Settings,
) -> Result<(), sqlx::Error> {
    let sagas: Vec<Saga> = sqlx::query_as(concat!(
        "UPDATE sagas SET status = 'moving',
                          deadline = now() + make_interval(secs => $2)
         WHERE status = 'command_sent' AND moving_hive_id = $1
         RETURNING ",
        saga_columns!()
    ))
    .bind(hive_id)
    .bind(settings.separation_timeout.as_secs_f64())
    .fetch_all(&mut *db)
    .await?;

    for saga in &sagas {
        record(db, saga, "Command delivered to the hive").await?;
    }
    Ok(())
}

/// `hive.telemetry`: completes sagas whose hives are now far enough apart.
pub async fn on_telemetry(
    db: &mut PgConnection, hive_id: &str, settings: Settings,
) -> Result<(), sqlx::Error> {
    let sagas: Vec<Saga> = sqlx::query_as(concat!(
        "SELECT ",
        saga_columns!(),
        " FROM sagas WHERE status IN ('command_sent', 'moving')
            AND $1 IN (hive_id, other_hive_id)"
    ))
    .bind(hive_id)
    .fetch_all(&mut *db)
    .await?;

    for saga in &sagas {
        let (Some(a), Some(b)) = (
            position(&mut *db, &saga.hive_id).await?,
            position(&mut *db, &saga.other_hive_id).await?,
        ) else {
            continue;
        };
        let distance_m = a.distance_km(b) * 1000.0;
        if distance_m > settings.separation_m {
            let note = format!("Hives {distance_m:.0} m apart");
            transition(db, saga, SagaStatus::Completed, &note).await?;
        }
    }
    Ok(())
}

/// The engine: sends commands for new sagas and handles expired deadlines, forever.
pub async fn run(pool: PgPool, cc: CcClient, area: Area, settings: Settings) {
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;
        if let Err(error) = tick(&pool, &cc, area, settings).await {
            error!(%error, "Saga engine failed, retrying");
        }
    }
}

async fn tick(
    pool: &PgPool, cc: &CcClient, area: Area, settings: Settings,
) -> Result<(), sqlx::Error> {
    let started: Vec<Saga> = sqlx::query_as(concat!(
        "SELECT ",
        saga_columns!(),
        " FROM sagas WHERE status = 'started' ORDER BY saga_id"
    ))
    .fetch_all(pool)
    .await?;
    for saga in &started {
        start_attempt(pool, cc, area, settings, saga).await?;
    }

    let expired: Vec<Saga> = sqlx::query_as(concat!(
        "SELECT ",
        saga_columns!(),
        " FROM sagas WHERE finished_at IS NULL AND deadline < now() ORDER BY saga_id"
    ))
    .fetch_all(pool)
    .await?;
    for saga in &expired {
        let mut tx = pool.begin().await?;
        let note = format!(
            "Deadline expired in status {:?}; compensation is not implemented yet",
            saga.status
        );
        transition(&mut tx, saga, SagaStatus::Failed, &note).await?;
        tx.commit().await?;
    }
    Ok(())
}

/// Plans a safe point and commands the moving hive to fly there.
async fn start_attempt(
    pool: &PgPool, cc: &CcClient, area: Area, settings: Settings, saga: &Saga,
) -> Result<(), sqlx::Error> {
    // Attempt 1 moves the other hive, plan B (attempt 2) the first one.
    let (moving_id, fixed_id) = if saga.attempt == 1 {
        (&saga.other_hive_id, &saga.hive_id)
    } else {
        (&saga.hive_id, &saga.other_hive_id)
    };

    let mut db = pool.acquire().await?;
    let (Some(moving), Some(fixed)) = (
        position(&mut db, moving_id).await?,
        position(&mut db, fixed_id).await?,
    ) else {
        warn!(
            saga_id = saga.saga_id,
            "Hive positions unknown yet, waiting"
        );
        return Ok(());
    };
    let zones = active_zones(&mut db).await?;
    drop(db);

    let Some(target) =
        planner::safe_point(moving, fixed, settings.safe_distance_m, area, &zones)
    else {
        let mut tx = pool.begin().await?;
        transition(&mut tx, saga, SagaStatus::Failed, "No safe point found").await?;
        return tx.commit().await;
    };

    // The saga is persisted as `started`: if this request succeeds but the service
    // stops before the update below, the next tick sends the same command again.
    if let Err(error) = cc.move_hive(moving_id, target).await {
        warn!(saga_id = saga.saga_id, %error, "Move command failed, retrying");
        return Ok(());
    }

    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE sagas SET moving_hive_id = $2,
                          origin_latitude = $3, origin_longitude = $4,
                          target_latitude = $5, target_longitude = $6,
                          deadline = now() + make_interval(secs => $7)
         WHERE saga_id = $1",
    )
    .bind(saga.saga_id)
    .bind(moving_id)
    .bind(moving.latitude)
    .bind(moving.longitude)
    .bind(target.latitude)
    .bind(target.longitude)
    .bind(settings.command_timeout.as_secs_f64())
    .execute(&mut *tx)
    .await?;

    let saga = Saga {
        moving_hive_id: Some(moving_id.clone()),
        ..saga.clone()
    };
    let note = format!(
        "Hive {moving_id} sent to ({:.5}, {:.5})",
        target.latitude, target.longitude
    );
    transition(&mut tx, &saga, SagaStatus::CommandSent, &note).await?;
    tx.commit().await
}

/// Changes the status, then journals and announces it.
async fn transition(
    db: &mut PgConnection, saga: &Saga, status: SagaStatus, note: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE sagas SET status = $2,
                          finished_at = CASE WHEN $2 IN ('completed', 'failed')
                                             THEN now() END,
                          deadline = CASE WHEN $2 IN ('completed', 'failed')
                                          THEN NULL ELSE deadline END
         WHERE saga_id = $1",
    )
    .bind(saga.saga_id)
    .bind(status)
    .execute(&mut *db)
    .await?;

    let saga = Saga {
        status,
        ..saga.clone()
    };
    record(db, &saga, note).await
}

/// Writes the current status of the saga to the journal and the outbox.
async fn record(
    db: &mut PgConnection, saga: &Saga, note: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO saga_steps (saga_id, status, note, at) VALUES ($1, $2, $3, now())",
    )
    .bind(saga.saga_id)
    .bind(saga.status)
    .bind(note)
    .execute(&mut *db)
    .await?;

    outbox::enqueue(
        db,
        Event::SagaUpdated(SagaUpdate {
            saga_id: saga.saga_id,
            alert_id: saga.alert_id,
            hive_id: saga.hive_id.clone(),
            other_hive_id: saga.other_hive_id.clone(),
            status: saga.status,
            moving_hive_id: saga.moving_hive_id.clone(),
            note: note.to_owned(),
        }),
    )
    .await?;

    info!(saga_id = saga.saga_id, status = ?saga.status, note, "Saga");
    Ok(())
}

async fn position(
    db: &mut PgConnection, hive_id: &str,
) -> Result<Option<GeoPoint>, sqlx::Error> {
    let row: Option<(f64, f64)> = sqlx::query_as(
        "SELECT latitude, longitude FROM hive_positions WHERE hive_id = $1",
    )
    .bind(hive_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(latitude, longitude)| GeoPoint {
        latitude,
        longitude,
    }))
}

async fn active_zones(db: &mut PgConnection) -> Result<Vec<Zone>, sqlx::Error> {
    let rows: Vec<(f64, f64, f64)> =
        sqlx::query_as("SELECT latitude, longitude, radius_km FROM ew_zones")
            .fetch_all(db)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(latitude, longitude, radius_km)| Zone {
            center: GeoPoint {
                latitude,
                longitude,
            },
            radius_km,
        })
        .collect())
}
