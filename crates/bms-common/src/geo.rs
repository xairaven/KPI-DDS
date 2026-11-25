use serde::{Deserialize, Serialize};

/// Mean Earth radius (IUGG), km.
const EARTH_RADIUS_KM: f64 = 6371.0088;

/// A point on the Earth surface in degrees (WGS 84).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeoPoint {
    pub latitude: f64,
    pub longitude: f64,
}

impl GeoPoint {
    /// Great-circle distance to another point, km (haversine formula).
    ///
    /// Plain Pythagoras on degrees would be wrong: at latitude 48° a degree of
    /// longitude is about 74 km, while a degree of latitude is about 111 km.
    pub fn distance_km(self, other: Self) -> f64 {
        let (lat1, lat2) = (self.latitude.to_radians(), other.latitude.to_radians());
        let d_lat = lat2 - lat1;
        let d_lon = (other.longitude - self.longitude).to_radians();

        let (sin_lat, sin_lon) = ((d_lat / 2.0).sin(), (d_lon / 2.0).sin());
        // a = sin²(Δφ/2) + cos φ1 · cos φ2 · sin²(Δλ/2)
        let a = sin_lat.mul_add(sin_lat, lat1.cos() * lat2.cos() * sin_lon * sin_lon);
        2.0 * EARTH_RADIUS_KM * a.sqrt().asin()
    }

    /// Whether the point lies inside the circle (border included).
    pub fn is_within(self, center: Self, radius_km: f64) -> bool {
        self.distance_km(center) <= radius_km
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn point(latitude: f64, longitude: f64) -> GeoPoint {
        GeoPoint {
            latitude,
            longitude,
        }
    }

    #[test]
    fn same_point_is_zero_km() {
        let p = point(48.7, 38.07);
        assert!(p.distance_km(p).abs() < 1e-9);
    }

    #[test]
    fn one_degree_of_latitude_is_about_111_km() {
        let d = point(48.0, 38.0).distance_km(point(49.0, 38.0));
        assert!((d - 111.19).abs() < 0.01, "got {d}");
    }

    #[test]
    fn one_degree_of_longitude_is_shorter_at_48_degrees() {
        let d = point(48.0, 38.0).distance_km(point(48.0, 39.0));
        assert!((d - 74.40).abs() < 0.01, "got {d}");
    }

    #[test]
    fn distance_is_symmetric() {
        let (a, b) = (point(48.7195, 38.0927), point(48.68, 38.04));
        assert!((a.distance_km(b) - b.distance_km(a)).abs() < 1e-9);
    }

    #[test]
    fn point_inside_and_outside_a_zone() {
        let center = point(48.70, 38.07);
        // About 1.1 km north of the center.
        let p = point(48.71, 38.07);
        assert!(p.is_within(center, 1.5));
        assert!(!p.is_within(center, 1.0));
    }
}
