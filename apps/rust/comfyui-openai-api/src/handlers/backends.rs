use axum::{extract::State, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::proxy::ProxyState;

pub async fn backends_handler(State(state): State<Arc<ProxyState>>) -> Json<Value> {
    let backends: Vec<Value> = state.backends.list_backends()
        .into_iter()
        .map(|(name, healthy)| json!({
            "name": name,
            "healthy": healthy
        }))
        .collect();
    Json(json!({ "backends": backends }))
}