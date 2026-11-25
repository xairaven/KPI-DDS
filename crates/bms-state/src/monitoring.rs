//! Monitoring: hives inside EW zones (requirement BMS-5) and hives too close to
//! each other.
//!
//! - A hive is inside a zone when its distance to the zone center is at most the
//!   zone radius. Entering opens a `zone_entered` alert, leaving resolves it.
//! - Two active hives closer than the proximity threshold get a `proximity` alert.
//!   It is resolved only when they are farther apart than the threshold times
//!   `RESOLVE_FACTOR` (hysteresis), so hives drifting around the threshold do not
//!   open and resolve alerts every few seconds.
//!
//! Checks run inside the transaction of the event that triggered them; new alerts
//! are announced through the transactional outbox.

use bms_common::events::{Event, ProximityAlert, ZoneEnteredAlert};
use bms_common::geo::GeoPoint;
use bms_common::outbox;
use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use std::collections::{BTreeMap, BTreeSet};
use tracing::{info, warn};
use uuid::Uuid;

/// A proximity alert is resolved above `threshold * RESOLVE_FACTOR`.
const RESOLVE_FACTOR: f64 = 1.2;

#[derive(Debug, Clone, Copy)]
pub struct Settings {
    /// Hives closer than this (meters) are too close.
    pub proximity_threshold_m: f64,
}

#[derive(Debug, Clone, Copy)]
struct ActiveZone {
    zone_id: Uuid,
    center: GeoPoint,
    radius_km: f64,
}

/// Re-evaluates the alerts of one hive (after it moved, reconnected, went silent).
pub async fn refresh_hive(
    db: &mut PgConnection, settings: Settings, hive_id: &str, at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let zones = active_zones(&mut *db).await?;
    refresh_zones(&mut *db, hive_id, &zones, at).await?;
    refresh_proximity(db, settings, hive_id, at).await
}

