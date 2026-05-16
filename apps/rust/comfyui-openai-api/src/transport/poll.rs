use reqwest::Client;
use serde_json::{Map, Value};
use std::time::{Duration, Instant};
use tracing::{debug, error, warn};
use crate::error::ProxyError;

fn jittered_backoff(attempt: u64, base_ms: u64, cap_ms: u64) -> Duration {
    let exp = base_ms * 2u64.pow(attempt.min(10) as u32);
    let capped = exp.min(cap_ms);
    let jitter = rand::random::<u64>() % (capped + 1);
    Duration::from_millis(capped + jitter / 2)
}

fn queue_contains_pid(queue_json: &Value, pid: &str) -> bool {
    let candidate_fields = ["queue_running", "queue_pending", "queue", "jobs", "running"];
    for field in &candidate_fields {
        if let Some(items) = queue_json[field].as_array() {
            for item in items {
                if item.as_str() == Some(pid) {
                    return true;
                }
                if let Some(obj) = item.as_object() {
                    for key in ["id", "job_id", "pid", "uuid", "task_id"] {
                        if obj.get(key).and_then(|v| v.as_str()) == Some(pid) {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

/// 检查任务是否在队列的任何位置（运行中、待处理等）
fn task_in_any_queue(queue_json: &Value, pid: &str) -> bool {
    queue_contains_pid(queue_json, pid)
}

fn get_job_from_history<'a>(history: &'a Value, pid: &str) -> Option<&'a Map<String, Value>> {
    if let Some(job) = history.get(pid).and_then(|v| v.as_object()) {
        return Some(job);
    }
    if let Some(job) = history.as_object() {
        if job.contains_key("status") || job.contains_key("outputs") || job.contains_key("node_errors") {
            return Some(job);
        }
    }
    None
}

pub async fn poll_history_for_images(
    base: &str,
    pid: &str,
    client: &Client,
    timeout_secs: u64,
) -> Result<Vec<(String, Vec<u8>)>, ProxyError> {
    let history_url = format!("http://{}/history/{}", base, pid);
    let queue_url = format!("http://{}/queue", base);
    let max_duration = Duration::from_secs(timeout_secs);
    let start = Instant::now();
    let mut attempt = 0u64;
    let mut missing_pid_streak: u32 = 0;
    const MAX_MISSING_STREAK: u32 = 10;

    loop {
        if start.elapsed() > max_duration {
            return Err(ProxyError::Upstream("Timeout waiting for image".into()));
        }

        let resp = client.get(&history_url).send().await
            .map_err(|e| {
                warn!("History fetch failed for {}: {}", history_url, e);
                ProxyError::Upstream(format!("History fetch: {}", e))
            })?;
        let mut history: Value = resp.json().await
            .map_err(|e| ProxyError::Json(format!("JSON parse: {}", e)))?;
        debug!("poll_history_for_images attempt={} pid={} history={:?}", attempt, pid, history);

        let mut maybe_job = get_job_from_history(&history, pid);
        // if maybe_job.is_none() {
        //     if let Ok(queue_resp) = client.get(&queue_url).send().await {
        //         if let Ok(queue_json) = queue_resp.json::<Value>().await {
        //             debug!("poll_history_for_images pid={} queue={:?}", pid, queue_json);
        //             // 检查 queue_running（运行） 和 queue_pending（等待）队列是否有对应任务id
        //             let pid_found = 
        //             // json格式检索，
        //             queue_json.get("queue_running")
        //                 .and_then(|v| v.as_array())
        //                 .map(|arr| arr.iter().any(|item| {
        //                     item.as_array()
        //                         .and_then(|inner| inner.get(1))          // 取第二个元素，索引 1
        //                         .and_then(|uuid_val| uuid_val.as_str())  // 转为 &str
        //                         .map(|uuid| uuid == pid)                 // 与 pid 比较（pid 是 &str）
        //                         .unwrap_or(false)
        //                 }))
        //                 .unwrap_or(false)
        //             ||
        //             queue_json.get("queue_pending")
        //                 .and_then(|v| v.as_array())
        //                 .map(|arr| arr.iter().any(|item| {
        //                     item.as_array()
        //                         .and_then(|inner| inner.get(1))
        //                         .and_then(|uuid_val| uuid_val.as_str())
        //                         .map(|uuid| uuid == pid)
        //                         .unwrap_or(false)
        //                 })).unwrap_or(false);
        //             if !pid_found {
        //                 warn!("poll_history_for_images pid={} not found in queue, rechecking history", pid);
        //                 let recheck_resp = client.get(&history_url).send().await
        //                     .map_err(|e| {
        //                         warn!("History recheck failed for {}: {}", history_url, e);
        //                         ProxyError::Upstream(format!("History fetch: {}", e))
        //                     })?;
        //                 let history_recheck: Value = recheck_resp.json().await
        //                     .map_err(|e| ProxyError::Json(format!("JSON parse: {}", e)))?;
        //                 warn!("history_url_recheck:{}", history_recheck);
        //                 history = history_recheck;
        //                 maybe_job = get_job_from_history(&history, pid);
        //                 if maybe_job.is_none() {
        //                     error!("ComfyUI job {} 任务超时或被终止！", pid);
        //                     return Err(ProxyError::Upstream(format!("ComfyUI task {} 超时或被终止，请重新生成", pid)));
        //                 }
        //             }
        //         }
        //     }
        // }

        if let Some(job) = maybe_job {
            debug!("poll_history_for_images pid={} job={:?}", pid, job);
            // 错误处理：检查 status 或 status_str 字段，看返回结果status_str是包含在status的object里面，所以额外在status里查询status_str
            let status = job.get("status")
            .and_then(|s| s.get("status_str"))
            .and_then(|v| v.as_str())
            .or_else(|| job.get("status_str").and_then(|v| v.as_str()));
            //warn!("读到状态结果:{:?}",status);
            if let Some(status) = status {
                if matches!(status, "error" | "failed" | "aborted" | "exception" | "canceled" | "cancelled") {
                    let mut msgs = Vec::new();
                    if let Some(msg) = job.get("status_message").and_then(|v| v.as_str()) {
                        msgs.push(msg.to_string());
                    }
                    if let Some(msg) = job.get("error").and_then(|v| v.as_str()) {
                        msgs.push(msg.to_string());
                    }
                    if let Some(node_errors) = job.get("node_errors").and_then(|v| v.as_object()) {
                        for (node_id, err_info) in node_errors {
                            if let Some(errors) = err_info.get("errors").and_then(|v| v.as_array()) {
                                for err in errors {
                                    let msg = format!("Node {}: {}",
                                        node_id,
                                        err.get("message").and_then(|v| v.as_str()).unwrap_or("unknown"));
                                    msgs.push(msg);
                                }
                            }
                        }
                    }
                    let full = if msgs.is_empty() {
                        status.to_string()
                    } else {
                        msgs.join("; ")
                    };
                    error!("ComfyUI job {} failed: {} 任务超时或被终止", pid, full);
                    return Err(ProxyError::Upstream(format!("ComfyUI task {} failed: {} 任务超时或被终止，请重新生成！", pid, full)));
                }
            }

            let mut images_info = Vec::new();
            if let Some(outputs) = job.get("outputs").and_then(|v| v.as_object()) {
                for (_, out) in outputs {
                    if let Some(imgs) = out.get("images").and_then(|v| v.as_array()) {
                        for img in imgs {
                            let img_type = img.get("type").and_then(|v| v.as_str()).unwrap_or("output");
                            if img_type == "output" {
                                images_info.push((
                                    img.get("filename").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                    img.get("subfolder").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                ));
                            }
                        }
                    }
                }
            }

            if !images_info.is_empty() {
                let mut handles = Vec::new();
                for (filename, subfolder) in images_info {
                    let client = client.clone();
                    let base = base.to_string();
                    handles.push(tokio::spawn(async move {
                        let view_url = format!(
                            "http://{}/view?filename={}&subfolder={}&type=output",
                            base, filename, subfolder
                        );
                        let resp = client.get(&view_url).send().await?;
                        Ok((filename, resp.bytes().await?.to_vec()))
                    }));
                }
                let mut images = Vec::with_capacity(handles.len());
                for h in handles {
                    images.push(
                        h.await
                            .map_err(|_| ProxyError::Internal("Join error".into()))?
                            .map_err(|e: reqwest::Error| ProxyError::Upstream(format!("Download: {}", e)))?
                    );
                }
                return Ok(images);
            }
        } else {
            missing_pid_streak += 1;
            warn!("poll_history_for_images attempt={} pid={} history entry missing (streak={})", attempt, pid, missing_pid_streak);

            if missing_pid_streak >= MAX_MISSING_STREAK {
                warn!("History entry for pid {} missing for {} attempts", pid, missing_pid_streak);
                let queue_resp = match client.get(&queue_url).send().await {
                    Ok(r) => r,
                    Err(e) => {
                        return Err(ProxyError::Upstream(format!("Job '{}' lost and queue unreachable: {}", pid, e)));
                    }
                };

                if let Ok(queue_json) = queue_resp.json::<Value>().await {
                    debug!("poll_history_for_images pid={} queue={:?}", pid, queue_json);
                    let running = queue_json.get("queue_running").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
                    //修改 let pid_found = queue_contains_pid(&queue_json, pid);
                    let pid_found = 
                    // json格式检索，
                    queue_json.get("queue_running")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().any(|item| {
                            item.as_array()
                                .and_then(|inner| inner.get(1))          // 取第二个元素，索引 1
                                .and_then(|uuid_val| uuid_val.as_str())  // 转为 &str
                                .map(|uuid| uuid == pid)                 // 与 pid 比较（pid 是 &str）
                                .unwrap_or(false)
                        }))
                        .unwrap_or(false)
                    ||
                    queue_json.get("queue_pending")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().any(|item| {
                            item.as_array()
                                .and_then(|inner| inner.get(1))
                                .and_then(|uuid_val| uuid_val.as_str())
                                .map(|uuid| uuid == pid)
                                .unwrap_or(false)
                        })).unwrap_or(false);
                    debug!("poll_history_for_images pid={} found_in_queue={} running={}", pid, pid_found, running);
                    if running == 0 || !pid_found {
                        return Err(ProxyError::Upstream(format!("Job '{}' lost (history missing and pid not present in queue)", pid)));
                    }
                    missing_pid_streak = 0;
                } else {
                    return Err(ProxyError::Upstream(format!("Job '{}' lost (history missing and queue parse failed)", pid)));
                }
            }
        }

        attempt += 1;
        tokio::time::sleep(jittered_backoff(attempt, 1000, 5000)).await;
    }
}

pub struct VideoOutput {
    pub filename: String,
    pub subfolder: String,
    pub bytes: Vec<u8>,
}

pub async fn poll_history_for_videos(
    base: &str,
    pid: &str,
    client: &Client,
    timeout_secs: u64,
) -> Result<Vec<VideoOutput>, ProxyError> {
    let history_url = format!("http://{}/history/{}", base, pid);
    let queue_url = format!("http://{}/queue", base);
    let start = Instant::now();
    let absolute_timeout = Duration::from_secs(timeout_secs * 2); // 放宽超时
    let mut attempt = 0u64;
    let mut missing_pid_streak: u32 = 0;
    const MAX_MISSING_STREAK: u32 = 10;

    loop {
        if start.elapsed() > absolute_timeout {
            error!("Absolute timeout reached for job {}", pid);
            return Err(ProxyError::Upstream(format!(
                "Job '{}' timed out after {} seconds",
                pid, absolute_timeout.as_secs()
            )));
        }

        let resp = match client.get(&history_url).send().await {
            Ok(r) => r,
            Err(e) => {
                warn!("History fetch error: {}", e);
                tokio::time::sleep(jittered_backoff(attempt, 1000, 5000)).await;
                attempt += 1;
                continue;
            }
        };
        let history: Value = match resp.json().await {
            Ok(h) => h,
            Err(e) => {
                warn!("History JSON parse error: {}", e);
                tokio::time::sleep(jittered_backoff(attempt, 1000, 5000)).await;
                attempt += 1;
                continue;
            }
        };
        debug!("poll_history_for_videos attempt={} pid={} history={:?}", attempt, pid, history);
        
        if let Some(job) = get_job_from_history(&history, pid) {
            //warn!("poll_history_for_videos pid={} job={:?}", pid, job);
            debug!("poll_history_for_videos pid={} job={:?}", pid, job);
            missing_pid_streak = 0;

            // 错误检测：检查 status 或 status_str 字段
            let mut has_error = false;
            let mut error_msgs = Vec::new();
            // let status = job.get("status").and_then(|v| v.as_str())
            //     .or_else(|| job.get("status_str").and_then(|v| v.as_str()));
            let status = job.get("status")
            .and_then(|s| s.get("status_str"))
            .and_then(|v| v.as_str())
            .or_else(|| job.get("status_str").and_then(|v| v.as_str()));
            warn!("读到状态结果:{:?}",status);
            if let Some(status) = status {
                if matches!(status, "error" | "exception" | "failed" | "aborted" | "canceled" | "cancelled") {
                    has_error = true;
                    if let Some(msg) = job.get("status_message").and_then(|v| v.as_str()) {
                        error_msgs.push(msg.to_string());
                    }
                }
            }
            if let Some(node_errors) = job.get("node_errors").and_then(|v| v.as_object()) {
                if !node_errors.is_empty() {
                    has_error = true;
                    for (node_id, err_info) in node_errors {
                        if let Some(errors) = err_info.get("errors").and_then(|v| v.as_array()) {
                            for err in errors {
                                let msg = format!("Node {}: {}",
                                    node_id,
                                    err.get("message").and_then(|v| v.as_str()).unwrap_or("unknown"));
                                error_msgs.push(msg);
                            }
                        }
                    }
                }
            }
            if has_error {
                let full = error_msgs.join("; ");
                error!("ComfyUI job error: {}", full);
                return Err(ProxyError::Upstream(format!("ComfyUI error: {}", full)));
            }

            // 收集视频输出
            let mut videos_info = Vec::new();
            if let Some(outputs) = job.get("outputs").and_then(|v| v.as_object()) {
                for (_, out) in outputs {
                    for field in &["videos", "video", "gifs", "images"] {
                        if let Some(items) = out.get(*field).and_then(|v| v.as_array()) {
                            for item in items {
                                let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                if item_type == "output" || item_type.is_empty() {
                                    videos_info.push((
                                        item.get("filename").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                        item.get("subfolder").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                    ));
                                }
                            }
                        }
                        if !videos_info.is_empty() {
                            break;
                        }
                    }
                }
            }

            if !videos_info.is_empty() {
                let mut handles = Vec::new();
                for (filename, subfolder) in videos_info {
                    let client = client.clone();
                    let base = base.to_string();
                    handles.push(tokio::spawn(async move {
                        let view_url = format!(
                            "http://{}/view?filename={}&subfolder={}&type=output",
                            base, filename, subfolder
                        );
                        let resp = client.get(&view_url).send().await?;
                        let bytes = resp.bytes().await?;
                        Ok::<VideoOutput, reqwest::Error>(VideoOutput {
                            filename,
                            subfolder,
                            bytes: bytes.to_vec(),
                        })
                    }));
                }
                let mut videos = Vec::with_capacity(handles.len());
                for h in handles {
                    match h.await {
                        Ok(Ok(v)) => videos.push(v),
                        _ => return Err(ProxyError::Upstream("Failed to download video".into())),
                    }
                }
                return Ok(videos);
            }
        } else {
            missing_pid_streak += 1;
            warn!("poll_history_for_videos attempt={} pid={} history entry missing (streak={})", attempt, pid, missing_pid_streak);
            if missing_pid_streak >= MAX_MISSING_STREAK {
                // 检查队列确认任务是否丢失
                let queue_resp = match client.get(&queue_url).send().await {
                    Ok(r) => r,
                    Err(_) => {
                        return Err(ProxyError::Upstream(format!("Job '{}' lost and queue unreachable", pid)));
                    }
                };
                if let Ok(queue_json) = queue_resp.json::<Value>().await {
                    debug!("poll_history_for_videos pid={} queue={:?}", pid, queue_json);
                    let running = queue_json.get("queue_running").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
                    //let pid_found = queue_contains_pid(&queue_json, pid);
                    // 检查 queue_running（运行） 和 queue_pending（等待）队列是否有对应任务id
                    let pid_found = 
                    // json格式检索，
                    queue_json.get("queue_running")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().any(|item| {
                            item.as_array()
                                .and_then(|inner| inner.get(1))          // 取第二个元素，索引 1
                                .and_then(|uuid_val| uuid_val.as_str())  // 转为 &str
                                .map(|uuid| uuid == pid)                 // 与 pid 比较（pid 是 &str）
                                .unwrap_or(false)
                        }))
                        .unwrap_or(false)
                    ||
                    queue_json.get("queue_pending")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().any(|item| {
                            item.as_array()
                                .and_then(|inner| inner.get(1))
                                .and_then(|uuid_val| uuid_val.as_str())
                                .map(|uuid| uuid == pid)
                                .unwrap_or(false)
                        })).unwrap_or(false);
                    
                    warn!("running: {} and pid_found: {}", running, pid_found);
                    debug!("poll_history_for_videos pid={} found_in_queue={} running={}", pid, pid_found, running);
                    if running == 0 || !pid_found {
                        return Err(ProxyError::Upstream(format!("Job '{}' lost (not in history and pid not present in queue)", pid)));
                    }
                    // 队列还在运行且本 pid 仍然存在，重置计数
                    missing_pid_streak = 0;
                } else {
                    return Err(ProxyError::Upstream(format!("Job '{}' lost", pid)));
                }
            }
        }

        attempt += 1;
        tokio::time::sleep(jittered_backoff(attempt, 1000, 5000)).await;
    }
}