use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use rand::Rng;

pub struct SeedTracker {
    seeds: RwLock<HashMap<String, i64>>,
}

impl SeedTracker {
    pub fn new() -> Self {
        Self { seeds: RwLock::new(HashMap::new()) }
    }

    pub async fn get_seed(&self, role: &str) -> i64 {
        let guard = self.seeds.read().await;
        guard.get(role).copied().unwrap_or_else(|| {
            rand::thread_rng().gen_range(0..i64::MAX)
        })
    }

    pub async fn update_seed(&self, role: &str, seed: i64) {
        self.seeds.write().await.insert(role.to_string(), seed);
    }

    pub fn random_seed() -> i64 {
        rand::thread_rng().gen_range(0..i64::MAX)
    }
}