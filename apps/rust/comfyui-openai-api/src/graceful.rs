use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::info;

pub struct GracefulShutdown {
    is_shutting_down: AtomicBool,
    wait_semaphore: Semaphore,
    max_permits: usize,   // 保存最大并发数，用于等待所有许可
    timeout: Duration,
}

impl GracefulShutdown {
    pub fn new(max_concurrency: usize, timeout_secs: u64) -> Self {
        Self {
            is_shutting_down: AtomicBool::new(false),
            wait_semaphore: Semaphore::new(max_concurrency),
            max_permits: max_concurrency,
            timeout: Duration::from_secs(timeout_secs),
        }
    }

    pub fn is_shutting_down(&self) -> bool {
        self.is_shutting_down.load(Ordering::Relaxed)
    }

    pub async fn acquire(&self) -> Option<tokio::sync::SemaphorePermit<'_>> {
        if self.is_shutting_down() {
            return None;
        }
        Some(self.wait_semaphore.acquire().await.unwrap())
    }

    pub async fn shutdown(&self) {
        info!("🛑 Graceful shutdown initiated");
        self.is_shutting_down.store(true, Ordering::Relaxed);
        // 等待所有许可被释放（即所有请求处理完毕）
        if tokio::time::timeout(self.timeout, async {
            let _ = self.wait_semaphore.acquire_many(self.max_permits as u32).await;
        }).await.is_err() {
            info!("⏰ Graceful shutdown timed out, forcing exit");
        }
    }
}