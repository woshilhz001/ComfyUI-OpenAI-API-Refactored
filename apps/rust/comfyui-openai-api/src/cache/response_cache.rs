// src/cache/response_cache.rs
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct ResponseCache {
    cache: Mutex<LruCache<String, (Vec<u8>, Instant)>>,
    ttl: Duration,
}

impl ResponseCache {
    pub fn new(max_entries: usize, ttl_secs: u64) -> Self {
        Self {
            cache: Mutex::new(LruCache::new(
                NonZeroUsize::new(max_entries).unwrap_or(NonZeroUsize::new(1).unwrap()),
            )),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        let mut cache = self.cache.lock().unwrap();
        if let Some((data, timestamp)) = cache.get(key) {
            if timestamp.elapsed() < self.ttl {
                return Some(data.clone());
            } else {
                cache.pop(key);
            }
        }
        None
    }

    pub fn insert(&self, key: String, data: Vec<u8>) {
        self.cache.lock().unwrap().put(key, (data, Instant::now()));
    }
}