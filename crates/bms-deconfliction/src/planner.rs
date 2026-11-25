//! Choice of a safe point for the hive being moved apart.

use crate::cc_client::Area;
use bms_common::geo::GeoPoint;

/// An active EW zone, as stored in the local copy.
#[derive(Debug, Clone, Copy)]
pub struct Zone {
    pub center: GeoPoint,
    pub radius_km: f64,
}

/// Step between tried directions, degrees.
const ANGLE_STEP_DEG: f64 = 30.0;
/// Steps up to the opposite direction: 6 * 30° = 180°.
const ANGLE_STEPS: i32 = 6;
/// Below this distance the hives are treated as being at one point.
const SAME_POINT_M: f64 = 1.0;

/// A point `safe_distance_m` away from `other`, preferably straight away from it
/// (through `moving`), inside the operational area and outside every EW zone.
///
/// If the preferred direction is not allowed, directions rotated by ±30°, ±60°, ...
/// up to the opposite one are tried. `None` if no direction works.
pub fn safe_point(
    moving: GeoPoint, other: GeoPoint, safe_distance_m: f64, area: Area, zones: &[Zone],
) -> Option<GeoPoint> {
    // Straight away from the other hive; north if both are at the same point.
    let away = if other.distance_km(moving) * 1000.0 < SAME_POINT_M {
        0.0
    } else {
        other.bearing_to(moving)
    };

    (0..=ANGLE_STEPS)
        .flat_map(|step| {
            let turn = f64::from(step) * ANGLE_STEP_DEG;
            // 0 and 180 degrees once, every other turn to both sides.
            if step == 0 || step == ANGLE_STEPS {
                vec![turn]
            } else {
                vec![turn, -turn]
            }
        })
        .map(|turn| other.offset(away + turn, safe_distance_m))
        .find(|candidate| {
            candidate.is_within(area.center, area.radius_km)
                && zones
                    .iter()
                    .all(|zone| !candidate.is_within(zone.center, zone.radius_km))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Area = Area {
        center: GeoPoint {
            latitude: 48.697_19,
            longitude: 38.066_246,
        },
        radius_km: 5.0,
    };

    const OTHER: GeoPoint = GeoPoint {
        latitude: 48.70,
        longitude: 38.07,
    };

    #[test]
    fn moves_straight_away_from_the_other_hive() {
        // The moving hive is 300 m north of the other one.
        let moving = OTHER.offset(0.0, 300.0);

        let point = safe_point(moving, OTHER, 1000.0, AREA, &[]).unwrap();

        assert!((OTHER.distance_km(point) - 1.0).abs() < 1e-6);
        assert!(OTHER.bearing_to(point).abs() < 1e-3);
    }

    #[test]
    fn turns_around_an_ew_zone() {
        let moving = OTHER.offset(0.0, 300.0);
        // A zone right where the straight-away point would be.
        let zone = Zone {
            center: OTHER.offset(0.0, 1000.0),
            radius_km: 0.3,
        };

        let point = safe_point(moving, OTHER, 1000.0, AREA, &[zone]).unwrap();

        assert!(!point.is_within(zone.center, zone.radius_km));
        assert!((OTHER.distance_km(point) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn stays_inside_the_operational_area() {
        // Hives near the northern edge of the area: north would leave it.
        let other = AREA.center.offset(0.0, 4800.0);
        let moving = other.offset(0.0, 100.0);

        let point = safe_point(moving, other, 1000.0, AREA, &[]).unwrap();

        assert!(point.is_within(AREA.center, AREA.radius_km));
    }

    #[test]
    fn no_point_when_everything_is_blocked() {
        let moving = OTHER.offset(0.0, 300.0);
        let zone = Zone {
            center: OTHER,
            radius_km: 2.0,
        };

        assert!(safe_point(moving, OTHER, 1000.0, AREA, &[zone]).is_none());
    }
}
