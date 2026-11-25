//! Projection: applies events to the current state stored in the database.
//!
//! Every event is applied in one transaction together with remembering its id,
//! so a redelivered event changes nothing (idempotency).

use crate::monitoring;
use bms_common::db;
use bms_common::events::{
    Envelope, Event, HiveRef, HiveTelemetry, ReportedState, Zone, ZoneId, ZoneRef,
};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};
use tracing::{debug, info};

/// Coordinate change (in degrees, about 1 cm) below which a hive counts as not moving.
const MOVEMENT_EPSILON: f64 = 1e-7;

/// Applies the event unless it was applied before.
pub async fn apply(
    pool: &PgPool, settings: monitoring::Settings, envelope: &Envelope,
) -> Result<(), sqlx::Error> {
    let Some(mut tx) = db::begin_event(pool, envelope.event_id).await? else {
        debug!(event_id = %envelope.event_id, "Duplicate event skipped");
        return Ok(());
    };

    let at = envelope.occurred_at;
    match &envelope.event {
        Event::HiveConnected(hive) | Event::HiveReconnected(hive) => {
            hive_connected(&mut tx, hive, at).await?;
            // Position became unknown: the hive leaves all zones and close pairs.
            monitoring::refresh_hive(&mut tx, settings, &hive.hive_id, at).await?;
        },
        Event::HiveDisconnected(hive) => {
            hive_disconnected(&mut tx, hive, at).await?;
            // A disconnected hive takes no part in proximity checks.
            monitoring::refresh_hive(&mut tx, settings, &hive.hive_id, at).await?;
        },
        Event::HiveTelemetry(telemetry) => {
            if hive_telemetry(&mut tx, telemetry, at).await? {
                monitoring::refresh_hive(&mut tx, settings, &telemetry.hive_id, at)
                    .await?;
            }
        },
        // Commands do not change the hive state: movement is seen in telemetry.
        // Alerts are published by this service itself and not consumed by it.
        // Saga updates are not stored yet.
        Event::HiveMoveCommanded(_)
        | Event::HiveStopCommanded(_)
        | Event::AlertZoneEntered(_)
        | Event::AlertProximity(_)
        | Event::SagaUpdated(_) => {},
        // A zone may appear over (or disappear from) hives that do not move.
        Event::ZoneActivated(zone) => {
            zone_activated(&mut tx, zone).await?;
            monitoring::refresh_all_zones(&mut tx, at).await?;
        },
        Event::ZoneDeactivated(zone) => {
            zone_deactivated(&mut tx, zone, at).await?;
            monitoring::refresh_all_zones(&mut tx, at).await?;
        },
        Event::ZoneSnapshot(snapshot) => {
            zone_snapshot(&mut tx, &snapshot.zones, at).await?;
            monitoring::refresh_all_zones(&mut tx, at).await?;
        },
    }

    tx.commit().await
}

/// A (re)connected hive starts over: position unknown until its first telemetry.
async fn hive_connected(
    db: &mut PgConnection, hive: &HiveRef, at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO hives (hive_id, status, last_seen_at, connected_at)
         VALUES ($1, 'stopped', $2, $2)
         ON CONFLICT (hive_id) DO UPDATE SET
             status            = 'stopped',
             latitude          = NULL,
             longitude         = NULL,
             speed             = NULL,
             height            = NULL,
             reported_state    = NULL,
             last_telemetry_at = NULL,
             last_seen_at      = EXCLUDED.last_seen_at,
             connected_at      = EXCLUDED.connected_at",
    )
    .bind(&hive.hive_id)
    .bind(at)
    .execute(db)
    .await?;

    info!(hive_id = %hive.hive_id, "Hive connected");
    Ok(())
}

async fn hive_disconnected(
    db: &mut PgConnection, hive: &HiveRef, at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let updated = sqlx::query(
        "UPDATE hives SET status = 'disconnected', last_seen_at = $2 WHERE hive_id = $1",
    )
    .bind(&hive.hive_id)
    .bind(at)
    .execute(db)
    .await?
    .rows_affected();

    if updated == 0 {
        debug!(hive_id = %hive.hive_id, "Unknown hive disconnected, nothing to update");
    } else {
        info!(hive_id = %hive.hive_id, "Hive disconnected");
    }
    Ok(())
}

