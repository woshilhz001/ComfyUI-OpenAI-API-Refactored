// src/handlers/image.rs 完整文件
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response as AxumResponse;
use axum::body::Body;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::time::timeout;
use rand::Rng;
use tracing::{info, warn};                 // 只导入 info 和 warn
use base64::{engine::general_purpose, Engine as _};

use crate::error::{ProxyError, handle_request_error};
use crate::proxy::ProxyState;
use crate::task_manager::TaskState;
use crate::cache::image_cache;
use crate::transport::poll::poll_history_for_images;
use crate::workflows::template::{PreparedWorkflow, InjectRole};
use crate::config::BackendConfig;
use crate::utils::format_file_info;

// … 下方的数据结构、handler、create_image_payload、build_openai_image_response 等保持不变 …

#[derive(Debug, Deserialize)]
pub struct OpenAIImageRequest {
    pub model: String,
    pub prompt: Option<String>,
    pub negative_prompt: Option<String>,
    pub size: Option<String>,
    pub seed: Option<i64>,
    pub n: Option<i64>,
    #[serde(default)]
    pub reference_images: Vec<ReferenceImage>,
    #[serde(default)]
    pub image: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ReferenceImage {
    pub name: Option<String>,
    pub data: String,
}

pub fn sanitize_log_body(body_str: &str) -> String {
    if let Ok(mut json) = serde_json::from_str::<Value>(body_str) {
        if let Some(arr) = json.get_mut("image").and_then(|v| v.as_array_mut()) {
            for item in arr.iter_mut() {
                if item.is_string() {
                    *item = Value::String("[base64 omitted]".into());
                }
            }
        }
        if let Some(arr) = json.get_mut("reference_images").and_then(|v| v.as_array_mut()) {
            for item in arr.iter_mut() {
                if let Some(data) = item.get_mut("data") {
                    if data.is_string() {
                        *data = Value::String("[base64 omitted]".into());
                    }
                }
            }
        }
        serde_json::to_string(&json).unwrap_or_else(|_| body_str.to_string())
    } else {
        if body_str.len() > 500 {
            format!("{}...", &body_str[..500])
        } else {
            body_str.to_string()
        }
    }
}

pub async fn image_generations_handler(
    State(state): State<Arc<ProxyState>>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<AxumResponse, ProxyError> {
    let body_str = String::from_utf8_lossy(&body);
    info!("📥 Image request: {}", sanitize_log_body(&body_str));

    // 拒绝正在关闭的服务器
    if state.graceful_shutdown.is_shutting_down() {
        warn!("Image request rejected because server is shutting down");
        return Err(ProxyError::Internal("Server is shutting down".into()));
    }

    // 幂等键检查
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    if state.enable_response_cache {
        if let (true, Some(ref key)) = (state.enable_idempotency, &idempotency_key) {
            if let Some(cached) = state.response_cache.as_ref().and_then(|c| c.get(key)) {
                info!("相同提交结果，已触发缓存命中，不会执行ai生成，如果需要取消缓存命中，请到配置文件config.yaml修改 enable_response_cache 字段值为 false。");
                return Ok(AxumResponse::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(Body::from(cached))
                    .unwrap());
            }
        }
    }
    
    // 限流
    if let Some(limiter) = &state.rate_limiter {
        if !limiter.try_acquire() {
            warn!("Image request rejected by rate limiter");
            return Err(ProxyError::RateLimited("Too many requests".into()));
        }
    }

    // 选择后端
    let backend_name = params.get("backend").map(|s| s.as_str());
    let backend = state.get_backend(backend_name)?;
    info!("Selected backend '{}' at {}:{}", backend.name, backend.host, backend.port);

    // 解析请求
    let mut request: OpenAIImageRequest = match serde_json::from_str(&body_str) {
        Ok(req) => req,
        Err(e) => {
            warn!("Invalid image request JSON: {}", e);
            return Err(ProxyError::Json(format!("Invalid request: {}", e)));
        }
    };
    for img in &request.image {
        request.reference_images.push(ReferenceImage {
            name: None,
            data: img.clone(),
        });
    }
    
    // 在解析完 request 并合并 image 字段后（大约原代码第 110 行附近），添加以下代码：
    info!("📸 Reference images count: {}", request.reference_images.len());
    for (idx, img) in request.reference_images.iter().enumerate() {
        let preview = if img.data.len() > 30 {
            format!("{}...", &img.data[..30])
        } else {
            img.data.clone()
        };
        info!("  Reference image {}: name={:?}, data={}", idx, img.name, preview);
    }

    let task_id = format!(
        "img-{}-{}",
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis(),
        rand::random::<u16>()
    );
    state.task_manager.insert(task_id.clone(), TaskState::Processing { comfyui_task_id: None }).await;

    // 获取工作流模板
    let template = state.registry.get(&request.model)
        .ok_or_else(|| {
            warn!("Workflow '{}' not found", request.model);
            ProxyError::Json(format!("Workflow '{}' not found", request.model))
        })?;
    let prepared = PreparedWorkflow::from_template(&template);

    // 请求级响应缓存（无参考图时可用）
    let cache_key = if request.reference_images.is_empty() {
        Some(format!(
            "img_{}_{}_{:?}_{:?}",
            request.model,
            request.prompt.as_deref().unwrap_or(""),
            request.size,
            request.seed
        ))
    } else {
        None
    };
    if state.enable_response_cache {
        if let Some(ref key) = cache_key {
            if let Some(cached) = state.response_cache.as_ref().and_then(|c| c.get(key)) {
                info!("相同提交结果，已触发缓存命中，不会执行ai生成，如果需要取消缓存命中，请到配置文件config.yaml修改 enable_response_cache 字段值为 false。");
                return Ok(AxumResponse::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(Body::from(cached))
                    .unwrap());
            }
        }
    }

    // 构建并提交工作流
    let payload = create_image_payload(
        &request,
        &prepared,
        &state.client_id,
        &state.client,
        state.image_width,
        state.image_height,
        backend,
    ).await?;

    let target_url = format!("http://{}:{}/prompt", backend.host, backend.port);
    info!("🚀 Submitting to ComfyUI: {}", target_url);

    let response = state.client
        .post(&target_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| handle_request_error(e, &target_url))?;

    let resp_json: Value = response.json().await?;
    let prompt_id = resp_json["prompt_id"].as_str()
        .ok_or_else(|| ProxyError::Upstream("No prompt_id in response".into()))?
        .to_string();
    info!("任务生成：{}", prompt_id);
    state.task_manager.update(&task_id, TaskState::Processing { comfyui_task_id: Some(prompt_id.clone()) }).await;
    let start_time = Instant::now();

    // 轮询获取图片
    let images = match timeout(
        Duration::from_secs(state.job_timeout_seconds),
        poll_history_for_images(
            &format!("{}:{}", backend.host, backend.port),
            &prompt_id,
            &state.client,
            state.job_timeout_seconds,
        ),
    )
    .await {
        Ok(Ok(images)) => images,
        Ok(Err(e)) => {
            state.task_manager.update(
                &task_id,
                TaskState::Failed {
                    error: e.to_string(),
                    comfyui_task_id: Some(prompt_id.clone()),
                },
            ).await;
            return Err(e);
        }
        Err(_) => {
            let err = ProxyError::Upstream("Job completion timeout".into());
            state.task_manager.update(
                &task_id,
                TaskState::Failed {
                    error: err.to_string(),
                    comfyui_task_id: Some(prompt_id.clone()),
                },
            ).await;
            return Err(err);
        }
    };

    let result_info: Vec<String> = images
        .iter()
        .enumerate()
        .map(|(_idx, (name, bytes))| {
            format_file_info(name, bytes.len())
        })
        .collect();
    let elapsed = start_time.elapsed();
    let elapsed_minutes = elapsed.as_secs() / 60;
    let elapsed_seconds = elapsed.as_secs() % 60;
    info!(
        "任务（{}）完成，返回结果：{}，执行时间：{}m{}s",
        prompt_id,
        result_info.join(", "),
        elapsed_minutes,
        elapsed_seconds
    );

    let response_json = build_openai_image_response(
        images.into_iter().map(|(_, bytes)| bytes).collect(),
    );
    let output_body = serde_json::to_vec(&response_json)?;

    // 更新任务状态
    state.task_manager.update(
        &task_id,
        TaskState::Completed {
            video_url: None,
            b64_json: Some(result_info.join(", ")),
            comfyui_task_id: Some(prompt_id.clone()),
            execution_time: Some(format!("{}m{}s", elapsed_minutes, elapsed_seconds)),
        },
    ).await;

    // 填充缓存
    if let Some(ref key) = cache_key {
        if let Some(cache) = &state.response_cache {
            cache.insert(key.clone(), output_body.clone());
        }
    }
    if let (true, Some(ref key)) = (state.enable_idempotency, &idempotency_key) {
        if let Some(cache) = &state.response_cache {
            cache.insert(key.clone(), output_body.clone());
        }
    }

    Ok(AxumResponse::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(Body::from(output_body))
        .unwrap())
}

async fn create_image_payload(
    request: &OpenAIImageRequest,
    prepared: &PreparedWorkflow,
    client_id: &str,
    http_client: &Client,
    config_width: Option<u32>,
    config_height: Option<u32>,
    backend: &BackendConfig,
) -> Result<Value, ProxyError> {
    let mut workflow = prepared.raw.clone();
    let seed_val = request.seed.unwrap_or_else(|| rand::thread_rng().gen_range(0..i64::MAX));
    let seed_str = seed_val.to_string();

    if let Some(obj) = workflow.as_object_mut() {
        // 注入种子值到 RandomNoise 和 KSampler 节点
        for (_, node) in obj.iter_mut() {
            let class_type = node["class_type"].as_str().unwrap_or("");
            if class_type == "RandomNoise" {
                node["inputs"]["noise_seed"] = json!(seed_str);
            } else if class_type == "KSampler" {
                node["inputs"]["seed"] = json!(seed_str);
            }
        }

        // 提示词
        if let Some(pos_id) = prepared.inject_points.get(&InjectRole::PositivePrompt) {
            if let Some(prompt) = &request.prompt {
                obj[pos_id]["inputs"]["text"] = json!(prompt);
            }
        }
        if let Some(neg_id) = prepared.inject_points.get(&InjectRole::NegativePrompt) {
            if let Some(neg) = &request.negative_prompt {
                obj[neg_id]["inputs"]["text"] = json!(neg);
            }
        }

        // 尺寸计算与注入
        let final_width = config_width.filter(|&w| w > 0)
            .or_else(|| request.size.as_ref()
                .and_then(|s| s.split('x').next())
                .and_then(|v| v.parse().ok()))
            .unwrap_or(0);
        let final_height = config_height.filter(|&h| h > 0)
            .or_else(|| request.size.as_ref()
                .and_then(|s| s.split('x').nth(1))
                .and_then(|v| v.parse().ok()))
            .unwrap_or(0);

        if let Some(width_id) = prepared.inject_points.get(&InjectRole::Width) {
            if final_width > 0 {
                obj[width_id]["inputs"]["value"] = json!(final_width);
            }
        }
        if let Some(height_id) = prepared.inject_points.get(&InjectRole::Height) {
            if final_height > 0 {
                obj[height_id]["inputs"]["value"] = json!(final_height);
            }
        }

        // 批量大小
        if let Some(n) = request.n {
            for (_, node) in obj.iter_mut() {
                let ct = node["class_type"].as_str().unwrap_or("");
                if ct == "EmptyLatentImage" || ct == "EmptySD3LatentImage" || ct == "EmptyFlux2LatentImage" {
                    node["inputs"]["batch_size"] = json!(n.to_string());
                }
            }
        }

        // 参考图注入
        let ref_count = request.reference_images.len();
        info!("🖼️ Preparing to inject {} reference images into {} LoadImage nodes", ref_count, prepared.load_image_nodes.len());
        if !prepared.load_image_nodes.is_empty() {
            if ref_count == 0 {
                let placeholder = image_cache::get_placeholder_filename(http_client, backend).await?;
                info!("No reference images, using placeholder: {}", placeholder);
                for nid in &prepared.load_image_nodes {
                    obj[nid]["inputs"]["image"] = json!(placeholder);
                    info!("  Node {} -> placeholder", nid);
                }
            } else {
                for (i, nid) in prepared.load_image_nodes.iter().enumerate() {
                    let idx = if i < ref_count { i } else { ref_count - 1 };
                    let filename = image_cache::cache_image(http_client, backend, &request.reference_images[idx].data).await?;
                    obj[nid]["inputs"]["image"] = json!(filename);
                    info!("  Node {} -> reference image index {} (filename: {})", nid, idx, filename);
                }
            }
        }
    }

    Ok(json!({
        "prompt": workflow,
        "client_id": client_id
    }))
}

fn build_openai_image_response(images: Vec<Vec<u8>>) -> Value {
    let created = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let data: Vec<Value> = images
        .into_iter()
        .map(|bytes| json!({ "b64_json": general_purpose::STANDARD.encode(&bytes) }))
        .collect();
    json!({ "created": created, "data": data })
}