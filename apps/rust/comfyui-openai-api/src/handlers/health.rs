use axum::http::StatusCode;
use axum::response::IntoResponse;

pub async fn health_handler() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}