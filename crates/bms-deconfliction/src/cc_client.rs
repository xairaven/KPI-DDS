//! Client of Communication Control's client API (lecturer's format: `PascalCase` JSON).

use bms_common::geo::GeoPoint;
use bms_common::telemetry;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// The operational area: hives must stay inside this circle.
#[derive(Debug, Clone, Copy)]
pub struct Area {
    pub center: GeoPoint,
    pub radius_km: f64,
}

#[derive(Debug, Clone)]
pub struct CcClient {
    http: reqwest::Client,
    base_url: String,
}

#[derive(Deserialize)]
struct AreaResponse {
    #[serde(rename = "Latitude")]
    latitude: f64,
    #[serde(rename = "Longitude")]
    longitude: f64,
    #[serde(rename = "Radius_KM")]
    radius_km: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct MoveRequest<'a> {
    hives: [&'a str; 1],
    destination: Location,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Location {
    latitude: f64,
    longitude: f64,
}

impl CcClient {
    pub fn new(base_url: String) -> reqwest::Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()?,
            base_url,
        })
    }

    /// `GET /api/v1/client/area`.
    #[tracing::instrument(name = "GET /api/v1/client/area", skip_all, fields(otel.kind = "client"))]
    pub async fn area(&self) -> reqwest::Result<Area> {
        let area: AreaResponse = self
            .http
            .get(format!("{}/api/v1/client/area", self.base_url))
            .headers(trace_headers())
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(Area {
            center: GeoPoint {
                latitude: area.latitude,
                longitude: area.longitude,
            },
            radius_km: area.radius_km,
        })
    }

    /// `PATCH /api/v1/client/hive`: sends the hive to the destination.
    ///
    /// Communication Control answers `202 Accepted` before contacting the hive,
    /// so success here does not mean the command was delivered: delivery is
    /// confirmed by the `hive.moved` event.
    #[tracing::instrument(
        name = "PATCH /api/v1/client/hive",
        skip_all,
        fields(otel.kind = "client", hive_id = %hive_id, destination = ?destination)
    )]
    pub async fn move_hive(
        &self, hive_id: &str, destination: GeoPoint,
    ) -> reqwest::Result<()> {
        self.http
            .patch(format!("{}/api/v1/client/hive", self.base_url))
            .headers(trace_headers())
            .json(&MoveRequest {
                hives: [hive_id],
                destination: Location {
                    latitude: destination.latitude,
                    longitude: destination.longitude,
                },
            })
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}

/// W3C `traceparent` header of the current span. Communication Control does not
/// export traces, but any traced server would continue the same trace from it.
fn trace_headers() -> HeaderMap {
    telemetry::current_context()
        .into_iter()
        .filter_map(|(name, value)| {
            Some((
                HeaderName::try_from(name).ok()?,
                HeaderValue::try_from(value).ok()?,
            ))
        })
        .collect()
}
