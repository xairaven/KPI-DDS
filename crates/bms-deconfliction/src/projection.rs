//! Local copies of hive positions and active EW zones, built from events.
//! The saga reads them to plan safe points and to check that hives moved apart.

use bms_common::db;
use bms_common::events::{Envelope, Event, HiveTelemetry, Zone, ZoneId, ZoneRef};
use sqlx::{PgConnection, PgPool};
use tracing::{debug, info};

/// Applies the event unless it was applied before.
pub async fn apply(pool: &PgPool, envelope: &Envelope) -> Result<(), sqlx::Error> {
    let Some(mut tx) = db::begin_event(pool, envelope.event_id).await? else {
        debug!(event_id = %envelope.event_id, "Duplicate event skipped");
        return Ok(());
    };

    match &envelope.event {
        Event::HiveTelemetry(telemetry) => hive_position(&mut tx, telemetry).await?,
        Event::ZoneActivated(zone) => zone_activated(&mut tx, zone).await?,
        Event::ZoneDeactivated(zone) => zone_deactivated(&mut tx, zone).await?,
        Event::ZoneSnapshot(snapshot) => zone_snapshot(&mut tx, &snapshot.zones).await?,
        Event::AlertProximity(alert) => {
            info!(
                alert_id = alert.alert_id,
                hive_id = %alert.hive_id,
                other_hive_id = %alert.other_hive_id,
                distance_m = alert.distance_m.round(),
                "Hives too close, saga not implemented yet"
            );
        },
        // Not bound to this queue, or not needed by the saga.
        _ => {},
    }

    tx.commit().await
}

/// Stores the latest position; older telemetry is ignored.
async fn hive_position(
    db: &mut PgConnection, telemetry: &HiveTelemetry,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO hive_positions (hive_id, latitude, longitude, reported_at)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (hive_id) DO UPDATE SET
             latitude    = EXCLUDED.latitude,
             longitude   = EXCLUDED.longitude,
             reported_at = EXCLUDED.reported_at
         WHERE hive_positions.reported_at < EXCLUDED.reported_at",
    )
    .bind(&telemetry.hive_id)
    .bind(telemetry.location.latitude)
    .bind(telemetry.location.longitude)
    .bind(telemetry.reported_at)
    .execute(db)
    .await?;
    Ok(())
}

async fn zone_activated(db: &mut PgConnection, zone: &Zone) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO ew_zones (zone_id, latitude, longitude, radius_km)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (zone_id) DO NOTHING",
    )
    .bind(zone.zone_id)
    .bind(zone.center.latitude)
    .bind(zone.center.longitude)
    .bind(zone.radius_km)
    .execute(db)
    .await?;
    Ok(())
}

async fn zone_deactivated(
    db: &mut PgConnection, zone: &ZoneRef,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM ew_zones WHERE zone_id = $1")
        .bind(zone.zone_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Makes the local list exactly the snapshot.
async fn zone_snapshot(db: &mut PgConnection, zones: &[Zone]) -> Result<(), sqlx::Error> {
    let ids: Vec<ZoneId> = zones.iter().map(|zone| zone.zone_id).collect();
    sqlx::query("DELETE FROM ew_zones WHERE NOT (zone_id = ANY($1))")
        .bind(&ids)
        .execute(&mut *db)
        .await?;

    for zone in zones {
        zone_activated(&mut *db, zone).await?;
    }
    Ok(())
}
