use indexmap::IndexMap;
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use tokio::sync::RwLock;
use std::path::PathBuf;
use tokio::fs;
use tracing::info;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum TaskState {
    #[serde(rename = "processing")]
    Processing {
        comfyui_task_id: Option<String>,
    },
    #[serde(rename = "completed")]
    Completed {
        video_url: Option<String>,
        b64_json: Option<String>,
        comfyui_task_id: Option<String>,
        #[serde(default)]
        execution_time: Option<String>,
    },
    #[serde(rename = "failed")]
    Failed {
        error: String,
        comfyui_task_id: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskRecord {
    pub created_at: u128,
    pub state: TaskState,
}

pub struct TaskManager {
    tasks: RwLock<IndexMap<String, TaskRecord>>,
    persist_path: Option<PathBuf>,
}

impl TaskManager {
    pub fn new() -> Self {
        TaskManager {
            tasks: RwLock::new(IndexMap::new()),
            persist_path: None,
        }
    }

    pub fn new_with_persist(path: impl Into<PathBuf>) -> Self {
        TaskManager {
            tasks: RwLock::new(IndexMap::new()),
            persist_path: Some(path.into()),
        }
    }

    pub async fn load_persisted_async(&self) {
        if let Some(ref path) = self.persist_path {
            if let Ok(content) = fs::read_to_string(path).await {
                let mut tasks = self.tasks.write().await;
                if let Ok(map) = serde_json::from_str::<IndexMap<String, TaskRecord>>(&content) {
                    *tasks = map;
                    info!("📂 Loaded {} persisted tasks", tasks.len());
                    return;
                }
                if let Ok(old_map) = serde_json::from_str::<HashMap<String, TaskState>>(&content) {
                    for (k, v) in old_map {
                        tasks.insert(k, TaskRecord {
                            created_at: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis(),
                            state: v,
                        });
                    }
                    info!("📂 Loaded {} persisted tasks from legacy format", tasks.len());
                }
            }
        }
    }

    async fn save_persisted(&self) {
        if let Some(ref path) = self.persist_path {
            let tasks_snapshot = self.tasks.read().await.clone();
            let mut entries: Vec<_> = tasks_snapshot.into_iter().collect();
            entries.sort_unstable_by_key(|(_, record)| std::cmp::Reverse(record.created_at));
            let ordered: IndexMap<_, _> = entries.into_iter().collect();
            if let Ok(json) = serde_json::to_string_pretty(&ordered) {
                let _ = fs::write(path, json).await;
            }
        }
    }

    fn prune_old_tasks(tasks: &mut IndexMap<String, TaskRecord>) {
        const MAX_TASKS: usize = 100;
        if tasks.len() <= MAX_TASKS {
            return;
        }
        let mut entries: Vec<_> = tasks.iter()
            .map(|(id, record)| (record.created_at, id.clone()))
            .collect();
        entries.sort_unstable_by_key(|(created_at, _)| *created_at);
        for (_, id) in entries.into_iter().take(tasks.len() - MAX_TASKS) {
            tasks.shift_remove(&id);
        }
    }

    fn preserve_comfyui_id(existing: &TaskState, incoming: TaskState) -> TaskState {
        match (existing, incoming) {
            (TaskState::Processing { comfyui_task_id: Some(id) }, TaskState::Completed { video_url, b64_json, comfyui_task_id: None, execution_time }) => {
                TaskState::Completed { video_url, b64_json, comfyui_task_id: Some(id.clone()), execution_time }
            }
            (TaskState::Processing { comfyui_task_id: Some(id) }, TaskState::Failed { error, comfyui_task_id: None }) => {
                TaskState::Failed { error, comfyui_task_id: Some(id.clone()) }
            }
            (_, other) => other,
        }
    }

    pub async fn insert(&self, task_id: String, state: TaskState) {
        let mut tasks = self.tasks.write().await;
        let record = TaskRecord {
            created_at: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis(),
            state,
        };
        tasks.insert(task_id, record);
        Self::prune_old_tasks(&mut tasks);
        drop(tasks);
        self.save_persisted().await;
    }

    pub async fn update(&self, task_id: &str, state: TaskState) {
        let mut tasks = self.tasks.write().await;
        let created_at = tasks.get(task_id).map(|r| r.created_at).unwrap_or_else(|| {
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()
        });
        let final_state = if let Some(existing) = tasks.get(task_id) {
            Self::preserve_comfyui_id(&existing.state, state)
        } else {
            state
        };
        let record = TaskRecord { created_at, state: final_state };
        tasks.insert(task_id.to_string(), record);
        Self::prune_old_tasks(&mut tasks);
        drop(tasks);
        self.save_persisted().await;
    }

    pub async fn get(&self, task_id: &str) -> Option<TaskState> {
        self.tasks.read().await.get(task_id).map(|record| record.state.clone())
    }

    pub async fn get_all(&self) -> Vec<(String, TaskState)> {
        let mut tasks: Vec<_> = self.tasks.read().await.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        tasks.sort_unstable_by_key(|(_, record)| std::cmp::Reverse(record.created_at));
        tasks.into_iter().map(|(id, record)| (id, record.state)).collect()
    }

    /// 删除指定任务
    pub async fn remove(&self, task_id: &str) {
        self.tasks.write().await.shift_remove(task_id);
        self.save_persisted().await;
    }
}