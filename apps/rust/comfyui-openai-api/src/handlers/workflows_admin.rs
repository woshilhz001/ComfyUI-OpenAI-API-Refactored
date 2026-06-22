use axum::{extract::State, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::proxy::ProxyState;

/// POST /v1/workflows/reload
/// 重新扫描工作流文件夹，热重载所有工作流模板（无需重启服务器）
pub async fn reload_workflows_handler(
    State(state): State<Arc<ProxyState>>,
) -> Json<Value> {
    match state.registry.write() {
        Ok(mut registry) => {
            match registry.reload_from_folder(&state.workflows_folder) {
                Ok(()) => {
                    let models: Vec<String> = registry.list_models();
                    tracing::info!("Workflows reloaded: {} models", models.len());
                    Json(json!({
                        "status": "ok",
                        "models": models
                    }))
                }
                Err(e) => {
                    tracing::error!("Failed to reload workflows: {}", e);
                    Json(json!({
                        "status": "error",
                        "message": format!("Failed to reload workflows: {}", e)
                    }))
                }
            }
        }
        Err(e) => {
            Json(json!({
                "status": "error",
                "message": format!("RwLock poisoned: {}", e)
            }))
        }
    }
}
