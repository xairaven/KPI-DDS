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

    /// Initial bearing towards another point, degrees clockwise from north (0..360).
    pub fn bearing_to(self, other: Self) -> f64 {
        let (lat1, lat2) = (self.latitude.to_radians(), other.latitude.to_radians());
        let d_lon = (other.longitude - self.longitude).to_radians();

        let y = d_lon.sin() * lat2.cos();
        let x = lat1
            .cos()
            .mul_add(lat2.sin(), -(lat1.sin() * lat2.cos() * d_lon.cos()));
        y.atan2(x).to_degrees().rem_euclid(360.0)
    }

    /// The point `distance_m` away along the great circle with the given bearing.
    #[must_use]
    pub fn offset(self, bearing_deg: f64, distance_m: f64) -> Self {
        let lat1 = self.latitude.to_radians();
        let lon1 = self.longitude.to_radians();
        let bearing = bearing_deg.to_radians();
        // Angular distance.
        let delta = distance_m / 1000.0 / EARTH_RADIUS_KM;

        let lat2 = lat1
            .sin()
            .mul_add(delta.cos(), lat1.cos() * delta.sin() * bearing.cos())
            .asin();
        let lon2 = lon1
            + (bearing.sin() * delta.sin() * lat1.cos())
                .atan2(lat1.sin().mul_add(-lat2.sin(), delta.cos()));

        Self {
            latitude: lat2.to_degrees(),
            longitude: lon2.to_degrees(),
        }
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
    fn bearing_to_cardinal_directions() {
        let p = point(48.7, 38.07);
        assert!(p.bearing_to(point(48.8, 38.07)).abs() < 1e-6); // north
        assert!((p.bearing_to(point(48.6, 38.07)) - 180.0).abs() < 1e-6); // south
        assert!((p.bearing_to(point(48.7, 38.17)) - 90.0).abs() < 0.1); // east
    }

    #[test]
    fn offset_lands_at_the_requested_distance_and_bearing() {
        let start = point(48.7195, 38.0927);
        for bearing in [0.0, 45.0, 135.0, 270.0] {
            let end = start.offset(bearing, 1000.0);
            assert!((start.distance_km(end) - 1.0).abs() < 1e-5);
            assert!((start.bearing_to(end) - bearing).abs() < 0.01);
        }
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
