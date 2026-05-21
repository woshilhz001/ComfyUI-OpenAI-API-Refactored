use std::sync::Arc;
use tracing::{info, warn};
use reqwest::Client;
use serde_json::Value;

use crate::error::ProxyError;
use crate::proxy::ProxyState;
use crate::task_manager::TaskState;
use crate::transport::poll::{poll_history_for_videos, poll_history_for_images, VideoOutput};
use crate::utils::format_file_info;

/// 查询任务在 ComfyUI 中的最终状态（已完成或失败）
async fn fetch_job_final_status(base: &str, prompt_id: &str, client: &Client) -> Option<TaskState> {
    let history_url = format!("http://{}/history/{}", base, prompt_id);
    match client.get(&history_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(history) = resp.json::<Value>().await {
                // 检查是否包含该 prompt_id
                if let Some(job) = history.get(prompt_id).and_then(|v| v.as_object()) {
                    // 判断是否失败
                    let status = job.get("status")
                        .and_then(|s| s.get("status_str"))
                        .and_then(|v| v.as_str())
                        .or_else(|| job.get("status_str").and_then(|v| v.as_str()));
                    if let Some(status) = status {
                        if status == "error" || status == "failed" || status == "aborted" || status == "cancelled" {
                            let error_msg = job.get("status_message")
                                .and_then(|v| v.as_str())
                                .unwrap_or(status);
                            return Some(TaskState::Failed {
                                error: format!("ComfyUI job {} failed: {}", prompt_id, error_msg),
                                comfyui_task_id: Some(prompt_id.to_string()),
                            });
                        } else if status == "success" || status == "completed" {
                            // 成功状态应该已由 poll 函数处理，这里返回 None 让上层继续
                            return None;
                        }
                    }
                    // 如果状态未知，返回 None 表示不确定
                } else {
                    // 历史中没有该任务，视为丢失/失败
                    return Some(TaskState::Failed {
                        error: format!("ComfyUI job {} not found in history", prompt_id),
                        comfyui_task_id: Some(prompt_id.to_string()),
                    });
                }
            }
        }
        _ => {}
    }
    None
}

pub async fn recover_pending_tasks(state: Arc<ProxyState>) {
    let tasks = state.task_manager.get_all().await;
    for (task_id, task_state) in tasks {
        if let TaskState::Processing { comfyui_task_id, backend_name } = task_state {
            let (prompt_id, backend_name) = match (comfyui_task_id, backend_name) {
                (Some(pid), Some(bn)) => (pid, bn),
                _ => continue,
            };

            let state_clone = state.clone();
            tokio::spawn(async move {
                // 获取后端信息
                let backend_state = match state_clone.backends.get_by_name(&backend_name) {
                    Some(b) => b,
                    None => {
                        warn!("恢复任务 {} 失败：后端 {} 不存在", task_id, backend_name);
                        return;
                    }
                };
                let base = format!("{}:{}", backend_state.config.host, backend_state.config.port);
                let client = &state_clone.client;

                let is_video = task_id.starts_with("vid-");
                let timeout_secs = 10;

                let result: Result<Vec<VideoOutput>, ProxyError> = if is_video {
                    poll_history_for_videos(&base, &prompt_id, client, timeout_secs).await
                } else {
                    match poll_history_for_images(&base, &prompt_id, client, timeout_secs).await {
                        Ok(images) => {
                            let videos: Vec<VideoOutput> = images
                                .into_iter()
                                .map(|(filename, bytes)| VideoOutput {
                                    filename,
                                    subfolder: "".to_string(),
                                    bytes,
                                })
                                .collect();
                            Ok(videos)
                        }
                        Err(e) => Err(e),
                    }
                };

                match result {
                    Ok(videos) => {
                        if let Some(output) = videos.first() {
                            if is_video {
                                let url = format!(
                                    "http://{}/view?filename={}&subfolder={}&type=output",
                                    base, output.filename, output.subfolder
                                );
                                state_clone
                                    .task_manager
                                    .update(
                                        &task_id,
                                        TaskState::Completed {
                                            video_url: Some(url),
                                            b64_json: Some(format_file_info(&output.filename, output.bytes.len())),
                                            comfyui_task_id: Some(prompt_id),
                                            execution_time: Some("recovered".to_string()),
                                        },
                                    )
                                    .await;
                                info!("✅ 恢复任务 {} 成功 (视频)", task_id);
                            } else {
                                state_clone
                                    .task_manager
                                    .update(
                                        &task_id,
                                        TaskState::Completed {
                                            video_url: None,
                                            b64_json: Some(format_file_info(&output.filename, output.bytes.len())),
                                            comfyui_task_id: Some(prompt_id),
                                            execution_time: Some("recovered".to_string()),
                                        },
                                    )
                                    .await;
                                info!("✅ 恢复任务 {} 成功 (图片)", task_id);
                            }
                        } else {
                            warn!("恢复任务 {}：结果列表为空", task_id);
                        }
                    }
                    Err(e) => {
                        // 轮询失败，尝试查询历史确认是否永久失败
                        if let Some(final_state) = fetch_job_final_status(&base, &prompt_id, client).await {
                            state_clone.task_manager.update(&task_id, final_state).await;
                            info!("❌ 恢复任务 {} 标记为失败 (ComfyUI 任务已失败/丢失)", task_id);
                        } else {
                            warn!("恢复任务 {} 失败 (可能仍在运行): {}", task_id, e);
                        }
                    }
                }
            });
        }
    }
}