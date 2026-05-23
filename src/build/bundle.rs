use std::collections::BTreeMap;

use serde::Serialize;

pub const SLOT_PLACEHOLDER: &str = "###SLOT###";

#[derive(Debug, Clone, Serialize)]
pub struct LaunchBundle {
    pub version: u32,
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_body: Option<String>,
    pub routing: Routing,
    pub execution_policy: ExecutionPolicy,
    pub prompt_surface: PromptSurface,
    pub scaffold_slots: ScaffoldSlots,
    pub tools: ToolsSpec,
    pub skills_metadata: SkillsMetadata,
    pub provenance: BTreeMap<String, String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Routing {
    pub model: String,
    pub model_token: String,
    pub harness: String,
    pub selection_kind: String,
    pub match_evidence: String,
    pub harness_model: String,
    pub harness_model_source: String,
    pub harness_model_confidence: String,
    /// Diagnostic only: probe/catalog slug candidates for the selected harness.
    /// Consumers should run `harness_model` verbatim and ignore this unless debugging.
    pub candidate_slugs: Vec<String>,
    pub route_trace: crate::routing::report::RouteDecisionReport,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodexRule {
    pub name: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExecutionPolicy {
    pub effort: Option<String>,
    pub approval: Option<String>,
    pub sandbox: Option<String>,
    pub autocompact: Option<u32>,
    pub autocompact_pct: Option<u8>,
    pub timeout: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_config: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_rules: Option<Vec<CodexRule>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PromptSurface {
    pub system_instruction: String,
    pub supplemental_documents: Vec<SupplementalDoc>,
    pub inventory_prompt: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScaffoldSlots {
    pub completion_contract: String,
    pub context_prompt: String,
    pub user_prompt_file: String,
    pub context_files: String,
    pub prior_session_context: String,
    pub spawn_metadata: String,
}

impl ScaffoldSlots {
    pub fn placeholders() -> Self {
        Self {
            completion_contract: SLOT_PLACEHOLDER.to_string(),
            context_prompt: SLOT_PLACEHOLDER.to_string(),
            user_prompt_file: SLOT_PLACEHOLDER.to_string(),
            context_files: SLOT_PLACEHOLDER.to_string(),
            prior_session_context: SLOT_PLACEHOLDER.to_string(),
            spawn_metadata: SLOT_PLACEHOLDER.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SupplementalDoc {
    pub kind: String,
    pub name: String,
    pub content: String,
    pub skill_type: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolsSpec {
    pub allowed: Vec<String>,
    pub disallowed: Vec<String>,
    pub mcp: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillsMetadata {
    pub loaded: Vec<String>,
    pub missing: Vec<String>,
}
