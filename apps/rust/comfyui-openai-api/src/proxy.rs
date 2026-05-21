use axum::http::StatusCode;
use axum::response::IntoResponse;
use reqwest::Client;
use serde::Serialize;
use std::{collections::HashMap, sync::Arc};
use crate::config::BackendConfig;
use crate::workflows::registry::WorkflowRegistry;
use crate::workflows::template::PreparedWorkflow;
use crate::backend::pool::BackendPool;
use crate::cache::response_cache::ResponseCache;
use crate::seed_tracker::SeedTracker;
use crate::middleware::rate_limiter::RateLimiter;
use crate::graceful::GracefulShutdown;
use crate::task_manager::TaskManager;

#[derive(Clone)]
pub struct ProxyState {
    pub client: Client,
    pub backends: Arc<BackendPool>,
    pub client_id: String,
    pub registry: Arc<WorkflowRegistry>,
    pub job_timeout_seconds: u64,
    pub task_manager: Arc<TaskManager>,
    pub image_width: Option<u32>,
    pub image_height: Option<u32>,
    pub video_width: Option<u32>,
    pub video_height: Option<u32>,
    pub default_fps: Option<f64>,
    pub free_model_before_video: bool,
    pub free_model_before_image: bool,
    pub response_cache: Option<Arc<ResponseCache>>,
    pub enable_response_cache: bool,
    pub seed_tracker: Arc<SeedTracker>,
    pub rate_limiter: Option<Arc<RateLimiter>>,
    pub graceful_shutdown: Arc<GracefulShutdown>,
    pub enable_idempotency: bool,
}

impl ProxyState {
    pub fn get_backend(&self, name: Option<&str>) -> Result<&BackendConfig, crate::error::ProxyError> {
        if let Some(name) = name {
            self.backends.get_by_name(name)
                .ok_or_else(|| crate::error::ProxyError::Json(format!("Backend '{}' not found", name)))
                .map(|b| &b.config)
        } else {
            // 使用负载均衡
            self.backends.select_backend()
                .map(|b| &b.config)
                .ok_or_else(|| crate::error::ProxyError::Upstream("No healthy backend available".into()))
        }
    }
}