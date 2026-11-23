use serde::{Deserialize, Serialize};

/// A point on the Earth surface in degrees (WGS 84).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeoPoint {
    pub latitude: f64,
    pub longitude: f64,
}
