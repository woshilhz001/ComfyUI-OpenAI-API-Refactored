use axum::http::HeaderMap;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// 幂等键检查结果
pub enum IdempotencyResult {
    /// 已存在缓存的响应
    Hit(Vec<u8>),
    /// 新请求，应继续处理
    Miss,
}

/// 简单的内存幂等键存储（可替换为 Redis 等）
pub struct IdempotencyStore {
    cache: Mutex<HashMap<String, (Vec<u8>, Instant)>>,
    ttl: Duration,
}

impl IdempotencyStore {
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    /// 检查幂等键，若存在则返回缓存响应，否则表示需要处理
    pub fn check_or_ignore(&self, key: &str) -> Option<Vec<u8>> {
        let mut cache = self.cache.lock().unwrap();
        if let Some((data, timestamp)) = cache.get(key) {
            if timestamp.elapsed() < self.ttl {
                return Some(data.clone());
            } else {
                cache.remove(key);
            }
        }
        None
    }

    /// 存储幂等键及其响应
    pub fn store(&self, key: &str, data: Vec<u8>) {
        self.cache
            .lock()
            .unwrap()
            .insert(key.to_string(), (data, Instant::now()));
    }
}

/// 从请求头中提取幂等键
pub fn extract_idempotency_key(headers: &HeaderMap) -> Option<String> {
    headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}