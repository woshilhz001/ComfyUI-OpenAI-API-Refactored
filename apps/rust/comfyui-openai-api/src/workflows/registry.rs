use anyhow::{anyhow, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tracing::info;

#[derive(Clone)]
pub struct WorkflowTemplate {
    pub raw: Value,
    pub positive_prompt_node: Option<String>,
    pub negative_prompt_node: Option<String>,
    pub positive_string_node: Option<String>,
    pub negative_string_node: Option<String>,
    pub load_image_nodes: Vec<String>,
    pub width_node: Option<String>,
    pub height_node: Option<String>,
    pub has_prompt_relay: bool,
    pub lora_nodes: Vec<String>,
    pub reference_latent_nodes: Vec<String>,
    pub empty_ltxv_latent_node: Option<String>,
    pub ltxv_add_guide_multi_nodes: Vec<(String, usize)>,
    pub duration_node: Option<String>,
    pub fps_node: Option<String>,
}

impl WorkflowTemplate {
    pub fn from_json(name: &str, json: Value) -> Self {
        // 保持与原 parse 逻辑一致
        let mut positive_prompt_node = None;
        let mut negative_prompt_node = None;
        let mut positive_string_node = None;
        let mut negative_string_node = None;
        let mut width_node = None;
        let mut height_node = None;
        let mut has_prompt_relay = false;
        let mut lora_nodes = Vec::new();
        let mut reference_latent_nodes = Vec::new();
        let mut empty_ltxv_latent_node = None;
        let mut ltxv_add_guide_multi_nodes = Vec::new();
        let mut duration_node = None;
        let mut fps_node = None;
        let mut temp_load_images: Vec<(String, String)> = Vec::new();

        if let Some(obj) = json.as_object() {
            for (node_id, node) in obj {
                let class_type = node["class_type"].as_str().unwrap_or("");
                let title = node["_meta"]["title"].as_str().unwrap_or("").to_string();
                match class_type {
                    "CLIPTextEncode" => {
                        if title.contains("Positive") { positive_prompt_node = Some(node_id.clone()); }
                        else if title.contains("Negative") { negative_prompt_node = Some(node_id.clone()); }
                    }
                    "LoadImage" => { temp_load_images.push((node_id.clone(), title)); }
                    "PrimitiveInt" | "INTConstant" => {
                        if title == "Width" || title == "WIDTH" { width_node = Some(node_id.clone()); }
                        else if title == "Height" || title == "HEIGHT" { height_node = Some(node_id.clone()); }
                    }
                    "PrimitiveFloat" => {
                        if title == "Duration" { duration_node = Some(node_id.clone()); }
                        else if title == "FPS" { fps_node = Some(node_id.clone()); }
                    }
                    "PrimitiveStringMultiline" => {
                        if title.contains("Positive") { positive_string_node = Some(node_id.clone()); }
                        else if title.contains("Negative") { negative_string_node = Some(node_id.clone()); }
                    }
                    "PromptRelayEncode" => { has_prompt_relay = true; }
                    "LoraLoaderModelOnly" | "LoraLoader" => { lora_nodes.push(node_id.clone()); }
                    "ReferenceLatent" => { reference_latent_nodes.push(node_id.clone()); }
                    "EmptyLTXVLatentVideo" => { empty_ltxv_latent_node = Some(node_id.clone()); }
                    "LTXVAddGuideMulti" => {
                        let num_guides = node["inputs"]["num_guides"]
                            .as_str().and_then(|s| s.parse().ok())
                            .or_else(|| node["inputs"]["num_guides"].as_u64().map(|v| v as usize))
                            .unwrap_or(0);
                        if num_guides > 0 { ltxv_add_guide_multi_nodes.push((node_id.clone(), num_guides)); }
                    }
                    _ => {}
                }
            }
        }

        temp_load_images.sort_by(|(id_a, title_a), (id_b, title_b)| {
            let idx_a = WorkflowTemplate::extract_reference_index(title_a);
            let idx_b = WorkflowTemplate::extract_reference_index(title_b);
            match (idx_a, idx_b) {
                (Some(a), Some(b)) => a.cmp(&b),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => id_a.parse::<u64>().unwrap_or(0).cmp(&id_b.parse::<u64>().unwrap_or(0)),
            }
        });
        let load_image_nodes: Vec<String> = temp_load_images.into_iter().map(|(id, _)| id).collect();

        info!("Parsed workflow '{}': {} LoadImage nodes", name, load_image_nodes.len());
        info!("Parsed workflow '{}': {} LoadImage nodes", name, load_image_nodes.len());
        if !load_image_nodes.is_empty() {
            let details: Vec<String> = load_image_nodes.iter()
                .map(|id| format!("{} (title: {:?})", id, json.as_object().and_then(|obj| obj.get(id).and_then(|n| n["_meta"]["title"].as_str()))))
                .collect();
            info!("  LoadImage nodes: {}", details.join(", "));
        }
        WorkflowTemplate {
            raw: json,
            positive_prompt_node,
            negative_prompt_node,
            positive_string_node,
            negative_string_node,
            load_image_nodes,
            width_node,
            height_node,
            has_prompt_relay,
            lora_nodes,
            reference_latent_nodes,
            empty_ltxv_latent_node,
            ltxv_add_guide_multi_nodes,
            duration_node,
            fps_node,
        }
    }

    fn extract_reference_index(title: &str) -> Option<u32> {
        // 精确匹配 "Reference Image"（没有数字）视为第 1 张
        if title == "Reference Image" {
            return Some(1);
        }
        let prefix = "Reference Image ";
        title.find(prefix).map(|pos| {
            title[pos + prefix.len()..]
                .split_whitespace()
                .next()?
                .parse::<u32>()
                .ok()
        }).flatten()
    }
}

pub struct WorkflowRegistry {
    templates: HashMap<String, Arc<WorkflowTemplate>>,
}

impl WorkflowRegistry {
    pub fn new() -> Self { Self { templates: HashMap::new() } }

    pub fn load_from_folder(&mut self, folder_path: &str) -> Result<()> {
        let path = Path::new(folder_path);
        if !path.is_dir() { return Err(anyhow!("Invalid folder: {}", folder_path)); }
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let file_path = entry.path();
            if file_path.extension().and_then(|s| s.to_str()) == Some("json") {
                let name = file_path.file_stem().unwrap().to_str().unwrap().to_string();
                let content = fs::read_to_string(&file_path)?;
                let json: Value = serde_json::from_str(&content)?;
                let tpl = WorkflowTemplate::from_json(&name, json);
                self.templates.insert(name, Arc::new(tpl));
            }
        }
        info!("Loaded {} workflow templates", self.templates.len());
        Ok(())
    }

    pub fn get(&self, model: &str) -> Option<Arc<WorkflowTemplate>> {
        self.templates.get(model).cloned()
    }

    /// 返回所有已注册的模型名称
    pub fn list_models(&self) -> Vec<String> {
        self.templates.keys().cloned().collect()
    }
}