/// Stores the latest position. The hive is `moving` if its position changed since
/// the previous telemetry. Telemetry older than the stored one is ignored.
/// Returns whether the state was updated.
async fn hive_telemetry(
    db: &mut PgConnection, telemetry: &HiveTelemetry, at: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    let updated = sqlx::query(
        "INSERT INTO hives (hive_id, status, latitude, longitude, speed, height,
                            reported_state, last_telemetry_at, last_seen_at, connected_at)
         VALUES ($1, 'stopped', $2, $3, $4, $5, $6::reported_state, $7, $8, $8)
         ON CONFLICT (hive_id) DO UPDATE SET
             status = CASE
                 WHEN hives.latitude IS NULL THEN 'stopped'::hive_status
                 WHEN abs(hives.latitude - EXCLUDED.latitude) > $9
                   OR abs(hives.longitude - EXCLUDED.longitude) > $9
                     THEN 'moving'::hive_status
                 ELSE 'stopped'::hive_status
             END,
             latitude          = EXCLUDED.latitude,
             longitude         = EXCLUDED.longitude,
             speed             = EXCLUDED.speed,
             height            = EXCLUDED.height,
             reported_state    = EXCLUDED.reported_state,
             last_telemetry_at = EXCLUDED.last_telemetry_at,
             last_seen_at      = EXCLUDED.last_seen_at
         WHERE hives.last_telemetry_at IS NULL
            OR hives.last_telemetry_at < EXCLUDED.last_telemetry_at",
    )
    .bind(&telemetry.hive_id)
    .bind(telemetry.location.latitude)
    .bind(telemetry.location.longitude)
    .bind(telemetry.speed)
    .bind(telemetry.height)
    .bind(reported_state(telemetry.state))
    .bind(telemetry.reported_at)
    .bind(at)
    .bind(MOVEMENT_EPSILON)
    .execute(db)
    .await?
    .rows_affected();

    if updated == 0 {
        debug!(hive_id = %telemetry.hive_id, "Outdated telemetry ignored");
    }
    Ok(updated == 1)
}

/// Value of the `reported_state` database enum.
const fn reported_state(state: ReportedState) -> &'static str {
    match state {
        ReportedState::Stop => "stop",
        ReportedState::Move => "move",
        ReportedState::Error => "error",
        ReportedState::Unknown => "unknown",
    }
}

/// Records a new zone in the history. Already known zones are left unchanged.
async fn zone_activated(db: &mut PgConnection, zone: &Zone) -> Result<(), sqlx::Error> {
    let inserted = insert_zone(db, zone).await?;
    if inserted {
        info!(zone_id = %zone.zone_id, radius_km = zone.radius_km, "EW zone activated");
    }
    Ok(())
}

/// Closes the zone's activity period.
async fn zone_deactivated(
    db: &mut PgConnection, zone: &ZoneRef, at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let updated = sqlx::query(
        // GREATEST: the end can't precede the start even if clocks of
        // Communication Control and BMS differ slightly.
        "UPDATE ew_zones SET active_to = GREATEST(active_from, $2)
         WHERE zone_id = $1 AND active_to IS NULL",
    )
    .bind(zone.zone_id)
    .bind(at)
    .execute(db)
    .await?
    .rows_affected();

    if updated == 0 {
        debug!(zone_id = %zone.zone_id, "Zone unknown or already inactive");
    } else {
        info!(zone_id = %zone.zone_id, "EW zone deactivated");
    }
    Ok(())
}

/// Reconciles the history with the full list of currently active zones:
/// adds missing zones and closes active ones that are no longer in the list
/// (deleted while `bms-ingest` was not running).
async fn zone_snapshot(
    db: &mut PgConnection, zones: &[Zone], at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let mut added = 0;
    for zone in zones {
        if insert_zone(&mut *db, zone).await? {
            added += 1;
        }
    }

    let active_ids: Vec<ZoneId> = zones.iter().map(|zone| zone.zone_id).collect();
    let closed = sqlx::query(
        "UPDATE ew_zones SET active_to = GREATEST(active_from, $2)
         WHERE active_to IS NULL AND NOT (zone_id = ANY($1))",
    )
    .bind(&active_ids)
    .bind(at)
    .execute(db)
    .await?
    .rows_affected();

    info!(
        active = zones.len(),
        added, closed, "EW zones reconciled with snapshot"
    );
    Ok(())
}

/// Returns whether the zone was new.
async fn insert_zone(db: &mut PgConnection, zone: &Zone) -> Result<bool, sqlx::Error> {
    let inserted = sqlx::query(
        "INSERT INTO ew_zones (zone_id, latitude, longitude, radius_km, active_from)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (zone_id) DO NOTHING",
    )
    .bind(zone.zone_id)
    .bind(zone.center.latitude)
    .bind(zone.center.longitude)
    .bind(zone.radius_km)
    .bind(zone.created_at)
    .execute(db)
    .await?
    .rows_affected();

    Ok(inserted == 1)
}
