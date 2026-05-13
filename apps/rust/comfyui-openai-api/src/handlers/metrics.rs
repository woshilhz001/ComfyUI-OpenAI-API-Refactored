use axum::response::IntoResponse;
use axum::http::StatusCode;
use lazy_static::lazy_static;
use prometheus::{IntCounter, IntGauge, HistogramOpts, HistogramVec, Registry, Encoder};

lazy_static! {
    pub static ref TOTAL_REQUESTS: IntCounter = IntCounter::new("total_requests", "Total requests").unwrap();
    pub static ref ACTIVE_TASKS: IntGauge = IntGauge::new("active_tasks", "Active tasks").unwrap();
    pub static ref REQUEST_DURATION: HistogramVec = HistogramVec::new(
        HistogramOpts::new("request_duration_seconds", "Request duration in seconds"),
        &["endpoint"]
    ).unwrap();
    pub static ref CACHE_HIT_TOTAL: IntCounter = IntCounter::new("cache_hit_total", "Cache hits").unwrap();
    pub static ref REGISTRY: Registry = Registry::new();
}

pub fn init_metrics() {
    REGISTRY.register(Box::new(TOTAL_REQUESTS.clone())).unwrap();
    REGISTRY.register(Box::new(ACTIVE_TASKS.clone())).unwrap();
    REGISTRY.register(Box::new(REQUEST_DURATION.clone())).unwrap();
    REGISTRY.register(Box::new(CACHE_HIT_TOTAL.clone())).unwrap();
}

pub async fn metrics_handler() -> impl IntoResponse {
    let mut buffer = vec![];
    let encoder = prometheus::TextEncoder::new();
    encoder.encode(&REGISTRY.gather(), &mut buffer).unwrap();
    (StatusCode::OK, buffer)
}