use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use std::sync::Arc;
use crate::proxy::ProxyState;
use crate::task_manager::TaskState;

pub async fn task_query(
    State(state): State<Arc<ProxyState>>,
    Path(task_id): Path<String>,
) -> Json<TaskState> {
    let result = state.task_manager.get(&task_id).await;
    Json(result.unwrap_or(TaskState::Failed {
        error: format!("Task '{}' not found", task_id),
    }))
}

pub async fn task_list(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let tasks = state.task_manager.get_all().await;
    let data: Vec<serde_json::Value> = tasks.into_iter().map(|(id, state)| {
        serde_json::json!({
            "task_id": id,
            "status": state
        })
    }).collect();
    Json(serde_json::json!({ "tasks": data }))
}

pub async fn task_delete(
    State(state): State<Arc<ProxyState>>,
    Path(task_id): Path<String>,
) -> impl IntoResponse {
    state.task_manager.remove(&task_id).await;
    StatusCode::NO_CONTENT
}