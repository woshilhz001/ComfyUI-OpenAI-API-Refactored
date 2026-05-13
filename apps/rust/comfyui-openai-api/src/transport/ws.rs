// transport/ws.rs
use futures::stream::StreamExt;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Notify};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

const COMPLETED_CAPACITY: usize = 100;
const INITIAL_RECONNECT_DELAY: u64 = 2;
const MAX_RECONNECT_DELAY: u64 = 60;
const RECONNECT_MULTIPLIER: f64 = 1.5;

pub struct WsManager {
    completed: Mutex<Vec<String>>,
    backend_url: String,
    backend_port: String,
    client_id: String,
    notify: Notify,
}

impl WsManager {
    pub async fn connect(backend_url: String, backend_port: String, client_id: String) -> Arc<Self> {
        let mgr = Arc::new(Self {
            completed: Mutex::new(Vec::new()),
            backend_url: backend_url.clone(),
            backend_port: backend_port.clone(),
            client_id,
            notify: Notify::new(),
        });
        let mgr_clone = Arc::clone(&mgr);
        tokio::spawn(async move { mgr_clone.run_loop().await });
        mgr
    }

    async fn run_loop(&self) {
        let mut delay = Duration::from_secs(INITIAL_RECONNECT_DELAY);
        loop {
            let url = format!(
                "ws://{}:{}/ws?clientId={}",
                self.backend_url, self.backend_port, self.client_id
            );
            match connect_async(&url).await {
                Ok((ws_stream, _)) => {
                    info!("WebSocket connected to {}", url);
                    let (_, read) = ws_stream.split();
                    delay = Duration::from_secs(INITIAL_RECONNECT_DELAY);
                    self.listen(read).await;
                }
                Err(e) => {
                    error!("WebSocket connection failed: {}", e);
                }
            }
            warn!("Reconnecting WebSocket in {} sec…", delay.as_secs());
            tokio::time::sleep(delay).await;
            delay = (delay.mul_f64(RECONNECT_MULTIPLIER))
                .min(Duration::from_secs(MAX_RECONNECT_DELAY));
        }
    }

    async fn listen(
        &self,
        mut read: futures::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
    ) {
        loop {
            match tokio::time::timeout(Duration::from_secs(60), read.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    if let Ok(json) = serde_json::from_str::<Value>(&text) {
                        if json["type"] == "executing" && json["data"]["node"].is_null() {
                            let pid = json["data"]["prompt_id"].as_str().unwrap_or("");
                            debug!("Job completed via WS: {pid}");
                            let mut list = self.completed.lock().await;
                            if list.len() >= COMPLETED_CAPACITY {
                                list.remove(0);
                            }
                            list.push(pid.to_string());
                            self.notify.notify_waiters();
                        }
                    }
                }
                Ok(Some(Ok(_))) => {}          // 忽略 Binary, Ping, Pong 等消息
                Ok(Some(Err(e))) => {
                    error!("WebSocket error: {e}");
                    return;
                }
                Ok(None) => return,
                Err(_) => {}                   // 读取超时，继续
            }
        }
    }

    pub async fn wait_for_job(&self, prompt_id: &str) -> anyhow::Result<()> {
        loop {
            if self.completed.lock().await.iter().any(|id| id == prompt_id) {
                return Ok(());
            }
            tokio::select! {
                _ = self.notify.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(600)) => {
                    return Err(anyhow::anyhow!("Timeout waiting for job completion"));
                }
            }
        }
    }
}