/// Re-evaluates the EW zone alerts of all hives (after the zones changed).
pub async fn refresh_all_zones(
    db: &mut PgConnection, at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let zones = active_zones(&mut *db).await?;
    let hive_ids: Vec<String> = sqlx::query_scalar("SELECT hive_id FROM hives")
        .fetch_all(&mut *db)
        .await?;

    for hive_id in hive_ids {
        refresh_zones(&mut *db, &hive_id, &zones, at).await?;
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
async fn refresh_zones(
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

    for &zone_id in inside.difference(&open) {
        let alert_id: i64 = sqlx::query_scalar(
            "INSERT INTO alerts (kind, hive_id, zone_id, raised_at)
             VALUES ('zone_entered', $1, $2, $3)
             RETURNING alert_id",
        )
        .bind(hive_id)
        .bind(zone_id)
        .bind(at)
        .fetch_one(&mut *db)
        .await?;

        outbox::enqueue(
            &mut *db,
            Event::AlertZoneEntered(ZoneEnteredAlert {
                alert_id,
                hive_id: hive_id.to_owned(),
                zone_id,
            }),
        )
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

/// Opens alerts for hives that came too close to this one and resolves alerts
/// for hives that moved away (or for any pair where a hive is no longer active).
async fn refresh_proximity(
    db: &mut PgConnection, settings: Settings, hive_id: &str, at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    // Positions of active hives; inactive and disconnected hives take no part.
    let positions: BTreeMap<String, GeoPoint> = sqlx::query_as::<_, (String, f64, f64)>(
        "SELECT hive_id, latitude, longitude FROM hives
         WHERE status IN ('moving', 'stopped')
           AND latitude IS NOT NULL AND longitude IS NOT NULL",
    )
    .fetch_all(&mut *db)
    .await?
    .into_iter()
    .map(|(id, latitude, longitude)| {
        (
            id,
            GeoPoint {
                latitude,
                longitude,
            },
        )
    })
    .collect();

    // Open proximity alerts involving this hive: other hive -> alert id.
    let open: BTreeMap<String, i64> = sqlx::query_as::<_, (i64, String, String)>(
        "SELECT alert_id, hive_id, other_hive_id FROM alerts
         WHERE kind = 'proximity' AND resolved_at IS NULL
           AND (hive_id = $1 OR other_hive_id = $1)",
    )
    .bind(hive_id)
    .fetch_all(&mut *db)
    .await?
    .into_iter()
    .map(|(alert_id, a, b)| (if a == hive_id { b } else { a }, alert_id))
    .collect();

    let changes = proximity_changes(hive_id, &positions, &open, settings);

    for alert_id in changes.resolve {
        sqlx::query(
            "UPDATE alerts SET resolved_at = GREATEST(raised_at, $2) WHERE alert_id = $1",
        )
        .bind(alert_id)
        .bind(at)
        .execute(&mut *db)
        .await?;
        info!(alert_id, hive_id, "Hives are no longer too close");
    }

    for close in changes.open {
        let alert_id: i64 = sqlx::query_scalar(
            "INSERT INTO alerts (kind, hive_id, other_hive_id, distance_m, raised_at)
             VALUES ('proximity', $1, $2, $3, $4)
             RETURNING alert_id",
        )
        .bind(&close.hive_id)
        .bind(&close.other_hive_id)
        .bind(close.distance_m)
        .bind(at)
        .fetch_one(&mut *db)
        .await?;

        warn!(
            alert_id,
            hive_id = %close.hive_id,
            other_hive_id = %close.other_hive_id,
            distance_m = close.distance_m.round(),
            "Hives too close"
        );
        outbox::enqueue(
            &mut *db,
            Event::AlertProximity(ProximityAlert { alert_id, ..close }),
        )
        .await?;
    }
    Ok(())
}

#[derive(Debug, Default, PartialEq)]
struct ProximityChanges {
    /// New close pairs (`alert_id` is filled in after the insert).
    open: Vec<ProximityAlert>,
    /// Alerts to resolve.
    resolve: Vec<i64>,
}

/// Decides which proximity alerts of `hive_id` to open and which to resolve.
fn proximity_changes(
    hive_id: &str, positions: &BTreeMap<String, GeoPoint>, open: &BTreeMap<String, i64>,
    settings: Settings,
) -> ProximityChanges {
    let me = positions.get(hive_id).copied();
    let distance_m = |other: &str| -> Option<f64> {
        Some(me?.distance_km(*positions.get(other)?) * 1000.0)
    };

    let resolve = open
        .iter()
        .filter(|(other, _)| {
            distance_m(other)
                .is_none_or(|d| d > settings.proximity_threshold_m * RESOLVE_FACTOR)
        })
        .map(|(_, &alert_id)| alert_id)
        .collect();

    let Some(me) = me else {
        return ProximityChanges {
            open: Vec::new(),
            resolve,
        };
    };

    let open = positions
        .iter()
        .filter(|(other, _)| other.as_str() != hive_id && !open.contains_key(*other))
        .filter_map(|(other, &other_position)| {
            let distance_m = me.distance_km(other_position) * 1000.0;
            (distance_m < settings.proximity_threshold_m).then(|| {
                // Pairs are stored ordered, so (1, 2) and (2, 1) are one alert.
                let (first, second) = if hive_id < other.as_str() {
                    ((hive_id, me), (other.as_str(), other_position))
                } else {
                    ((other.as_str(), other_position), (hive_id, me))
                };
                ProximityAlert {
                    alert_id: 0,
                    hive_id: first.0.to_owned(),
                    hive_location: first.1,
                    other_hive_id: second.0.to_owned(),
                    other_location: second.1,
                    distance_m,
                }
            })
        })
        .collect();

    ProximityChanges { open, resolve }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTINGS: Settings = Settings {
        proximity_threshold_m: 500.0,
    };

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

    /// A position `meters_north` north of a fixed point (1° of latitude ≈ 111.19 km).
    fn north_of_origin(meters_north: f64) -> GeoPoint {
        GeoPoint {
            latitude: 48.7 + meters_north / 111_195.0,
            longitude: 38.07,
        }
    }

    fn positions(list: &[(&str, f64)]) -> BTreeMap<String, GeoPoint> {
        list.iter()
            .map(|&(id, meters)| (id.to_owned(), north_of_origin(meters)))
            .collect()
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

    #[test]
    fn close_pair_is_opened_once_and_ordered() {
        let positions = positions(&[("2", 0.0), ("1", 300.0), ("3", 5000.0)]);

        let changes = proximity_changes("2", &positions, &BTreeMap::new(), SETTINGS);

        assert_eq!(changes.resolve, Vec::<i64>::new());
        assert_eq!(changes.open.len(), 1);
        let alert = &changes.open[0];
        assert_eq!(
            (alert.hive_id.as_str(), alert.other_hive_id.as_str()),
            ("1", "2")
        );
        assert!((alert.distance_m - 300.0).abs() < 1.0);
    }

    #[test]
    fn already_open_pair_is_not_opened_again() {
        let positions = positions(&[("1", 0.0), ("2", 300.0)]);
        let open = BTreeMap::from([("2".to_owned(), 7)]);

        let changes = proximity_changes("1", &positions, &open, SETTINGS);

        assert_eq!(changes, ProximityChanges::default());
    }

    #[test]
    fn hysteresis_keeps_the_alert_open_slightly_above_threshold() {
        let open = BTreeMap::from([("2".to_owned(), 7)]);

        // 550 m: above the threshold (500 m) but below 500 * 1.2 = 600 m.
        let near = proximity_changes(
            "1",
            &positions(&[("1", 0.0), ("2", 550.0)]),
            &open,
            SETTINGS,
        );
        // 650 m: beyond the resolve distance.
        let far = proximity_changes(
            "1",
            &positions(&[("1", 0.0), ("2", 650.0)]),
            &open,
            SETTINGS,
        );

        assert_eq!(near.resolve, Vec::<i64>::new());
        assert_eq!(far.resolve, vec![7]);
    }

    #[test]
    fn inactive_hive_resolves_its_alerts() {
        // Hive 2 is not among the active positions any more.
        let open = BTreeMap::from([("2".to_owned(), 7)]);

        let changes = proximity_changes("1", &positions(&[("1", 0.0)]), &open, SETTINGS);

        assert_eq!(changes.resolve, vec![7]);
    }
}
