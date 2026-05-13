use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response as AxumResponse;
use axum::body::Body;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::timeout;
use rand::Rng;
use tracing::{debug, error, info, warn};
use regex::Regex;

use crate::error::{ProxyError, handle_request_error};
use crate::proxy::ProxyState;
use crate::task_manager::TaskState;
use crate::cache::image_cache;
use crate::transport::poll::{poll_history_for_videos, VideoOutput};
use crate::workflows::template::{PreparedWorkflow, InjectRole};
use crate::config::BackendConfig;
// 在现有 use 下添加：
use base64::{Engine as _};   // 已经有的话就确保有这一行

const DEFAULT_FPS: f64 = 24.0;

#[derive(Debug, Deserialize)]
pub struct VideoGenerationRequest {
    pub model: String,
    pub content: Vec<ContentItem>,
    pub ratio: Option<String>,
    pub duration: Option<u32>,
    pub resolution: Option<String>,
    #[allow(dead_code)]
    pub watermark: Option<bool>,
    pub reference_roles: Option<Vec<String>>,
    #[serde(default)]
    pub local_prompts: Option<String>,
    #[serde(default)]
    pub segment_lengths: Option<String>,
    #[serde(default)]
    pub guide_strengths: Option<Vec<f32>>,
    #[serde(default)]
    pub global_prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ContentItem {
    #[serde(rename = "type")]
    pub item_type: String,
    pub text: Option<String>,
    pub image_url: Option<ImageUrlWrapper>,
    pub role: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ImageUrlWrapper {
    pub url: Option<String>,
}

// handlers/video.rs 中的 video_generations_handler 函数修正后版本
pub async fn video_generations_handler(
    State(state): State<Arc<ProxyState>>,
    Query(params): Query<HashMap<String, String>>,
    _headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<AxumResponse, ProxyError> {
    let raw_body = String::from_utf8_lossy(&body);
    info!("📥 Video request: {}", crate::handlers::image::sanitize_log_body(&raw_body));

    // 优雅关闭检查
    if state.graceful_shutdown.is_shutting_down() {
        return Err(ProxyError::Internal("Server is shutting down".into()));
    }

    let backend_name = params.get("backend").map(|s| s.as_str());
    let backend = state.get_backend(backend_name)?.clone();
    let video_req: VideoGenerationRequest = serde_json::from_str(&raw_body)
        .map_err(|e| ProxyError::Json(format!("Invalid video request: {}", e)))?;

    let prompt = video_req.content.iter()
        .find(|c| c.item_type == "text")
        .and_then(|c| c.text.clone())
        .unwrap_or_default();

    let reference_urls: Vec<String> = video_req.content.iter()
        .filter(|c| c.item_type == "image_url" && c.role.as_deref() == Some("reference_image"))
        .filter_map(|c| c.image_url.as_ref()?.url.clone())
        .collect();

    let (mut width, mut height) = parse_resolution(&video_req.resolution, &video_req.ratio);
    if let Some(cfg_w) = state.video_width {
        if cfg_w > 0 { width = cfg_w; }
    }
    if let Some(cfg_h) = state.video_height {
        if cfg_h > 0 { height = cfg_h; }
    }

    let task_id = format!("vid-{}-{}", 
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis(),
        rand::random::<u16>()
    );
    state.task_manager.insert(task_id.clone(), TaskState::Processing).await;

    let state_clone = state.clone();
    let tm = state.task_manager.clone();
    let backend_for_generation = backend.clone();   // 传入执行函数
    let backend_for_result = backend.clone();       // 保留用于生成 URL
    let job_timeout = state.job_timeout_seconds;
    let task_id_clone = task_id.clone();

    tokio::spawn(async move {
        let res = execute_video_generation(
            &state_clone,
            &video_req,
            &prompt,
            &reference_urls,
            width,
            height,
            job_timeout,
            backend_for_generation,
        ).await;
        match res {
            Ok(videos) => {
                let url = videos.first().map(|v| {
                    format!("http://{}:{}/view?filename={}&subfolder={}&type=output",
                        backend_for_result.host, backend_for_result.port, v.filename, v.subfolder)
                });
                let b64 = videos.first().map(|v| base64::engine::general_purpose::STANDARD.encode(&v.bytes));
                tm.update(&task_id_clone, TaskState::Completed { video_url: url, b64_json: b64 }).await;
                info!("✅ Video task {} completed", task_id_clone);
            }
            Err(e) => {
                tm.update(&task_id_clone, TaskState::Failed { error: e.to_string() }).await;
                error!("❌ Video task {} failed: {}", task_id_clone, e);
            }
        }
    });

    let response = json!({ "task_id": task_id });
    let body = serde_json::to_vec(&response)?;
    Ok(AxumResponse::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(Body::from(body))
        .unwrap())
}

pub async fn video_health_handler() -> impl axum::response::IntoResponse {
    (StatusCode::OK, "Video generation service is running")
}

async fn execute_video_generation(
    state: &Arc<ProxyState>,
    req: &VideoGenerationRequest,
    prompt: &str,
    reference_urls: &[String],
    width: u32,
    height: u32,
    job_timeout: u64,
    backend: BackendConfig,
) -> Result<Vec<VideoOutput>, ProxyError> {
    let target_base = format!("{}:{}", backend.host, backend.port);

    if state.free_model_before_video {
        let free_url = format!("http://{}/free", target_base);
        info!("🧹 Freeing ComfyUI memory before video task...");
        let resp = state.client
            .post(&free_url)
            .json(&json!({"unload_models": true, "free_memory": true}))
            .send()
            .await
            .map_err(|e| handle_request_error(e, &free_url))?;
        if !resp.status().is_success() {
            return Err(ProxyError::Upstream(format!("ComfyUI /free failed: {}", resp.status())));
        }
    }

    let template = state.registry.get(&req.model)
        .ok_or_else(|| ProxyError::Json(format!("Workflow '{}' not found", req.model)))?;
    let prepared = PreparedWorkflow::from_template(&template);

    let payload = create_video_payload(
        req,
        &prepared,
        prompt,
        reference_urls,
        width,
        height,
        &state.client_id,
        &state.client,
        state.default_fps,
        &backend,
    ).await?;

    let target_url = format!("http://{}/prompt", target_base);
    info!("🚀 Submitting video workflow to ComfyUI: {}", target_url);
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

    let videos = timeout(
        Duration::from_secs(job_timeout),
        poll_history_for_videos(&target_base, &prompt_id, &state.client, job_timeout),
    )
    .await
    .map_err(|_| ProxyError::Upstream("Video generation timeout".into()))??;

    Ok(videos)
}

async fn create_video_payload(
    req: &VideoGenerationRequest,
    prepared: &PreparedWorkflow,
    prompt: &str,
    reference_urls: &[String],
    width: u32,
    height: u32,
    client_id: &str,
    http_client: &Client,
    default_fps: Option<f64>,
    backend: &BackendConfig,
) -> Result<Value, ProxyError> {
    let mut workflow = prepared.raw.clone();
    let duration = req.duration.unwrap_or(5) as f64;
    let fps = default_fps.unwrap_or(DEFAULT_FPS);

    if let Some(obj) = workflow.as_object_mut() {
        // 1. 随机种子
        for (_, node) in obj.iter_mut() {
            if node["class_type"].as_str() == Some("RandomNoise") {
                if let Some(inputs) = node["inputs"].as_object_mut() {
                    inputs.insert("noise_seed".to_string(),
                        Value::String(rand::thread_rng().gen_range(0..=u64::MAX).to_string()));
                }
            }
        }

        // 2. 提示词
        if let Some(pos_id) = prepared.inject_points.get(&InjectRole::PositivePrompt) {
            if let Some(node) = obj.get_mut(pos_id) {
                if let Some(inputs) = node["inputs"].as_object_mut() {
                    inputs.insert("text".to_string(), Value::String(prompt.to_string()));
                }
            }
        }
        if let Some(neg_id) = prepared.inject_points.get(&InjectRole::NegativePrompt) {
            if let Some(node) = obj.get_mut(neg_id) {
                if let Some(inputs) = node["inputs"].as_object_mut() {
                    inputs.insert("text".to_string(),
                        Value::String("low quality, blurry, worst quality".to_string()));
                }
            }
        }

        // 3. 宽高
        if let Some(width_id) = prepared.inject_points.get(&InjectRole::Width) {
            if let Some(node) = obj.get_mut(width_id) {
                let ct = node["class_type"].as_str().unwrap_or("").to_string();
                if let Some(inputs) = node["inputs"].as_object_mut() {
                    inject_value(inputs, &ct, json!(width));
                }
            }
        }
        if let Some(height_id) = prepared.inject_points.get(&InjectRole::Height) {
            if let Some(node) = obj.get_mut(height_id) {
                let ct = node["class_type"].as_str().unwrap_or("").to_string();
                if let Some(inputs) = node["inputs"].as_object_mut() {
                    inject_value(inputs, &ct, json!(height));
                }
            }
        }

        // 4. 参考图
        let ref_count = reference_urls.len();
        if !prepared.load_image_nodes.is_empty() {
            if ref_count == 0 {
                let placeholder = image_cache::get_placeholder_filename(http_client, backend).await?;
                for node_id in &prepared.load_image_nodes {
                    obj[node_id]["inputs"]["image"] = json!(placeholder);
                }
            } else {
                for (i, node_id) in prepared.load_image_nodes.iter().enumerate() {
                    let filename = if i < ref_count {
                        image_cache::cache_image(http_client, backend, &reference_urls[i]).await?
                    } else {
                        image_cache::get_3x3_placeholder_filename(http_client, backend).await?
                    };
                    obj[node_id]["inputs"]["image"] = json!(filename);
                }
            }
        }

        // 5. Duration
        let duration_val = duration;
        if let Some(dur_node) = prepared.duration_node.as_ref() {
            if let Some(node) = obj.get_mut(dur_node) {
                let ct = node["class_type"].as_str().unwrap_or("").to_string();
                if let Some(inputs) = node["inputs"].as_object_mut() {
                    inject_value(inputs, &ct, json!(duration_val));
                }
            }
        } else {
            for (_, node) in obj.iter_mut() {
                let title = node["_meta"]["title"].as_str().unwrap_or("").to_lowercase();
                if title == "duration" {
                    let ct = node["class_type"].as_str().unwrap_or("").to_string();
                    if let Some(inputs) = node["inputs"].as_object_mut() {
                        inject_value(inputs, &ct, json!(duration_val));
                    }
                    break;
                }
            }
        }

        // 6. FPS
        let fps_val = fps;
        if let Some(fps_node) = prepared.fps_node.as_ref() {
            if let Some(node) = obj.get_mut(fps_node) {
                let ct = node["class_type"].as_str().unwrap_or("").to_string();
                if let Some(inputs) = node["inputs"].as_object_mut() {
                    inject_value(inputs, &ct, json!(fps_val));
                }
            }
        } else {
            for (_, node) in obj.iter_mut() {
                let title = node["_meta"]["title"].as_str().unwrap_or("").to_lowercase();
                if title == "fps" {
                    let ct = node["class_type"].as_str().unwrap_or("").to_string();
                    if let Some(inputs) = node["inputs"].as_object_mut() {
                        inject_value(inputs, &ct, json!(fps_val));
                    }
                    break;
                }
            }
        }

        // 7. 总帧数
        let theoretical = (duration_val * fps_val).round() as u32 + 1;
        let total_frames = if theoretical % 8 == 1 { theoretical } else { ((theoretical + 7) / 8) * 8 + 1 };

        // 8. LTXV guides
        for (node_id, num_guides) in &prepared.ltxv_add_guide_multi_nodes {
            if let Some(node) = obj.get_mut(node_id) {
                if let Some(inputs) = node["inputs"].as_object_mut() {
                    let old: Vec<String> = inputs.keys()
                        .filter(|k| k.starts_with("frame_idx_") || k.starts_with("strength_"))
                        .cloned().collect();
                    for k in old { inputs.remove(&k); }

                    let cnt = std::cmp::min(reference_urls.len(), *num_guides);
                    for gi in 0..cnt {
                        let frame_idx = if *num_guides > 1 && total_frames > 1 {
                            ((gi as u32 * total_frames) / (*num_guides as u32)).min(total_frames.saturating_sub(1))
                        } else { 0 };
                        let frame_aligned = (frame_idx / 8) * 8;
                        let strength = req.guide_strengths.as_ref().and_then(|v| v.get(gi).copied()).unwrap_or(0.8);
                        inputs.insert(format!("frame_idx_{}", gi + 1), json!(frame_aligned.min(total_frames.saturating_sub(1))));
                        inputs.insert(format!("strength_{}", gi + 1), json!(strength as f64));
                    }
                    for gi in cnt..*num_guides {
                        inputs.insert(format!("frame_idx_{}", gi + 1), json!(0));
                        inputs.insert(format!("strength_{}", gi + 1), json!(0.0));
                    }
                }
            }
        }

        // 9. PromptRelayEncode
        if prepared.has_prompt_relay {
            for (_, node) in obj.iter_mut() {
                if node["class_type"].as_str() == Some("PromptRelayEncode") {
                    let global = req.global_prompt.as_deref().unwrap_or(prompt);
                    let (local, seg) = enhance_prompt_relay(req.local_prompts.as_deref(), global, duration);
                    if let Some(inputs) = node["inputs"].as_object_mut() {
                        inputs.insert("global_prompt".to_string(), json!(global));
                        inputs.insert("local_prompts".to_string(), json!(local));
                        if !seg.is_empty() { inputs.insert("segment_lengths".to_string(), json!(seg)); }
                    }
                    break;
                }
            }
        }

        // 10. 旧式 SEGMENT 帧索引
        let mut updates = vec![];
        for (id, node) in obj.iter() {
            let title = node["_meta"]["title"].as_str().unwrap_or("");
            if title.starts_with("SEGMENT") && title.contains("FRAMES") {
                let parts: Vec<&str> = title.split_whitespace().collect();
                let seg_num = parts.get(1).and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);
                let seg_idx = seg_num.saturating_sub(1);
                let seg_count = prepared.load_image_nodes.len() as u32;
                let f_idx = if seg_count > 0 { seg_idx as u32 * total_frames / seg_count } else { 0 };
                updates.push((id.clone(), f_idx));
            }
        }
        for (id, f_idx) in updates {
            if let Some(node) = obj.get_mut(&id) {
                let ct = node["class_type"].as_str().unwrap_or("").to_string();
                if let Some(inputs) = node["inputs"].as_object_mut() {
                    inject_value(inputs, &ct, json!(f_idx));
                }
            }
        }
    }

    Ok(json!({"prompt": workflow, "client_id": client_id}))
}

fn inject_value(inputs: &mut serde_json::Map<String, Value>, class_type: &str, val: Value) {
    match class_type {
        "PrimitiveFloat" | "FloatSlider" => {
            let v = if let Some(f) = val.as_f64() { json!(f) }
                    else if let Some(n) = val.as_u64() { json!(n as f64) }
                    else { val };
            inputs.insert("value".to_string(), v);
        }
        "PrimitiveInt" | "INTConstant" => {
            let v = if let Some(n) = val.as_u64() { json!(n as i64) }
                    else if let Some(f) = val.as_f64() { json!(f as i64) }
                    else { val };
            inputs.insert("value".to_string(), v);
        }
        _ => { inputs.insert("value".to_string(), val); }
    }
}

fn enhance_prompt_relay(
    local_prompts: Option<&str>,
    global_prompt: &str,
    duration_sec: f64,
) -> (String, String) {
    let raw = local_prompts.unwrap_or("");
    let re = Regex::new(r"\[(\d+\.?\d*)\s*-\s*(\d+\.?\d*)\]").unwrap();
    if re.is_match(raw) {
        let mut segments = vec![];
        let mut caps = re.captures_iter(raw);
        let mut last_end = 0.0;
        let mut _i = 0;
        while let Some(cap) = caps.next() {
            let start: f64 = cap[1].parse().unwrap_or(0.0);
            let end: f64 = cap[2].parse().unwrap_or(duration_sec);
            let text = raw.split(&cap[0]).nth(1).and_then(|s| s.split('\n').next()).unwrap_or("").trim();
            segments.push(format!("[{} - {}]\n{}", start, end, text));
            last_end = end;
            _i += 1;
        }
        if last_end < duration_sec {
            segments.push(format!("[{:.1} - {:.1}]\nRemaining", last_end, duration_sec));
        }
        let seg_count = segments.len() as f64;
        let lengths = segments.iter().map(|_| format!("{:.0}", duration_sec / seg_count)).collect::<Vec<_>>().join("|");
        (segments.join("|"), lengths)
    } else {
        let parts: Vec<&str> = raw.split('|').collect();
        if parts.len() >= 2 && parts.iter().all(|p| !p.trim().is_empty()) {
            (raw.to_string(), "".to_string())
        } else {
            (format!("[0.0 - {:.1}]\n{}|\n[remaining]", duration_sec, global_prompt), "".to_string())
        }
    }
}

fn parse_resolution(resolution: &Option<String>, ratio: &Option<String>) -> (u32, u32) {
    match resolution.as_deref() {
        Some("720p") => (1280, 704),
        Some("1080p") => (1920, 1088),
        _ => {
            if let Some(r) = ratio {
                let parts: Vec<&str> = r.split(':').collect();
                if parts.len() == 2 {
                    let w: u32 = parts[0].parse().unwrap_or(16);
                    let h: u32 = parts[1].parse().unwrap_or(9);
                    let raw_h = 1280 * h / w;
                    let adjusted_h = (raw_h / 64) * 64;
                    return (1280, if adjusted_h == 0 { 64 } else { adjusted_h });
                }
            }
            (1280, 704)
        }
    }
}