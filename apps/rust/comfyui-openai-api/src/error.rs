use axum::{
    http::StatusCode,
    response::{IntoResponse, Response as AxumResponse},
    Json,
};
use serde::Serialize;
use std::fmt;
use tracing::warn;

#[derive(Debug)]
pub enum ProxyError {
    Internal(String),
    Upstream(String),
    Json(String),
    RateLimited(String),
    IdempotencyConflict,
}

impl fmt::Display for ProxyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProxyError::Internal(msg) => write!(f, "Internal error: {msg}"),
            ProxyError::Upstream(msg) => write!(f, "Upstream error: {msg}"),
            ProxyError::Json(msg) => write!(f, "JSON error: {msg}"),
            ProxyError::RateLimited(msg) => write!(f, "Rate limited: {msg}"),
            ProxyError::IdempotencyConflict => write!(f, "Idempotency conflict"),
        }
    }
}

#[derive(Serialize)]
struct OpenAIError {
    message: String,
    #[serde(rename = "type")]
    error_type: String,
    code: String,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: OpenAIError,
}

impl IntoResponse for ProxyError {
    fn into_response(self) -> AxumResponse {
        let (status, message) = match &self {
            ProxyError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            ProxyError::Upstream(msg) => (StatusCode::BAD_GATEWAY, msg.clone()),
            ProxyError::Json(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            ProxyError::RateLimited(msg) => (StatusCode::TOO_MANY_REQUESTS, msg.clone()),
            ProxyError::IdempotencyConflict => (StatusCode::CONFLICT, "Idempotency conflict".to_string()),
        };
        let error_type = match &self {
            ProxyError::Internal(_) => "internal_error",
            ProxyError::Upstream(_) => "upstream_error",
            ProxyError::Json(_) => "invalid_request_error",
            ProxyError::RateLimited(_) => "rate_limit_exceeded",
            ProxyError::IdempotencyConflict => "idempotency_conflict",
        };
        let error_response = ErrorResponse {
            error: OpenAIError {
                message,
                error_type: error_type.to_string(),
                code: status.as_u16().to_string(),
            },
        };
        (status, Json(error_response)).into_response()
    }
}

impl From<reqwest::Error> for ProxyError {
    fn from(err: reqwest::Error) -> Self {
        ProxyError::Upstream(format!("Request error: {}", err))
    }
}

impl From<serde_json::Error> for ProxyError {
    fn from(err: serde_json::Error) -> Self {
        ProxyError::Json(format!("JSON error: {}", err))
    }
}

impl From<anyhow::Error> for ProxyError {
    fn from(err: anyhow::Error) -> Self {
        ProxyError::Internal(err.to_string())
    }
}

pub fn handle_request_error(e: reqwest::Error, full_url: &str) -> ProxyError {
    warn!("HTTP request to {} failed: {}", full_url, e);
    if e.is_timeout() {
        ProxyError::Upstream(format!("Timeout to {}: {}", full_url, e))
    } else if e.is_connect() {
        ProxyError::Upstream(format!("Connection failed to {}: {}", full_url, e))
    } else {
        ProxyError::Upstream(format!("Network error to {}: {}", full_url, e))
    }
}