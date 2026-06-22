use serde_json::Value;
use std::collections::HashMap;
use super::registry::WorkflowTemplate;

#[derive(Clone)]
pub struct PreparedWorkflow {
    pub raw: Value,
    pub inject_points: HashMap<InjectRole, String>,
    pub load_image_nodes: Vec<String>,
    pub has_prompt_relay: bool,
    pub ltxv_add_guide_multi_nodes: Vec<(String, usize)>,
    pub duration_node: Option<String>,
    pub fps_node: Option<String>,
}

#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub enum InjectRole {
    PositivePrompt,
    NegativePrompt,
    PositivePromptString,
    NegativePromptString,
    Width,
    Height,
    NoiseSeed,
    Seed,
    BatchSize,
    Duration,
    Fps,
}

impl PreparedWorkflow {
    pub fn from_template(tpl: &WorkflowTemplate) -> Self {
        let mut inject = HashMap::new();
        if let Some(id) = &tpl.positive_prompt_node { inject.insert(InjectRole::PositivePrompt, id.clone()); }
        if let Some(id) = &tpl.negative_prompt_node { inject.insert(InjectRole::NegativePrompt, id.clone()); }
        if let Some(id) = &tpl.positive_string_node { inject.insert(InjectRole::PositivePromptString, id.clone()); }
        if let Some(id) = &tpl.negative_string_node { inject.insert(InjectRole::NegativePromptString, id.clone()); }
        if let Some(id) = &tpl.width_node { inject.insert(InjectRole::Width, id.clone()); }
        if let Some(id) = &tpl.height_node { inject.insert(InjectRole::Height, id.clone()); }
        // NoiseSeed 和 Seed 在运行时遍历，不在此处固定

        Self {
            raw: tpl.raw.clone(),
            inject_points: inject,
            load_image_nodes: tpl.load_image_nodes.clone(),
            has_prompt_relay: tpl.has_prompt_relay,
            ltxv_add_guide_multi_nodes: tpl.ltxv_add_guide_multi_nodes.clone(),
            duration_node: tpl.duration_node.clone(),
            fps_node: tpl.fps_node.clone(),
        }
    }
}