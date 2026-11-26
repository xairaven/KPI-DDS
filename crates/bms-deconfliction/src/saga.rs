//! Deconfliction saga (orchestration): separates two hives that came too close.
//!
//! ```text
//! started ──(engine: plan, send move)──> command_sent ──(hive.moved)──> moving
//!    │                                        │                           │
//!    │                     (telemetry: hives farther apart than separation_m)
//!    │                                        └──────────> completed <────┘
//!    │                                        │                           │
//!    │                               (deadline expired)          (deadline expired)
//!    │                                        └──────> compensating <─────┘
//!    │                                     (send the moved hive back to its origin)
//!    │                                                      │
//!    └──(deadline expired / no safe point)──┬───────────────┘
//!                                           ├─ attempt 1: plan B ──> started (attempt 2)
//!                                           └─ attempt 2: ─────────> failed
//! ```
//!
//! Attempt 1 moves one hive; plan B (attempt 2) moves the other one.
//!
//! Reactions to events (`on_*`) run inside the transaction of the event. Actions
//! towards Communication Control and deadlines are handled by the engine (`run`):
//! the saga is persisted *before* each HTTP command, so after a crash the command
//! is simply sent again (sending a hive to the same point twice changes nothing).
//! Every transition is journaled in `saga_steps` and published as `saga.*`.

use crate::cc_client::{Area, CcClient};
use crate::planner::{self, Zone};
use bms_common::events::{Event, ProximityAlert, SagaStatus, SagaUpdate};
use bms_common::geo::GeoPoint;
use bms_common::outbox;
use bms_common::telemetry::{self, TraceContext};
use sqlx::types::Json;
use sqlx::{PgConnection, PgPool};
use std::time::Duration;
use tokio::time::MissedTickBehavior;
use tracing::{Instrument, Span, error, info, info_span, warn};

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
    trace_context: Json<TraceContext>,
}

macro_rules! saga_columns {
    () => {
        "saga_id, alert_id, hive_id, other_hive_id, status, attempt, moving_hive_id,
         trace_context"
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
        "INSERT INTO sagas (alert_id, hive_id, other_hive_id, status, deadline, started_at,
                            trace_context)
         VALUES ($1, $2, $3, 'started', now() + make_interval(secs => $4), now(), $5)
         ON CONFLICT (alert_id) DO NOTHING
         RETURNING ",
        saga_columns!()
    ))
    .bind(alert.alert_id)
    .bind(&alert.hive_id)
    .bind(&alert.other_hive_id)
    .bind(settings.command_timeout.as_secs_f64())
    // The current span handles the alert: the engine's steps will continue its trace.
    .bind(Json(telemetry::current_context()))
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
        start_attempt(pool, cc, area, settings, saga)
            .instrument(saga_span("saga start attempt", saga))
            .await?;
    }

    let compensating: Vec<Saga> = sqlx::query_as(concat!(
        "SELECT ",
        saga_columns!(),
        " FROM sagas WHERE status = 'compensating' AND NOT compensation_sent
          ORDER BY saga_id"
    ))
    .fetch_all(pool)
    .await?;
    for saga in &compensating {
        compensate(pool, cc, settings, saga)
            .instrument(saga_span("saga compensate", saga))
            .await?;
    }

    let expired: Vec<Saga> = sqlx::query_as(concat!(
        "SELECT ",
        saga_columns!(),
        " FROM sagas WHERE finished_at IS NULL AND deadline < now() ORDER BY saga_id"
    ))
    .fetch_all(pool)
    .await?;
    for saga in &expired {
        async {
            let mut tx = pool.begin().await?;
            on_deadline(&mut tx, saga, settings).await?;
            tx.commit().await
        }
        .instrument(saga_span("saga deadline expired", saga))
        .await?;
    }
    Ok(())
}

/// A span for one engine step, in the trace of the alert that started the saga.
fn saga_span(name: &'static str, saga: &Saga) -> Span {
    let span = info_span!(
        "saga step",
        otel.name = name,
        saga_id = saga.saga_id,
        attempt = saga.attempt,
        status = ?saga.status,
    );
    telemetry::continue_trace(&span, &saga.trace_context);
    span
}

