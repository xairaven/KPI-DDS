//! Monitoring of hives inside EW zones (requirement BMS-5).
//!
//! A hive is inside a zone when its distance to the zone center is at most the
//! zone radius. Entering a zone opens a `zone_entered` alert, leaving it (or the
//! zone being deactivated) resolves the alert. The check runs after every
//! telemetry and after every change of EW zones, inside the same transaction.

use bms_common::geo::GeoPoint;
use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use std::collections::BTreeSet;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Copy)]
struct ActiveZone {
    zone_id: Uuid,
    center: GeoPoint,
    radius_km: f64,
}

/// Re-evaluates the EW zone alerts of one hive (after it moved or reconnected).
pub async fn refresh_hive(
    db: &mut PgConnection, hive_id: &str, at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let zones = active_zones(&mut *db).await?;
    refresh(db, hive_id, &zones, at).await
}

/// Re-evaluates the EW zone alerts of all hives (after the zones changed).
pub async fn refresh_all(
    db: &mut PgConnection, at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let zones = active_zones(&mut *db).await?;
    let hive_ids: Vec<String> = sqlx::query_scalar("SELECT hive_id FROM hives")
        .fetch_all(&mut *db)
        .await?;

    for hive_id in hive_ids {
        refresh(&mut *db, &hive_id, &zones, at).await?;
    }
    Ok(())
}

async fn active_zones(db: &mut PgConnection) -> Result<Vec<ActiveZone>, sqlx::Error> {
    let rows: Vec<(Uuid, f64, f64, f64)> = sqlx::query_as(
        "SELECT zone_id, latitude, longitude, radius_km FROM ew_zones WHERE active_to IS NULL",
    )
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(zone_id, latitude, longitude, radius_km)| ActiveZone {
            zone_id,
            center: GeoPoint {
                latitude,
                longitude,
            },
            radius_km,
        })
        .collect())
}

/// Opens alerts for zones the hive has entered and resolves those it has left.
async fn refresh(
    db: &mut PgConnection, hive_id: &str, zones: &[ActiveZone], at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let position: Option<(Option<f64>, Option<f64>)> =
        sqlx::query_as("SELECT latitude, longitude FROM hives WHERE hive_id = $1")
            .bind(hive_id)
            .fetch_optional(&mut *db)
            .await?;
    let position = match position {
        Some((Some(latitude), Some(longitude))) => Some(GeoPoint {
            latitude,
            longitude,
        }),
        // Unknown hive or position unknown (just reconnected): inside no zone.
        _ => None,
    };

    let inside = zones_containing(position, zones);
    let open: BTreeSet<Uuid> = sqlx::query_scalar(
        "SELECT zone_id FROM alerts
         WHERE hive_id = $1 AND kind = 'zone_entered' AND resolved_at IS NULL",
    )
    .bind(hive_id)
    .fetch_all(&mut *db)
    .await?
    .into_iter()
    .collect();

    for zone_id in inside.difference(&open) {
        sqlx::query(
            "INSERT INTO alerts (kind, hive_id, zone_id, raised_at)
             VALUES ('zone_entered', $1, $2, $3)",
        )
        .bind(hive_id)
        .bind(zone_id)
        .bind(at)
        .execute(&mut *db)
        .await?;
        warn!(hive_id, %zone_id, "Hive entered EW zone");
    }

    for zone_id in open.difference(&inside) {
        sqlx::query(
            "UPDATE alerts SET resolved_at = GREATEST(raised_at, $3)
             WHERE hive_id = $1 AND zone_id = $2
               AND kind = 'zone_entered' AND resolved_at IS NULL",
        )
        .bind(hive_id)
        .bind(zone_id)
        .bind(at)
        .execute(&mut *db)
        .await?;
        info!(hive_id, %zone_id, "Hive left EW zone");
    }
    Ok(())
}

/// Ids of the zones that contain the position.
fn zones_containing(position: Option<GeoPoint>, zones: &[ActiveZone]) -> BTreeSet<Uuid> {
    let Some(position) = position else {
        return BTreeSet::new();
    };
    zones
        .iter()
        .filter(|zone| position.is_within(zone.center, zone.radius_km))
        .map(|zone| zone.zone_id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(id: u128, latitude: f64, longitude: f64, radius_km: f64) -> ActiveZone {
        ActiveZone {
            zone_id: Uuid::from_u128(id),
            center: GeoPoint {
                latitude,
                longitude,
            },
            radius_km,
        }
    }

    #[test]
    fn hive_in_overlapping_zones_is_in_both() {
        let zones = [
            zone(1, 48.70, 38.07, 1.5),
            zone(2, 48.71, 38.07, 1.5),
            zone(3, 48.80, 38.07, 1.5),
        ];
        let position = GeoPoint {
            latitude: 48.705,
            longitude: 38.07,
        };

        let inside = zones_containing(Some(position), &zones);

        assert_eq!(
            inside,
            BTreeSet::from([Uuid::from_u128(1), Uuid::from_u128(2)])
        );
    }

    #[test]
    fn unknown_position_is_in_no_zone() {
        let zones = [zone(1, 48.70, 38.07, 1.5)];
        assert!(zones_containing(None, &zones).is_empty());
    }
}
