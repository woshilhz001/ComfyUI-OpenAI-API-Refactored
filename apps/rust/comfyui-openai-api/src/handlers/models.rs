use axum::{extract::State, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::proxy::ProxyState;

pub async fn models_handler(State(state): State<Arc<ProxyState>>) -> Json<Value> {
    let models: Vec<Value> = state.registry.list_models()
        .into_iter()
        .map(|name| json!({
            "id": name,
            "object": "model",
            "owned_by": "comfyui-openai-api"
        }))
        .collect();
    Json(json!({
        "object": "list",
        "data": models
    }))
}