/// The current step did not finish in time.
async fn on_deadline(
    db: &mut PgConnection, saga: &Saga, settings: Settings,
) -> Result<(), sqlx::Error> {
    match saga.status {
        // No command was sent, so there is nothing to undo.
        SagaStatus::Started => {
            next_attempt_or_fail(
                db,
                saga,
                settings,
                "Command not sent before the deadline",
            )
            .await
        },
        // The hive may have received the command: undo it first.
        SagaStatus::CommandSent | SagaStatus::Moving => {
            sqlx::query(
                "UPDATE sagas SET deadline = now() + make_interval(secs => $2)
                 WHERE saga_id = $1",
            )
            .bind(saga.saga_id)
            .bind(settings.command_timeout.as_secs_f64())
            .execute(&mut *db)
            .await?;

            let hive = saga.moving_hive_id.as_deref().unwrap_or("?");
            let note = format!(
                "Deadline expired in status {:?}; sending hive {hive} back",
                saga.status
            );
            transition(db, saga, SagaStatus::Compensating, &note).await
        },
        SagaStatus::Compensating => {
            next_attempt_or_fail(
                db,
                saga,
                settings,
                "Return command not sent before the deadline",
            )
            .await
        },
        SagaStatus::Completed | SagaStatus::Failed => Ok(()),
    }
}

/// Compensating action: sends the moved hive back to where it was before the command.
async fn compensate(
    pool: &PgPool, cc: &CcClient, settings: Settings, saga: &Saga,
) -> Result<(), sqlx::Error> {
    let origin: Option<(String, f64, f64)> = sqlx::query_as(
        "SELECT moving_hive_id, origin_latitude, origin_longitude FROM sagas
         WHERE saga_id = $1 AND moving_hive_id IS NOT NULL
           AND origin_latitude IS NOT NULL AND origin_longitude IS NOT NULL",
    )
    .bind(saga.saga_id)
    .fetch_optional(pool)
    .await?;

    let note = if let Some((hive_id, latitude, longitude)) = origin {
        let origin = GeoPoint {
            latitude,
            longitude,
        };
        // Same rule as for the main command: if this succeeds but the service stops
        // before the update below, the return command is sent again.
        if let Err(error) = cc.move_hive(&hive_id, origin).await {
            warn!(saga_id = saga.saga_id, %error, "Return command failed, retrying");
            return Ok(());
        }
        format!("Hive {hive_id} sent back to ({latitude:.5}, {longitude:.5})")
    } else {
        "Nothing to send back".to_owned()
    };

    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE sagas SET compensation_sent = true WHERE saga_id = $1")
        .bind(saga.saga_id)
        .execute(&mut *tx)
        .await?;
    next_attempt_or_fail(&mut tx, saga, settings, &note).await?;
    tx.commit().await
}

/// After a failed attempt: plan B (move the other hive), or give up after it.
async fn next_attempt_or_fail(
    db: &mut PgConnection, saga: &Saga, settings: Settings, note: &str,
) -> Result<(), sqlx::Error> {
    if saga.attempt >= 2 {
        let note = format!("{note}; both attempts failed, the operator must act");
        return transition(db, saga, SagaStatus::Failed, &note).await;
    }

    sqlx::query(
        "UPDATE sagas SET attempt = 2, status = 'started',
                          moving_hive_id = NULL,
                          origin_latitude = NULL, origin_longitude = NULL,
                          target_latitude = NULL, target_longitude = NULL,
                          compensation_sent = false,
                          deadline = now() + make_interval(secs => $2)
         WHERE saga_id = $1",
    )
    .bind(saga.saga_id)
    .bind(settings.command_timeout.as_secs_f64())
    .execute(&mut *db)
    .await?;

    let saga = Saga {
        attempt: 2,
        status: SagaStatus::Started,
        moving_hive_id: None,
        ..saga.clone()
    };
    let note = format!("{note}; plan B: moving hive {}", saga.hive_id);
    record(db, &saga, &note).await
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
        // Nothing was sent yet; maybe the other hive has somewhere to go.
        let mut tx = pool.begin().await?;
        next_attempt_or_fail(&mut tx, saga, settings, "No safe point found").await?;
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
