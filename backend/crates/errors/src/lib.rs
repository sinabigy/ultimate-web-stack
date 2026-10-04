//! One error type for every HTTP-facing failure, rendered as RFC 9457 `application/problem+json`.
//!
//! Rules:
//! - Clients get a stable machine-readable `code` and a safe `detail`. Internal causes
//!   (SQL, upstream bodies, stack traces) are logged server-side, never returned.
//! - 5xx responses are logged at ERROR with the internal cause; 4xx at DEBUG.
//! - The request id is returned in the `x-request-id` header, so a user report can be
//!   joined to server logs without leaking anything in the body.

use std::fmt;

use axum::{
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;

/// Field-level validation problem.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("validation failed")]
    Validation(Vec<FieldError>),
    #[error("authentication required")]
    Unauthenticated,
    #[error("forbidden")]
    Forbidden,
    /// 403 with a stable machine-readable reason (`mfa_required`, `reauth_required`,
    /// `csrf_failed`, `escalation`, `last_owner`, ...) so clients can react precisely.
    #[error("forbidden: {0}")]
    ForbiddenReason(&'static str),
    #[error("not found")]
    NotFound,
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("payload too large")]
    PayloadTooLarge,
    #[error("rate limited")]
    RateLimited { retry_after_secs: u64 },
    /// A dependency (database, cache, provider) is unavailable; the request may succeed later.
    #[error("service unavailable: {0}")]
    Unavailable(&'static str),
    #[error("upstream failure: {0}")]
    Upstream(String),
    #[error("timeout")]
    Timeout,
    /// Anything unexpected. The cause is logged, never sent to the client.
    #[error("internal error")]
    Internal(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl ApiError {
    pub fn internal(e: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self::Internal(e.into())
    }

    pub fn validation(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Validation(vec![FieldError { field: field.into(), message: message.into() }])
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Unauthenticated => StatusCode::UNAUTHORIZED,
            Self::Forbidden | Self::ForbiddenReason(_) => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Upstream(_) => StatusCode::BAD_GATEWAY,
            Self::Timeout => StatusCode::GATEWAY_TIMEOUT,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Stable, documented machine-readable code. Clients branch on this, never on `detail`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "bad_request",
            Self::Validation(_) => "validation_failed",
            Self::Unauthenticated => "unauthenticated",
            Self::Forbidden => "forbidden",
            Self::ForbiddenReason(code) => code,
            Self::NotFound => "not_found",
            Self::Conflict(_) => "conflict",
            Self::PayloadTooLarge => "payload_too_large",
            Self::RateLimited { .. } => "rate_limited",
            Self::Unavailable(_) => "unavailable",
            Self::Upstream(_) => "upstream_failure",
            Self::Timeout => "timeout",
            Self::Internal(_) => "internal",
        }
    }

    fn public_detail(&self) -> Option<String> {
        match self {
            Self::BadRequest(m) | Self::Conflict(m) => Some(m.clone()),
            Self::Unavailable(dep) => Some(format!("{dep} is temporarily unavailable")),
            // Upstream messages may contain provider internals; keep them in logs only.
            _ => None,
        }
    }
}

/// RFC 9457 problem document.
#[derive(Debug, Serialize)]
pub struct Problem {
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub status: u16,
    pub code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<FieldError>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        if status.is_server_error() {
            match &self {
                Self::Internal(cause) => tracing::error!(error = %cause, code = self.code(), "request failed"),
                other => tracing::warn!(error = %other, code = self.code(), "request failed"),
            }
        } else {
            tracing::debug!(error = %self, code = self.code(), "request rejected");
        }
        let problem = Problem {
            kind: format!("about:blank#{}", self.code()),
            title: status.canonical_reason().unwrap_or("Error").to_string(),
            status: status.as_u16(),
            code: self.code(),
            detail: self.public_detail(),
            errors: match &self {
                Self::Validation(v) => v.clone(),
                _ => Vec::new(),
            },
        };
        let body = serde_json::to_vec(&problem).unwrap_or_else(|_| b"{}".to_vec());
        let mut res = (status, body).into_response();
        res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/problem+json"));
        if let Self::RateLimited { retry_after_secs } = self
            && let Ok(v) = HeaderValue::from_str(&retry_after_secs.max(1).to_string())
        {
            res.headers_mut().insert(header::RETRY_AFTER, v);
        }
        res
    }
}

/// Convenience for handlers: `Result<Json<T>, ApiError>`.
pub type ApiResult<T> = Result<T, ApiError>;

/// Display wrapper that never prints secrets embedded in URLs (userinfo).
pub struct RedactedUrl<'a>(pub &'a str);

impl fmt::Display for RedactedUrl<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.0.find("://"), self.0.rfind('@')) {
            (Some(s), Some(at)) if at > s => write!(f, "{}://<redacted>@{}", &self.0[..s], &self.0[at + 1..]),
            _ => f.write_str(self.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    async fn render(e: ApiError) -> (StatusCode, axum::http::HeaderMap, serde_json::Value) {
        let res = e.into_response();
        let (parts, body) = res.into_parts();
        let bytes = body.collect().await.expect("body").to_bytes();
        (parts.status, parts.headers, serde_json::from_slice(&bytes).expect("json"))
    }

    #[tokio::test]
    async fn internal_errors_never_leak_their_cause() {
        let (status, headers, body) = render(ApiError::internal("password=hunter2 in SQL")).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(headers[header::CONTENT_TYPE], "application/problem+json");
        assert_eq!(body["code"], "internal");
        assert!(!body.to_string().contains("hunter2"));
    }

    #[tokio::test]
    async fn validation_errors_list_fields() {
        let (status, _, body) = render(ApiError::validation("email", "must contain @")).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["errors"][0]["field"], "email");
    }

    #[tokio::test]
    async fn rate_limited_sets_retry_after() {
        let (status, headers, _) = render(ApiError::RateLimited { retry_after_secs: 0 }).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(headers[header::RETRY_AFTER], "1");
    }

    #[test]
    fn redacts_url_credentials() {
        assert_eq!(RedactedUrl("postgres://u:pw@db:5432/app").to_string(), "postgres://<redacted>@db:5432/app");
        assert_eq!(RedactedUrl("redis://cache:6379").to_string(), "redis://cache:6379");
    }
}
