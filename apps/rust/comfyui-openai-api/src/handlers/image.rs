// src/handlers/image.rs 完整文件
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response as AxumResponse;
use axum::body::Body;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
//use std::collections::HashMap;
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

use std::collections::{HashMap, HashSet, VecDeque};

use multer::Multipart;

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

/// 解析 multipart/form-data 请求体为 OpenAIImageRequest
/// 用于处理 OpenAI SDK images.edit() 发送的 multipart 请求
async fn parse_multipart_request(
    body: &[u8],
    content_type: &str,
) -> Result<OpenAIImageRequest, ProxyError> {
    // 从 Content-Type 中提取 boundary
    let boundary = content_type
        .split(';')
        .find_map(|part| {
            let trimmed = part.trim();
            if trimmed.starts_with("boundary=") {
                Some(trimmed[9..].trim_matches('"').to_string())
            } else {
                None
            }
        })
        .ok_or_else(|| ProxyError::Json("Missing boundary in multipart content-type".into()))?;

    info!("📥 Multipart image request (boundary={})", boundary);

    let mut model = String::new();
    let mut prompt: Option<String> = None;
    let mut negative_prompt: Option<String> = None;
    let mut size: Option<String> = None;
    let mut n: Option<i64> = None;
    let mut seed: Option<i64> = None;
    let mut reference_images: Vec<ReferenceImage> = Vec::new();

    // 将字节转换为 stream 供 multer 使用
    let body_bytes = bytes::Bytes::copy_from_slice(body);
    let stream = futures::stream::once(async move {
        Ok::<bytes::Bytes, multer::Error>(body_bytes)
    });
    let mut multipart = Multipart::new(stream, boundary);

    while let Some(mut field) = multipart.next_field().await.map_err(|e| {
        ProxyError::Json(format!("Multipart parse error: {}", e))
    })? {
        let field_name = field.name().map(|n| n.to_string()).unwrap_or_default();
        let content_type = field.content_type().map(|ct| ct.to_string());

        // 判断是文件字段还是文本字段
        let is_file = field_name == "image" || field_name == "mask" || content_type.is_some();

        if is_file {
            // 文件字段：读取字节并 base64 编码
            let mut data = Vec::new();
            while let Some(chunk) = field.chunk().await.map_err(|e| {
                ProxyError::Json(format!("Multipart read error: {}", e))
            })? {
                data.extend_from_slice(&chunk);
            }
            let b64 = general_purpose::STANDARD.encode(&data);
            let img_name = if field_name == "mask" {
                Some("mask".to_string())
            } else {
                field.file_name().map(|f| f.to_string())
            };
            reference_images.push(ReferenceImage {
                name: img_name,
                data: b64,
            });
            info!(
                "  Multipart file field '{}': {} bytes (name={:?})",
                field_name,
                data.len(),
                reference_images.last().unwrap().name
            );
        } else {
            // 文本字段
            let text = field.text().await.map_err(|e| {
                ProxyError::Json(format!("Multipart text read error: {}", e))
            })?;
            let text_trimmed = text.trim().to_string();
            if text_trimmed.is_empty() {
                continue;
            }
            info!("  Multipart field '{}': {}", field_name, text_trimmed);
            match field_name.as_str() {
                "model" => model = text_trimmed,
                "prompt" => prompt = Some(text_trimmed),
                "negative_prompt" => negative_prompt = Some(text_trimmed),
                "size" => size = Some(text_trimmed),
                "n" => {
                    n = text_trimmed.parse().ok();
                    if n.is_none() {
                        warn!("Invalid multipart 'n' value: {}", text_trimmed);
                    }
                }
                "seed" => {
                    seed = text_trimmed.parse().ok();
                    if seed.is_none() {
                        warn!("Invalid multipart 'seed' value: {}", text_trimmed);
                    }
                }
                "response_format" => {
                    // OpenAI 的 response_format，忽略即可
                    info!("  Multipart response_format: {}", text_trimmed);
                }
                _ => {
                    info!("  Multipart unknown field '{}': {}", field_name, text_trimmed);
                }
            }
        }
    }

    if model.is_empty() {
        return Err(ProxyError::Json("Missing 'model' field in multipart request".into()));
    }

    info!("📸 Multipart: model={}, prompt={:?}, reference_images={}",
        model, prompt.as_deref().unwrap_or("(none)"), reference_images.len());

    Ok(OpenAIImageRequest {
        model,
        prompt,
        negative_prompt,
        size,
        seed,
        n,
        reference_images,
        image: Vec::new(), // 已合并到 reference_images
    })
}

