use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use reqwest::Client;
use tokio::sync::RwLock;
use crate::config::{BackendConfig, LbStrategy};

pub struct BackendState {
    pub config: BackendConfig,
    healthy: AtomicBool,
    last_check: RwLock<Instant>,
    fail_count: AtomicU32,
    active_conns: AtomicU32,
}

impl BackendState {
    fn new(config: BackendConfig) -> Self {
        Self {
            config,
            healthy: AtomicBool::new(true),
            last_check: RwLock::new(Instant::now()),
            fail_count: AtomicU32::new(0),
            active_conns: AtomicU32::new(0),
        }
    }

    pub fn is_healthy(&self) -> bool { self.healthy.load(Ordering::Relaxed) }
    pub fn set_healthy(&self, h: bool) { self.healthy.store(h, Ordering::Relaxed); }
    pub fn active_connections(&self) -> u32 { self.active_conns.load(Ordering::Relaxed) }
    pub fn inc_connections(&self) { self.active_conns.fetch_add(1, Ordering::Relaxed); }
    pub fn dec_connections(&self) { self.active_conns.fetch_sub(1, Ordering::Relaxed); }
}

pub struct BackendPool {
    backends: Vec<Arc<BackendState>>,
    strategy: LbStrategy,
    next_index: AtomicUsize,
    client: Client,
    health_interval: Duration,
    fail_threshold: u32,
}

impl BackendPool {
    pub fn new(backends: Vec<BackendConfig>, strategy: LbStrategy, health_interval_secs: u64, fail_threshold: u32) -> Self {
        let pool = Self {
            backends: backends.into_iter().map(|b| Arc::new(BackendState::new(b))).collect(),
            strategy,
            next_index: AtomicUsize::new(0),
            client: Client::new(),
            health_interval: Duration::from_secs(health_interval_secs),
            fail_threshold,
        };
        pool.start_health_checks();
        pool
    }

    fn start_health_checks(&self) {
        let backends = self.backends.clone();
        let client = self.client.clone();
        let interval = self.health_interval;
        let fail_threshold = self.fail_threshold;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                for backend in &backends {
                    let url = format!("http://{}:{}/system_stats", backend.config.host, backend.config.port);
                    match client.get(&url).send().await {
                        Ok(resp) if resp.status().is_success() => {
                            backend.fail_count.store(0, Ordering::Relaxed);
                            backend.set_healthy(true);
                        }
                        _ => {
                            let fails = backend.fail_count.fetch_add(1, Ordering::Relaxed) + 1;
                            if fails >= fail_threshold { backend.set_healthy(false); }
                        }
                    }
                }
            }
        });
    }

    pub fn get_by_name(&self, name: &str) -> Option<&Arc<BackendState>> {
        self.backends.iter().find(|b| b.config.name == name)
    }

    pub fn select_backend(&self) -> Option<&Arc<BackendState>> {
        let healthy: Vec<&Arc<BackendState>> = self.backends.iter().filter(|b| b.is_healthy()).collect();
        if healthy.is_empty() { return None; }
        match self.strategy {
            LbStrategy::RoundRobin => {
                let idx = self.next_index.fetch_add(1, Ordering::Relaxed);
                let chosen = healthy[idx % healthy.len()];
                Some(chosen)
            }
            LbStrategy::LeastConnections => {
                healthy.into_iter().min_by_key(|b| b.active_connections())
            }
            LbStrategy::Random => {
                let idx = rand::random::<usize>() % healthy.len();
                Some(healthy[idx])
            }
        }
    }

    /// 返回所有后端名称及其健康状态
    pub fn list_backends(&self) -> Vec<(String, bool)> {
        self.backends.iter().map(|b| (b.config.name.clone(), b.is_healthy())).collect()
    }
}