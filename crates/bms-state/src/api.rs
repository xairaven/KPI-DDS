//! HTTP API for the current state of the system (requirement BMS-6).
//!
//! Every route is declared once with `#[utoipa::path]`; `utoipa-axum` builds both
//! the axum router and the OpenAPI document from it, so they cannot disagree.

use crate::auth::{self, ApiTokens};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::response::{IntoResponse, Response};
use bms_common::events::SagaStatus;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use tracing::error;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use utoipa_swagger_ui::SwaggerUi;
use uuid::Uuid;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Battlefield Management System",
        description = "Current state of hives and history of EW zones. \
                       Every endpoint requires a bearer token (Authorize button)."
    ),
    modifiers(&BearerAuth),
    security(("bearer" = []))
)]
struct ApiDoc;

/// Declares the bearer token scheme in the OpenAPI document, so Swagger UI shows
/// the Authorize button and sends the token with every request.
struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        openapi
            .components
            .get_or_insert_default()
            .add_security_scheme(
                "bearer",
                SecurityScheme::Http(
                    HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build(),
                ),
            );
    }
}

/// Router with all endpoints under `/api/v1` (token required) and Swagger UI under
/// `/swagger` (public).
pub fn router(pool: PgPool, tokens: ApiTokens) -> axum::Router {
    let (router, openapi) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .nest(
            "/api/v1",
            OpenApiRouter::new()
                .routes(routes!(list_hives))
                .routes(routes!(get_hive))
                .routes(routes!(list_active_zones))
                .routes(routes!(zone_history))
                .routes(routes!(list_open_alerts))
                .routes(routes!(list_sagas)),
        )
        .with_state(pool)
        .split_for_parts();

    // The layer wraps only the routes added so far: Swagger UI, merged after it,
    // stays public.
    router
        .layer(middleware::from_fn_with_state(tokens, auth::require_token))
        .merge(SwaggerUi::new("/swagger").url("/api-docs/openapi.json", openapi))
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
    /// Active EW zones the hive is inside now (BMS-2, BMS-5); empty if none.
    ew_zone_ids: Vec<Uuid>,
}

/// Kind of a monitoring alert (database enum `alert_kind`).
#[derive(Debug, Clone, Copy, Serialize, ToSchema, sqlx::Type)]
#[sqlx(type_name = "alert_kind", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum AlertKind {
    ZoneEntered,
    Proximity,
}

/// Monitoring alert; open while `resolved_at` is null.
#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
#[expect(
    clippy::struct_field_names,
    reason = "JSON field is named alert_id, like in the database"
)]
pub struct Alert {
    alert_id: i64,
    kind: AlertKind,
    hive_id: String,
    /// For `zone_entered`: the zone.
    zone_id: Option<Uuid>,
    /// For `proximity`: the other hive and the distance to it.
    other_hive_id: Option<String>,
    distance_m: Option<f64>,
    raised_at: DateTime<Utc>,
    resolved_at: Option<DateTime<Utc>>,
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
         reported_state, last_telemetry_at, last_seen_at, connected_at, \
         ARRAY(SELECT a.zone_id FROM alerts a \
               WHERE a.hive_id = hives.hive_id AND a.kind = 'zone_entered' \
                 AND a.resolved_at IS NULL \
               ORDER BY a.zone_id) AS ew_zone_ids"
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

/// Deconfliction saga, as last reported by `bms-deconfliction`.
#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
#[expect(
    clippy::struct_field_names,
    reason = "JSON field is named saga_id, like in events and the database"
)]
pub struct Saga {
    saga_id: i64,
    /// The proximity alert that started the saga.
    alert_id: i64,
    hive_id: String,
    other_hive_id: String,
    /// started, `command_sent`, moving, compensating, completed or failed.
    #[schema(value_type = String)]
    status: SagaStatus,
    /// Hive commanded in the current attempt.
    moving_hive_id: Option<String>,
    /// Reason of the latest change.
    note: String,
    updated_at: DateTime<Utc>,
}

/// Most recent deconfliction sagas, newest first.
#[utoipa::path(
    get,
    path = "/sagas",
    tag = "Sagas",
    responses((status = 200, description = "Latest 50 sagas", body = Vec<Saga>))
)]
async fn list_sagas(State(pool): State<PgPool>) -> Result<Json<Vec<Saga>>, ApiError> {
    let sagas = sqlx::query_as(
        "SELECT saga_id, alert_id, hive_id, other_hive_id, status, moving_hive_id,
                note, updated_at
         FROM sagas ORDER BY saga_id DESC LIMIT 50",
    )
    .fetch_all(&pool)
    .await?;
    Ok(Json(sagas))
}

/// Open monitoring alerts, newest first.
#[utoipa::path(
    get,
    path = "/alerts",
    tag = "Alerts",
    responses((status = 200, description = "Open alerts", body = Vec<Alert>))
)]
async fn list_open_alerts(
    State(pool): State<PgPool>,
) -> Result<Json<Vec<Alert>>, ApiError> {
    let alerts = sqlx::query_as(
        "SELECT alert_id, kind, hive_id, zone_id, other_hive_id, distance_m,
                raised_at, resolved_at
         FROM alerts WHERE resolved_at IS NULL ORDER BY raised_at DESC",
    )
    .fetch_all(&pool)
    .await?;
    Ok(Json(alerts))
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
