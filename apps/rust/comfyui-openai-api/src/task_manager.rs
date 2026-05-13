use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use tokio::sync::RwLock;
use std::path::PathBuf;
use tokio::fs;
use tracing::info;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum TaskState {
    #[serde(rename = "processing")]
    Processing,
    #[serde(rename = "completed")]
    Completed {
        video_url: Option<String>,
        b64_json: Option<String>,
    },
    #[serde(rename = "failed")]
    Failed {
        error: String,
    },
}

pub struct TaskManager {
    tasks: RwLock<HashMap<String, TaskState>>,
    persist_path: Option<PathBuf>,
}

impl TaskManager {
    pub fn new() -> Self {
        TaskManager {
            tasks: RwLock::new(HashMap::new()),
            persist_path: None,
        }
    }

    pub fn new_with_persist(path: impl Into<PathBuf>) -> Self {
        TaskManager {
            tasks: RwLock::new(HashMap::new()),
            persist_path: Some(path.into()),
        }
    }

    pub async fn load_persisted_async(&self) {
        if let Some(ref path) = self.persist_path {
            if let Ok(content) = fs::read_to_string(path).await {
                if let Ok(map) = serde_json::from_str::<HashMap<String, TaskState>>(&content) {
                    let mut tasks = self.tasks.write().await;
                    for (k, v) in map {
                        tasks.insert(k, v);
                    }
                    info!("📂 Loaded {} persisted tasks", tasks.len());
                }
            }
        }
    }

    async fn save_persisted(&self) {
        if let Some(ref path) = self.persist_path {
            let tasks_snapshot = self.tasks.read().await.clone();
            if let Ok(json) = serde_json::to_string_pretty(&tasks_snapshot) {
                let _ = fs::write(path, json).await;
            }
        }
    }

    pub async fn insert(&self, task_id: String, state: TaskState) {
        self.tasks.write().await.insert(task_id, state);
        self.save_persisted().await;
    }

    pub async fn update(&self, task_id: &str, state: TaskState) {
        self.tasks.write().await.insert(task_id.to_string(), state);
        self.save_persisted().await;
    }

    pub async fn get(&self, task_id: &str) -> Option<TaskState> {
        self.tasks.read().await.get(task_id).cloned()
    }

    pub async fn get_all(&self) -> Vec<(String, TaskState)> {
        self.tasks.read().await.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// 删除指定任务
    pub async fn remove(&self, task_id: &str) {
        self.tasks.write().await.remove(task_id);
        self.save_persisted().await;
    }
}