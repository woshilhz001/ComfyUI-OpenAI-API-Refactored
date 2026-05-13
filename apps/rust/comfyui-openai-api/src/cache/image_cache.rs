use anyhow::{anyhow, Result};
use base64::{Engine as _, engine::general_purpose};
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::LazyLock;
use std::sync::OnceLock;
use tokio::sync::RwLock;
use reqwest::Client;
use tracing::{info, warn};

static IMAGE_CACHE: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
static CACHE_DIR: OnceLock<String> = OnceLock::new();
static PLACEHOLDER_FILENAME: OnceLock<String> = OnceLock::new();
static PLACEHOLDER_3X3_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAMAAAADCAYAAABWKLW/AAAAAXNSR0IArs4c6QAAAARnQU1BAACxjwv8YQUAAAAJcEhZcwAADsMAAA7DAcdvqGQAAAAZdEVYdFNvZnR3YXJlAHBhaW50Lm5ldCA0LjAuMTM0A1t6AAAAFUlEQVQYV2P8//8/AyAgIyOjGgAOmAUBAGMVB0IAAAAASUVORK5CYII=";

const PLACEHOLDER_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVQI12NgYAAAAAMAASDVlMcAAAAASUVORK5CYII=";

pub fn init_input_dir(dir: String) {
    info!("📁 Proxy cache directory configured: {}", dir);
    let _ = CACHE_DIR.set(dir);
}

pub fn get_cache_dir() -> String {
    CACHE_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| {
            warn!("CACHE_DIR not set, using default './cache'");
            "./cache".to_string()
        })
}

pub fn start_cache_cleaner() {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            let mut cache = IMAGE_CACHE.write().await;
            if cache.len() > 1000 {
                let keys_to_remove: Vec<String> = cache.keys().take(cache.len() / 2).cloned().collect();
                for key in keys_to_remove {
                    cache.remove(&key);
                }
                info!("🧹 Cleaned image cache, current size: {}", cache.len());
            }
        }
    });
}

async fn store_local_cache(bytes: &[u8], filename: &str) -> Result<String> {
    let cache_base = get_cache_dir();
    let cache_dir = Path::new(&cache_base);
    if !cache_dir.exists() {
        fs::create_dir_all(cache_dir)?;
        info!("📁 Created proxy cache directory: {}", cache_base);
    }
    let filepath = cache_dir.join(filename);
    if !filepath.exists() {
        fs::write(&filepath, bytes)?;
        info!("📄 Cached image locally: {} ({} bytes)", filename, bytes.len());
    }
    Ok(filename.to_string())
}

async fn upload_to_backend(client: &Client, backend: &crate::config::BackendConfig, bytes: &[u8], original_filename: &str) -> Result<String> {
    let upload_url = format!("http://{}:{}/upload/image", backend.host, backend.port);
    info!("📤 Uploading image to backend '{}': {}", backend.name, upload_url);
    let part = reqwest::multipart::Part::bytes(bytes.to_vec())
        .file_name(original_filename.to_string())
        .mime_str("image/png")?;
    let form = reqwest::multipart::Form::new().part("image", part);
    let resp = match client
        .post(&upload_url)
        .multipart(form)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!("Upload request failed: {}", e);
            return Err(anyhow!("Upload request to '{}' failed: {}", upload_url, e));
        }
    };
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        warn!("Upload response status: {}, body: {}", status, text);
        return Err(anyhow!("Upload to backend '{}' failed: HTTP {} - {}", backend.name, status, text));
    }
    let json: serde_json::Value = resp.json().await?;
    let name = json["name"].as_str().ok_or_else(|| anyhow!("Upload response missing 'name'"))?;
    info!("✅ Image uploaded to backend '{}': {}", backend.name, name);
    Ok(name.to_string())
}

pub async fn cache_image(client: &Client, backend: &crate::config::BackendConfig, image_input: &str) -> Result<String> {
    let bytes = if image_input.starts_with("data:image") || !image_input.starts_with("http") {
        let base64_clean = image_input.split(',').last().unwrap_or(image_input);
        general_purpose::STANDARD
            .decode(base64_clean)
            .map_err(|e| anyhow!("Base64 decode failed: {}", e))?
    } else {
        let resp = client.get(image_input).send().await?;
        resp.bytes().await?.to_vec()
    };

    let hash = {
        let mut hasher = std::hash::DefaultHasher::new();
        image_input.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    };
    let ext = if image_input.starts_with("data:image/png") {
        "png"
    } else if image_input.starts_with("data:image/jpeg") {
        "jpg"
    } else {
        "png"
    };
    let local_filename = format!("{}.{}", hash, ext);
    let cache_key = format!("{}:{}", backend.name, local_filename);

    {
        let cache = IMAGE_CACHE.read().await;
        if let Some(cached) = cache.get(&cache_key) {
            return Ok(cached.clone());
        }
    }

    let _ = store_local_cache(&bytes, &local_filename).await?;
    let remote_name = upload_to_backend(client, backend, &bytes, &local_filename).await?;
    IMAGE_CACHE
        .write()
        .await
        .insert(cache_key, remote_name.clone());
    Ok(remote_name)
}

pub async fn get_3x3_placeholder_filename(client: &Client, backend: &crate::config::BackendConfig) -> Result<String> {
    let placeholder_key = format!("placeholder_3x3_{}", backend.name);
    {
        let cache = IMAGE_CACHE.read().await;
        if let Some(name) = cache.get(&placeholder_key) {
            return Ok(name.clone());
        }
    }
    let bytes = general_purpose::STANDARD
        .decode(PLACEHOLDER_BASE64)
        .map_err(|e| anyhow!("Failed to decode 3x3 placeholder base64: {}", e))?;
    let local_filename = "placeholder_3x3.png".to_string();
    let _ = store_local_cache(&bytes, &local_filename).await?;
    let remote_name = upload_to_backend(client, backend, &bytes, &local_filename).await?;
    IMAGE_CACHE
        .write()
        .await
        .insert(placeholder_key, remote_name.clone());
    Ok(remote_name)
}

pub async fn get_placeholder_filename(client: &Client, backend: &crate::config::BackendConfig) -> Result<String> {
    let placeholder_key = format!("placeholder_{}", backend.name);
    {
        let cache = IMAGE_CACHE.read().await;
        if let Some(name) = cache.get(&placeholder_key) {
            return Ok(name.clone());
        }
    }
    let bytes = general_purpose::STANDARD
        .decode(PLACEHOLDER_BASE64)
        .map_err(|e| anyhow!("Failed to decode placeholder base64: {}", e))?;
    let local_filename = "placeholder.png".to_string();
    let _ = store_local_cache(&bytes, &local_filename).await?;
    let remote_name = upload_to_backend(client, backend, &bytes, &local_filename).await?;
    IMAGE_CACHE
        .write()
        .await
        .insert(placeholder_key, remote_name.clone());
    Ok(remote_name)
}

pub async fn init_placeholder(client: &Client) -> Result<()> {
    let dummy_backend = crate::config::BackendConfig {
        name: "dummy".to_string(),
        host: "127.0.0.1".to_string(),
        port: 8000,
        default: false,
    };
    let _ = get_placeholder_filename(client, &dummy_backend).await?;
    let _ = get_3x3_placeholder_filename(client, &dummy_backend).await?;
    Ok(())
}