pub async fn image_generations_handler(
    State(state): State<Arc<ProxyState>>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<AxumResponse, ProxyError> {
    let body_str = String::from_utf8_lossy(&body);

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

    // ---------- 新增：图片生成前的模型清理 ----------
    if state.free_model_before_image {
        let free_url = format!("http://{}:{}/free", backend.host, backend.port);
        info!("🧹 Freeing ComfyUI memory before image task...");
        let resp = state.client
            .post(&free_url)
            .json(&serde_json::json!({"unload_models": true, "free_memory": true}))
            .send()
            .await
            .map_err(|e| handle_request_error(e, &free_url))?;
        if !resp.status().is_success() {
            return Err(ProxyError::Upstream(format!("ComfyUI /free failed: {}", resp.status())));
        }
    }

    // 解析请求：支持 multipart/form-data 和 application/json
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let mut request: OpenAIImageRequest = if content_type.starts_with("multipart/form-data") {
        parse_multipart_request(&body, content_type).await?
    } else {
        info!("📥 Image request: {}", sanitize_log_body(&body_str));
        match serde_json::from_str(&body_str) {
            Ok(req) => req,
            Err(e) => {
                warn!("Invalid image request JSON: {}", e);
                return Err(ProxyError::Json(format!("Invalid request: {}", e)));
            }
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
    state.task_manager.insert(task_id.clone(), TaskState::Processing { comfyui_task_id: None, backend_name: Some(backend.name.clone())  }).await;

    // 获取工作流模板
    let template = state.registry.read().unwrap().get(&request.model)
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

    // 打印 payload 以便调试
    //info!("📤 Full payload (pretty): {}", serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "Invalid JSON".to_string()));
    // 也可以只打印前 2000 字符
    //info!("📤 Payload (first 2000 chars): {}", serde_json::to_string(&payload).unwrap_or_default());

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
    state.task_manager.update(&task_id, TaskState::Processing { comfyui_task_id: Some(prompt_id.clone()),backend_name: Some(backend.name.clone())  }).await;
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
    let mut reference_count = request.reference_images.len();
    let mut reference_images = request.reference_images.clone();

    // 如果没有参考图且工作流有 LoadImage 节点，则使用占位符保留一个节点
    if reference_count == 0 && !prepared.load_image_nodes.is_empty() {
        reference_count = 1;
        let placeholder_base64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVQI12NgYAAAAAMAASDVlMcAAAAASUVORK5CYII=";
        reference_images.push(ReferenceImage {
            name: Some("placeholder".to_string()),
            data: placeholder_base64.to_string(),
        });
        info!("没有传入参考图, 使用占位符作为参考图，确保工作流中的 LoadImage 节点能够正常工作");
    }

    // 一次可变借用完成所有修改
    if let Some(obj) = workflow.as_object_mut() {
        // 删除多余的参考图分支
        prune_redundant_reference_branches(obj, &prepared.load_image_nodes, reference_count)?;

        // 注入种子
        for (_, node) in obj.iter_mut() {
            let ct = node["class_type"].as_str().unwrap_or("");
            if ct == "RandomNoise" {
                node["inputs"]["noise_seed"] = json!(seed_val.to_string());
            } else if ct == "KSampler" {
                node["inputs"]["seed"] = json!(seed_val);
            }
        }

        // 提示词 — 优先注入 PrimitiveStringMultiline（inputs.value），否则注入 CLIPTextEncode（inputs.text）
        if let Some(str_id) = prepared.inject_points.get(&InjectRole::PositivePromptString) {
            if let Some(prompt) = &request.prompt {
                obj[str_id]["inputs"]["value"] = json!(prompt);
            }
        } else if let Some(pos_id) = prepared.inject_points.get(&InjectRole::PositivePrompt) {
            if let Some(prompt) = &request.prompt {
                obj[pos_id]["inputs"]["text"] = json!(prompt);
            }
        }
        if let Some(str_id) = prepared.inject_points.get(&InjectRole::NegativePromptString) {
            if let Some(neg) = &request.negative_prompt {
                obj[str_id]["inputs"]["value"] = json!(neg);
            }
        } else if let Some(neg_id) = prepared.inject_points.get(&InjectRole::NegativePrompt) {
            if let Some(neg) = &request.negative_prompt {
                obj[neg_id]["inputs"]["text"] = json!(neg);
            }
        }

        // 尺寸
        let final_width = config_width.filter(|&w| w > 0)
            .or_else(|| request.size.as_ref().and_then(|s| s.split('x').next().and_then(|v| v.parse().ok())))
            .unwrap_or(0);
        let final_height = config_height.filter(|&h| h > 0)
            .or_else(|| request.size.as_ref().and_then(|s| s.split('x').nth(1).and_then(|v| v.parse().ok())))
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

        // 注入图片到保留的 LoadImage 节点（注意：多余的节点已经被删除，所以实际保留的数量等于 reference_count）
        // 我们仍使用 prepared.load_image_nodes 的前 reference_count 个，因为这些节点未被删除
        for (idx, node_id) in prepared.load_image_nodes.iter().take(reference_count).enumerate() {
            let filename = image_cache::cache_image(http_client, backend, &reference_images[idx].data).await?;
            obj[node_id]["inputs"]["image"] = json!(filename);
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



/// 从起始节点出发，收集所有依赖它的节点（包括间接依赖）
fn collect_downstream(start_node: &str, rev: &HashMap<String, Vec<String>>) -> HashSet<String> {
    let mut queue = VecDeque::new();
    let mut visited = HashSet::new();
    queue.push_back(start_node.to_string());
    visited.insert(start_node.to_string());
    while let Some(node) = queue.pop_front() {
        if let Some(deps) = rev.get(&node) {
            for dep in deps {
                if visited.insert(dep.clone()) {
                    queue.push_back(dep.clone());
                }
            }
        }
    }
    visited
}

fn prune_redundant_reference_branches(
    obj: &mut serde_json::Map<String, Value>,
    load_image_nodes: &[String],
    reference_count: usize,
) -> Result<(), ProxyError> {
    if reference_count >= load_image_nodes.len() {
        return Ok(());
    }
    if reference_count == 0 {
        return Ok(());
    }

    let (forward, _reverse) = build_forward_reverse_graphs(obj);

    // 1. 收集每个 LoadImage 对应的 positive 和 negative ReferenceLatent
    let mut load_to_positive = Vec::with_capacity(load_image_nodes.len());
    let mut load_to_negative = Vec::with_capacity(load_image_nodes.len());
    for load_id in load_image_nodes {
        let positive = find_reference_latent_for_load_image(&forward, obj, load_id);
        let negative = find_negative_reference_latent_for_load_image(&forward, obj, load_id, positive.as_ref());
        match (positive, negative) {
            (Some(pos), Some(neg)) => {
                load_to_positive.push((load_id.clone(), pos));
                load_to_negative.push((load_id.clone(), neg));
            }
            _ => {
                warn!("Missing ReferenceLatent(s) for LoadImage node: {}", load_id);
                // 如果缺少，则无法安全修剪，直接返回错误
                return Err(ProxyError::Internal(format!(
                    "Incomplete ReferenceLatent mapping for LoadImage: {}",
                    load_id
                )));
            }
        }
    }

    // 2. 确定需要删除的节点集合（从索引 reference_count 开始的所有分支）
    let mut nodes_to_remove = std::collections::HashSet::new();

    // 处理 positive 链
    for (load_id, latent_id) in load_to_positive.iter().skip(reference_count) {
        let mut branch = std::collections::HashSet::new();
        collect_branch_nodes_forward(&forward, obj, load_id, &mut branch);
        branch.insert(latent_id.clone());
        nodes_to_remove.extend(branch);
    }

    // 处理 negative 链
    for (load_id, latent_id) in load_to_negative.iter().skip(reference_count) {
        let mut branch = std::collections::HashSet::new();
        collect_branch_nodes_forward(&forward, obj, load_id, &mut branch);
        branch.insert(latent_id.clone());
        nodes_to_remove.extend(branch);
    }

    // 3. 保留的 positive 和 negative ReferenceLatent 列表（按顺序）
    let active_positive: Vec<String> = load_to_positive
        .iter()
        .take(reference_count)
        .map(|(_, id)| id.clone())
        .collect();
    let active_negative: Vec<String> = load_to_negative
        .iter()
        .take(reference_count)
        .map(|(_, id)| id.clone())
        .collect();

    // 4. 修复 positive 链内的 conditioning 连接（后一个指向前一个）
    for i in 1..active_positive.len() {
        let prev = &active_positive[i - 1];
        let curr = &active_positive[i];
        if let Some(node) = obj.get_mut(curr) {
            if let Some(inputs) = node.get_mut("inputs").and_then(|v| v.as_object_mut()) {
                inputs.insert("conditioning".to_string(), json!([prev, 0]));
                info!("Reconnected positive conditioning: {} -> {}", curr, prev);
            }
        }
    }

    // 修复 negative 链内的 conditioning 连接
    for i in 1..active_negative.len() {
        let prev = &active_negative[i - 1];
        let curr = &active_negative[i];
        if let Some(node) = obj.get_mut(curr) {
            if let Some(inputs) = node.get_mut("inputs").and_then(|v| v.as_object_mut()) {
                inputs.insert("conditioning".to_string(), json!([prev, 0]));
                info!("Reconnected negative conditioning: {} -> {}", curr, prev);
            }
        }
    }

    // 5. 修复外部节点（如 FluxKontextMulti 和 CFGGuider）的引用
    let last_positive = active_positive.last().expect("至少有一个 positive latent");
    let last_negative = active_negative.last().expect("至少有一个 negative latent");

    // 处理被删除的 positive latent 的下游节点
    for (_, latent_id) in load_to_positive.iter().skip(reference_count) {
        if let Some(dependents) = forward.get(latent_id) {
            for dep_id in dependents {
                if nodes_to_remove.contains(dep_id) || active_positive.contains(dep_id) {
                    continue;
                }
                if let Some(dep_node) = obj.get_mut(dep_id) {
                    if let Some(inputs) = dep_node.get_mut("inputs").and_then(|v| v.as_object_mut()) {
                        for (_, val) in inputs.iter_mut() {
                            if let Some(arr) = val.as_array_mut() {
                                if arr.len() >= 2 && arr[0].as_str() == Some(latent_id) {
                                    arr[0] = json!(last_positive);
                                    info!("Fixed positive reference in node {}: {} -> {}", dep_id, latent_id, last_positive);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 处理被删除的 negative latent 的下游节点
    for (_, latent_id) in load_to_negative.iter().skip(reference_count) {
        if let Some(dependents) = forward.get(latent_id) {
            for dep_id in dependents {
                if nodes_to_remove.contains(dep_id) || active_negative.contains(dep_id) {
                    continue;
                }
                if let Some(dep_node) = obj.get_mut(dep_id) {
                    if let Some(inputs) = dep_node.get_mut("inputs").and_then(|v| v.as_object_mut()) {
                        for (_, val) in inputs.iter_mut() {
                            if let Some(arr) = val.as_array_mut() {
                                if arr.len() >= 2 && arr[0].as_str() == Some(latent_id) {
                                    arr[0] = json!(last_negative);
                                    info!("Fixed negative reference in node {}: {} -> {}", dep_id, latent_id, last_negative);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 6. 删除所有标记的节点
    for node_id in nodes_to_remove {
        obj.remove(&node_id);
    }

    Ok(())
}

fn build_forward_reverse_graphs(
    obj: &serde_json::Map<String, Value>,
) -> (
    HashMap<String, Vec<String>>, // forward: node -> downstream nodes
    HashMap<String, Vec<String>>, // reverse: node -> upstream nodes
) {
    let mut forward: HashMap<String, Vec<String>> = HashMap::new();
    let mut reverse: HashMap<String, Vec<String>> = HashMap::new();

    for (node_id, node) in obj {
        if let Some(inputs) = node.get("inputs").and_then(|v| v.as_object()) {
            for value in inputs.values() {
                if let Some(arr) = value.as_array() {
                    if arr.len() >= 2 {
                        if let Some(src_id) = arr[0].as_str() {
                            forward.entry(src_id.to_string()).or_default().push(node_id.clone());
                            reverse.entry(node_id.clone()).or_default().push(src_id.to_string());
                        }
                    }
                }
            }
        }
    }
    (forward, reverse)
}

fn find_reference_latent_for_load_image(
    forward: &HashMap<String, Vec<String>>,
    obj: &serde_json::Map<String, Value>,
    start_node: &str,
) -> Option<String> {
    let mut queue = std::collections::VecDeque::new();
    let mut visited = std::collections::HashSet::new();
    queue.push_back(start_node.to_string());
    visited.insert(start_node.to_string());

    while let Some(node_id) = queue.pop_front() {
        if let Some(node) = obj.get(&node_id) {
            if node.get("class_type").and_then(|v| v.as_str()) == Some("ReferenceLatent") {
                return Some(node_id);
            }
        }
        if let Some(downstream) = forward.get(&node_id) {
            for next in downstream {
                if visited.insert(next.clone()) {
                    queue.push_back(next.clone());
                }
            }
        }
    }
    None
}

/// 从 LoadImage 节点出发，找到属于 negative 链的 ReferenceLatent 节点
/// 通过排除已知的 positive_latent_id 来区分
fn find_negative_reference_latent_for_load_image(
    forward: &HashMap<String, Vec<String>>,
    obj: &serde_json::Map<String, Value>,
    start_node: &str,
    positive_latent_id: Option<&String>,
) -> Option<String> {
    let mut queue = std::collections::VecDeque::new();
    let mut visited = std::collections::HashSet::new();
    queue.push_back(start_node.to_string());
    visited.insert(start_node.to_string());

    while let Some(node_id) = queue.pop_front() {
        if let Some(node) = obj.get(&node_id) {
            if node.get("class_type").and_then(|v| v.as_str()) == Some("ReferenceLatent") {
                // 如果是 positive 链的节点，跳过
                if positive_latent_id == Some(&node_id) {
                    // 继续搜索，不返回
                } else {
                    return Some(node_id);
                }
            }
        }
        if let Some(downstream) = forward.get(&node_id) {
            for next in downstream {
                if visited.insert(next.clone()) {
                    queue.push_back(next.clone());
                }
            }
        }
    }
    None
}

// 从 start_node 出发，沿 forward 图收集所有节点，直到遇到 ReferenceLatent（包括它）
fn collect_branch_nodes_forward(
    forward: &HashMap<String, Vec<String>>,
    obj: &serde_json::Map<String, Value>,
    start_node: &str,
    visited: &mut std::collections::HashSet<String>,
) {
    if visited.contains(start_node) {
        return;
    }
    visited.insert(start_node.to_string());
    if let Some(node) = obj.get(start_node) {
        if node.get("class_type").and_then(|v| v.as_str()) == Some("ReferenceLatent") {
            return;
        }
    }
    if let Some(downstream) = forward.get(start_node) {
        for next in downstream {
            collect_branch_nodes_forward(forward, obj, next, visited);
        }
    }
}
