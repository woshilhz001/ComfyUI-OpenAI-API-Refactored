// middleware/request_id.rs
use axum::{
    http::{HeaderName, HeaderValue},
    middleware::Next,
    response::Response,
};
use std::time::{SystemTime, UNIX_EPOCH};

pub async fn request_id_middleware(
    mut req: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    let id = format!("{:x}", SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos());
    req.headers_mut().insert(
        HeaderName::from_static("x-request-id"),
        HeaderValue::from_str(&id).unwrap(),
    );
    let mut response = next.run(req).await;
    response.headers_mut().insert("x-request-id", HeaderValue::from_str(&id).unwrap());
    response
}