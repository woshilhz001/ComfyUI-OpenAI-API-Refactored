mod config;
mod proxy;
mod error;
mod seed_tracker;
mod graceful;
mod tracing_setup;
mod workflows;
mod handlers;
mod backend;
mod transport;
mod middleware;
mod cache;
mod task_manager;

use axum::{
    Router,
    routing::{get, post, delete},
    middleware as axum_mw,
    extract::DefaultBodyLimit,
    body::Body,
};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use std::sync::Arc;
use std::time::Duration;
use tower_http::{cors::CorsLayer, limit::RequestBodyLimitLayer};

use config::Config;
use proxy::ProxyState;
use workflows::registry::WorkflowRegistry;
use cache::image_cache;
use cache::response_cache::ResponseCache;
use backend::pool::BackendPool;
use middleware::rate_limiter::RateLimiter;
use seed_tracker::SeedTracker;
use graceful::GracefulShutdown;
use task_manager::TaskManager;

use handlers::{metrics, tasks, image, video, models, health, backends};
use crate::handlers::metrics::init_metrics;

async fn fallback_handler(req: axum::http::Request<axum::body::Body>) -> impl axum::response::IntoResponse {
    let method = req.method();
    let uri = req.uri();
    tracing::warn!("⚠️ 404 Not Found: {} {}", method, uri);
    (axum::http::StatusCode::NOT_FOUND, "Route not found")
}

#[tokio::main]
async fn main() {
    tracing_setup::init_tracing().expect("Tracing init failed");
    let config = Config::load().expect("Failed to load config");
    image_cache::init_input_dir(config.comfyui_backend.input_dir.clone());
    image_cache::start_cache_cleaner();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(config.routing.timeout_seconds))
        .connect_timeout(Duration::from_secs(5))
        .build().unwrap();

    let mut registry = WorkflowRegistry::new();
    registry.load_from_folder(&config.comfyui_backend.workflows_folder).expect("Failed to load workflows");
    let registry = Arc::new(registry);

    let backend_pool = Arc::new(BackendPool::new(
        config.comfyui_backends.clone(),
        config.routing.lb_strategy.clone(),
        config.routing.health_check_interval_secs,
        config.routing.health_check_fail_threshold,
    ));

    let rate_limiter = config.routing.rate_limit.map(|r| Arc::new(RateLimiter::new(r.max_tokens, r.refill_rate)));
    let response_cache = config.routing.response_cache.map(|c| Arc::new(ResponseCache::new(c.max_entries, c.ttl_secs)));
    let seed_tracker = Arc::new(SeedTracker::new());
    let graceful_shutdown = Arc::new(GracefulShutdown::new(100, config.routing.graceful_shutdown_timeout_secs));
    let task_manager = Arc::new(TaskManager::new_with_persist("./tasks.json"));
    task_manager.load_persisted_async().await;

    init_metrics();

    let proxy_state = Arc::new(ProxyState {
        client,
        backends: backend_pool,
        client_id: config.comfyui_backend.client_id,
        registry,
        job_timeout_seconds: config.routing.timeout_seconds,
        task_manager: task_manager.clone(),
        image_width: config.routing.image_width,
        image_height: config.routing.image_height,
        video_width: config.routing.video_width,
        video_height: config.routing.video_height,
        default_fps: config.routing.fps,
        free_model_before_video: config.routing.free_model_before_video,
        response_cache,
        enable_response_cache: config.routing.enable_response_cache,
        seed_tracker,
        rate_limiter,
        graceful_shutdown: graceful_shutdown.clone(),
        enable_idempotency: config.routing.enable_idempotency,
    });

    let app = Router::new()
        .route("/v1/help", get(help_handler))
        .route("/v1/models", get(models::models_handler))
        .route("/v1/health", get(health::health_handler))
        .route("/v1/backends", get(backends::backends_handler))
        .route("/v1/videos/health", get(video::video_health_handler))
        .route("/v1/videos/generations", post(video::video_generations_handler))
        .route("/v1/images/generations", post(image::image_generations_handler))
        .route("/v1/metrics", get(metrics::metrics_handler))
        .route("/v1/tasks/:task_id", get(tasks::task_query))
        .route("/v1/tasks", get(tasks::task_list))
        .layer(axum_mw::from_fn(|req, next: axum::middleware::Next| async move {
            metrics::TOTAL_REQUESTS.inc();
            next.run(req).await
        }))
        .layer(CorsLayer::permissive())
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new((config.routing.max_payload_size_mb as usize) * 1024 * 1024))
        .fallback(fallback_handler)
        .with_state(proxy_state);

    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("Server listening on {}", addr);

    let graceful_shutdown_clone = graceful_shutdown.clone();
    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            graceful_shutdown_clone.shutdown().await;
        })
        .await
        .unwrap();
}

async fn help_handler() -> impl IntoResponse {
    let doc = serde_json::json!({
        "service": "comfyui-openai-api",
        "version": "0.3.0",
        "description": "OpenAI-compatible proxy for ComfyUI with multi-backend support",
        "endpoints": {
            "/v1/models": {
                "method": "GET",
                "description": "List available models (workflows)"
            },
            "/v1/health": {
                "method": "GET",
                "description": "Health check"
            },
            "/v1/backends": {
                "method": "GET",
                "description": "Backend health status list"
            },
            "/v1/images/generations": {
                "method": "POST",
                "query_parameter": {
                    "name": "backend",
                    "required": false,
                    "description": "Name of the ComfyUI backend (uses load balancer if omitted)"
                },
                "request_body": {
                    "model": "string (workflow filename without .json)",
                    "prompt": "string (optional)",
                    "negative_prompt": "string (optional)",
                    "size": "string e.g. '1024x1024' (optional)",
                    "seed": "integer (optional)",
                    "n": "integer (optional, number of images to generate)",
                    "reference_images": "array of {name, data} (optional)",
                    "image": "array of base64 strings (optional, alternative to reference_images)"
                },
                "response": {
                    "created": "timestamp",
                    "data": "[ { \"b64_json\": \"base64...\" } ]"
                }
            },
            "/v1/videos/generations": {
                "method": "POST",
                "query_parameter": "backend (optional)",
                "request_body": {
                    "model": "string (video workflow filename)",
                    "content": "[ { \"type\":\"text\", \"text\":\"...\" }, { \"type\":\"image_url\", ... } ]",
                    "duration": "integer (seconds, optional)",
                    "resolution": "\"720p\" or \"1080p\" (optional)"
                },
                "response": {
                    "task_id": "string"
                }
            },
            "/v1/tasks": {
                "method": "GET",
                "description": "List all tasks"
            },
            "/v1/tasks/{task_id}": {
                "methods": ["GET", "DELETE"],
                "description": "Get or delete a specific task"
            },
            "/v1/metrics": {
                "method": "GET",
                "description": "Prometheus metrics"
            },
            "/v1/videos/health": {
                "method": "GET",
                "description": "Video generation subsystem health"
            },
            "/v1/help": {
                "method": "GET",
                "description": "This documentation"
            }
        }
    });
    (StatusCode::OK, Json(doc))
}