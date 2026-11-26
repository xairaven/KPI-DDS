//! API authorization with bearer tokens (requirement BMS-7).
//!
//! Every request to `/api/v1/*` must carry `Authorization: Bearer <token>` with one
//! of the configured tokens; otherwise the answer is `401 Unauthorized`. Swagger UI
//! and the OpenAPI document stay public: they describe the API, they give no data.
//!
//! Several tokens may be valid at once (one per client), so a token can be
//! replaced without downtime: add the new one, move the clients, remove the old one.

use axum::Json;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use tracing::debug;

/// The accepted tokens. Cheap to clone (shared list).
#[derive(Clone)]
pub struct ApiTokens(Arc<[Box<str>]>);

/// Tokens are secrets: never print them, not even in debug output.
impl fmt::Debug for ApiTokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ApiTokens({} configured)", self.0.len())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("expected one or more comma-separated, non-empty tokens")]
pub struct NoTokens;

/// Parses `BMS_API_TOKENS`, e.g. `token-for-ui,token-for-scripts`.
impl FromStr for ApiTokens {
    type Err = NoTokens;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let tokens: Arc<[Box<str>]> = value
            .split(',')
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(Box::from)
            .collect();
        if tokens.is_empty() {
            return Err(NoTokens);
        }
        Ok(Self(tokens))
    }
}

impl ApiTokens {
    fn accepts(&self, presented: &str) -> bool {
        // Every token is compared, without stopping at the first match, and each
        // comparison takes the same time however many characters match: the
        // response time tells an attacker nothing about how close a guess was.
        self.0.iter().fold(false, |found, token| {
            found | constant_time_eq(token.as_bytes(), presented.as_bytes())
        })
    }
}

/// Compares two byte strings in time that depends only on their lengths.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0_u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

/// Middleware: lets the request through only with a valid bearer token.
pub async fn require_token(
    State(tokens): State<ApiTokens>, request: Request, next: Next,
) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));

    match presented {
        Some(token) if tokens.accepts(token) => next.run(request).await,
        Some(_) => unauthorized("Invalid token"),
        None => unauthorized("Missing bearer token"),
    }
}

fn unauthorized(reason: &'static str) -> Response {
    debug!(reason, "Request rejected");
    let mut response =
        (StatusCode::UNAUTHORIZED, Json(json!({ "error": reason }))).into_response();
    // Tells the client which authentication scheme the API expects (RFC 6750).
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::routing::get;
    use tower::ServiceExt;

    fn tokens(value: &str) -> ApiTokens {
        value.parse().unwrap()
    }

    #[test]
    fn parses_comma_separated_tokens() {
        let tokens = tokens(" first , second,,");
        assert!(tokens.accepts("first"));
        assert!(tokens.accepts("second"));
        assert!(!tokens.accepts(""));
        assert!(!tokens.accepts("third"));
    }

    #[test]
    fn rejects_empty_configuration() {
        assert!("".parse::<ApiTokens>().is_err());
        assert!(" , ".parse::<ApiTokens>().is_err());
    }

    #[test]
    fn prefix_of_a_token_is_not_accepted() {
        let tokens = tokens("secret-token");
        assert!(!tokens.accepts("secret"));
        assert!(!tokens.accepts("secret-token-and-more"));
    }

    #[test]
    fn debug_output_hides_tokens() {
        assert_eq!(format!("{:?}", tokens("a,b")), "ApiTokens(2 configured)");
    }

    async fn status(authorization: Option<&str>) -> StatusCode {
        let app = Router::new().route("/", get(|| async { "data" })).layer(
            axum::middleware::from_fn_with_state(tokens("good"), require_token),
        );
        let mut request = Request::builder().uri("/");
        if let Some(value) = authorization {
            request = request.header(header::AUTHORIZATION, value);
        }
        let response = app
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        response.status()
    }

    #[tokio::test]
    async fn middleware_checks_the_header() {
        assert_eq!(status(Some("Bearer good")).await, StatusCode::OK);
        assert_eq!(status(Some("Bearer bad")).await, StatusCode::UNAUTHORIZED);
        assert_eq!(status(Some("Basic good")).await, StatusCode::UNAUTHORIZED);
        assert_eq!(status(None).await, StatusCode::UNAUTHORIZED);
    }
}
