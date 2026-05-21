use serde::{Deserialize, Serialize, Deserializer, de};
use std::env;
use std::fs;
use tracing::info;

fn deserialize_optional_u32<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where D: Deserializer<'de> {
    let value = serde_yaml::Value::deserialize(deserializer)?;
    match value {
        serde_yaml::Value::Null => Ok(None),
        serde_yaml::Value::String(ref s) if s.is_empty() => Ok(None),
        serde_yaml::Value::String(s) => s.parse::<u32>().map(Some).map_err(de::Error::custom),
        serde_yaml::Value::Number(n) => n.as_u64().map(|v| Some(v as u32)).ok_or_else(|| de::Error::custom("invalid number")),
        _ => Err(de::Error::custom("expected string or number")),
    }
}

fn deserialize_optional_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where D: Deserializer<'de> {
    let value = serde_yaml::Value::deserialize(deserializer)?;
    match value {
        serde_yaml::Value::Null => Ok(None),
        serde_yaml::Value::String(ref s) if s.is_empty() => Ok(None),
        serde_yaml::Value::String(s) => s.parse::<f64>().map(Some).map_err(de::Error::custom),
        serde_yaml::Value::Number(n) => n.as_f64().map(Some).ok_or_else(|| de::Error::custom("invalid number")),
        _ => Err(de::Error::custom("expected string or number")),
    }
}

fn default_rate_limit() -> Option<RateLimitConfig> {
    None
}

fn default_response_cache() -> Option<ResponseCacheConfig> {
    None
}

fn default_lb_strategy() -> LbStrategy {
    LbStrategy::RoundRobin
}

fn default_true() -> bool { true }
fn default_false() -> bool { false }

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct BackendConfig {
    pub name: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub default: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ComfyUiProxyConfig {
    pub client_id: String,
    pub workflows_folder: String,
    pub use_ws: bool,
    #[serde(default = "default_input_dir")]
    pub input_dir: String,
}

fn default_input_dir() -> String { "./input".to_string() }

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RateLimitConfig {
    pub max_tokens: u64,
    pub refill_rate: f64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ResponseCacheConfig {
    pub ttl_secs: u64,
    pub max_entries: usize,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "PascalCase")]
pub enum LbStrategy {
    RoundRobin,
    LeastConnections,
    Random,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RoutingConfig {
    pub timeout_seconds: u64,
    pub max_payload_size_mb: u16,
    #[serde(default, rename = "Image_Width", deserialize_with = "deserialize_optional_u32")]
    pub image_width: Option<u32>,
    #[serde(default, rename = "Image_Height", deserialize_with = "deserialize_optional_u32")]
    pub image_height: Option<u32>,
    #[serde(default, rename = "video_Width", deserialize_with = "deserialize_optional_u32")]
    pub video_width: Option<u32>,
    #[serde(default, rename = "video_Height", deserialize_with = "deserialize_optional_u32")]
    pub video_height: Option<u32>,
    #[serde(default, rename = "fps", deserialize_with = "deserialize_optional_f64")]
    pub fps: Option<f64>,
    #[serde(default = "default_true")]
    pub free_model_before_video: bool,
    // 新增：图片生成前是否清理模型，默认 false
    #[serde(default = "default_false")]
    pub free_model_before_image: bool,

    // 新增配置项
    #[serde(default = "default_lb_strategy")]
    pub lb_strategy: LbStrategy,
    #[serde(default = "default_rate_limit")]
    pub rate_limit: Option<RateLimitConfig>,
    #[serde(default = "default_response_cache")]
    pub response_cache: Option<ResponseCacheConfig>,
    #[serde(default = "default_true")]
    pub enable_response_cache: bool,
    #[serde(default = "default_false")]
    pub enable_idempotency: bool,
    #[serde(default = "default_graceful_shutdown_secs")]
    pub graceful_shutdown_timeout_secs: u64,
    #[serde(default = "default_health_interval")]
    pub health_check_interval_secs: u64,
    #[serde(default = "default_health_fail_threshold")]
    pub health_check_fail_threshold: u32,
}

fn default_graceful_shutdown_secs() -> u64 { 30 }
fn default_health_interval() -> u64 { 15 }
fn default_health_fail_threshold() -> u32 { 3 }

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    pub log_level: String,
    pub server: ServerConfig,
    pub comfyui_backends: Vec<BackendConfig>,
    pub comfyui_backend: ComfyUiProxyConfig,
    pub routing: RoutingConfig,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl Config {
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let config_path = env::var("CONFIG_PATH").unwrap_or_else(|_| "./config/config.yaml".to_string());
        info!("Loading config from: {}", config_path);
        let config_str = fs::read_to_string(&config_path)?;
        let mut config: Config = serde_yaml::from_str(&config_str)?;
        if config.comfyui_backends.is_empty() {
            return Err("No ComfyUI backends defined".into());
        }
        if !config.comfyui_backends.iter().any(|b| b.default) {
            config.comfyui_backends[0].default = true;
        }
        Ok(config)
    }
}