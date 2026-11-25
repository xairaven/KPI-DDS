//! HTTP API for the current state of the system (requirement BMS-6).
//!
//! Every route is declared once with `#[utoipa::path]`; `utoipa-axum` builds both
//! the axum router and the OpenAPI document from it, so they cannot disagree.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use tracing::error;
use utoipa::{OpenApi, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use utoipa_swagger_ui::SwaggerUi;
use uuid::Uuid;

#[derive(OpenApi)]
#[openapi(info(
    title = "Battlefield Management System",
    description = "Current state of hives and history of EW zones"
))]
struct ApiDoc;

/// Router with all endpoints under `/api/v1` and Swagger UI under `/swagger`.
pub fn router(pool: PgPool) -> axum::Router {
    let (router, openapi) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .nest(
            "/api/v1",
            OpenApiRouter::new()
                .routes(routes!(list_hives))
                .routes(routes!(get_hive))
                .routes(routes!(list_active_zones))
                .routes(routes!(zone_history)),
        )
        .with_state(pool)
        .split_for_parts();

    router.merge(SwaggerUi::new("/swagger").url("/api-docs/openapi.json", openapi))
}

/// Status decided by BMS (database enum `hive_status`).
#[derive(Debug, Clone, Copy, Serialize, ToSchema, sqlx::Type)]
#[sqlx(type_name = "hive_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum HiveStatus {
    Moving,
    Stopped,
    Inactive,
    Disconnected,
}

/// State reported by the hive itself (database enum `reported_state`).
#[derive(Debug, Clone, Copy, Serialize, ToSchema, sqlx::Type)]
#[sqlx(type_name = "reported_state", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ReportedState {
    Stop,
    Move,
    Error,
    Unknown,
}

/// Current state of one hive.
#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
#[expect(
    clippy::struct_field_names,
    reason = "JSON field is named hive_id, like in events and the database"
)]
pub struct Hive {
    hive_id: String,
    status: HiveStatus,
    /// Unknown (null) until the first telemetry after connecting.
    latitude: Option<f64>,
    longitude: Option<f64>,
    speed: Option<f64>,
    height: Option<f64>,
    reported_state: Option<ReportedState>,
    last_telemetry_at: Option<DateTime<Utc>>,
    last_seen_at: DateTime<Utc>,
    connected_at: DateTime<Utc>,
}

/// EW zone: a circle where radio links are jammed.
#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
#[expect(
    clippy::struct_field_names,
    reason = "JSON field is named zone_id, like in events and the database"
)]
pub struct Zone {
    zone_id: Uuid,
    latitude: f64,
    longitude: f64,
    radius_km: f64,
    active_from: DateTime<Utc>,
    /// Null while the zone is active.
    active_to: Option<DateTime<Utc>>,
    active: bool,
}

// Column lists shared by queries. Macros rather than constants: `concat!` joins them
// with the rest of the query at compile time, so every query stays a string literal
// (sqlx rejects SQL built at run time, to prevent SQL injection).
macro_rules! hive_columns {
    () => {
        "hive_id, status, latitude, longitude, speed, height, \
         reported_state, last_telemetry_at, last_seen_at, connected_at"
    };
}

macro_rules! zone_columns {
    () => {
        "zone_id, latitude, longitude, radius_km, active_from, \
         active_to, active_to IS NULL AS active"
    };
}

/// All hives known to BMS.
#[utoipa::path(
    get,
    path = "/hives",
    tag = "Hives",
    responses((status = 200, description = "All hives", body = Vec<Hive>))
)]
async fn list_hives(State(pool): State<PgPool>) -> Result<Json<Vec<Hive>>, ApiError> {
    let hives = sqlx::query_as(concat!(
        "SELECT ",
        hive_columns!(),
        " FROM hives ORDER BY hive_id"
    ))
    .fetch_all(&pool)
    .await?;
    Ok(Json(hives))
}

/// One hive by its identifier.
#[utoipa::path(
    get,
    path = "/hives/{hive_id}",
    tag = "Hives",
    params(("hive_id" = String, Path, description = "Hive identifier")),
    responses(
        (status = 200, description = "The hive", body = Hive),
        (status = 404, description = "No such hive")
    )
)]
async fn get_hive(
    State(pool): State<PgPool>, Path(hive_id): Path<String>,
) -> Result<Json<Hive>, ApiError> {
    let hive = sqlx::query_as(concat!(
        "SELECT ",
        hive_columns!(),
        " FROM hives WHERE hive_id = $1"
    ))
    .bind(&hive_id)
    .fetch_optional(&pool)
    .await?;
    hive.map(Json).ok_or(ApiError::NotFound)
}

/// EW zones that are active now.
#[utoipa::path(
    get,
    path = "/zones",
    tag = "EW zones",
    responses((status = 200, description = "Active EW zones", body = Vec<Zone>))
)]
async fn list_active_zones(
    State(pool): State<PgPool>,
) -> Result<Json<Vec<Zone>>, ApiError> {
    let zones = sqlx::query_as(concat!(
        "SELECT ",
        zone_columns!(),
        " FROM ew_zones WHERE active_to IS NULL ORDER BY active_from"
    ))
    .fetch_all(&pool)
    .await?;
    Ok(Json(zones))
}

/// Full history of EW zones, newest first.
#[utoipa::path(
    get,
    path = "/zones/history",
    tag = "EW zones",
    responses((status = 200, description = "All EW zones ever seen", body = Vec<Zone>))
)]
async fn zone_history(State(pool): State<PgPool>) -> Result<Json<Vec<Zone>>, ApiError> {
    let zones = sqlx::query_as(concat!(
        "SELECT ",
        zone_columns!(),
        " FROM ew_zones ORDER BY active_from DESC"
    ))
    .fetch_all(&pool)
    .await?;
    Ok(Json(zones))
}

/// Errors returned by handlers, turned into HTTP responses.
#[derive(Debug)]
enum ApiError {
    NotFound,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "Not found").into_response(),
            Self::Database(error) => {
                // Details go to the log, not to the client.
                error!(%error, "Database query failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
            },
        }
    }
}
