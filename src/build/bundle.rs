use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;

use crate::compiler::tool_policy::ToolRule;
use crate::config::{NetworkHost, NetworkPolicy};

pub const SLOT_PLACEHOLDER: &str = "###SLOT###";

#[derive(Debug, Clone, Serialize)]
pub struct LaunchBundle {
    pub version: u32,
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_source_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_body: Option<String>,
    pub routing: Routing,
    pub execution_policy: ExecutionPolicy,
    pub prompt_surface: PromptSurface,
    pub scaffold_slots: ScaffoldSlots,
    pub tools: ToolsSpec,
    pub skills: Skills,
    pub provenance: BTreeMap<String, String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Routing {
    pub model: String,
    pub model_token: String,
    pub provider_constraint: Option<String>,
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
pub struct ExecutionPolicy {
    pub effort: Option<String>,
    pub approval: Option<String>,
    pub sandbox: Option<String>,
    pub network: ExecutionNetworkPolicy,
    pub autocompact: Option<u32>,
    pub autocompact_pct: Option<u8>,
    pub timeout: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_config: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExecutionNetworkPolicy {
    pub mode: Option<NetworkPolicy>,
    pub allow: Vec<NetworkHost>,
    pub deny: Vec<NetworkHost>,
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

/// The agent's tool policy for the resolved harness.
///
/// `rules` is the contract: one entry per policy statement, carrying the canonical
/// capability for meaning, the harness-native permission names for emission and the unscoped
/// harness tool names for availability projection. `allowed`,
/// `disallowed` and `mcp` are flat native-name lists derived from `rules` for consumers that
/// predate it; they cannot express `ask`, `*`, or which capability a name came from.
/// In particular, `mcp` contains permission names, never MCP server definitions or commands.
/// `capability_natives` is independent of the profile and always names the capabilities a
/// launcher may deny as platform policy.
#[derive(Debug, Clone, Serialize)]
pub struct ToolsSpec {
    pub rules: Vec<ToolRuleSpec>,
    pub capability_natives: BTreeMap<String, CapabilityNativeSpec>,
    pub allowed: Vec<String>,
    pub disallowed: Vec<String>,
    pub mcp: Vec<String>,
}

/// How the resolved harness spells one capability that a launcher may gate independently of
/// the profile's tool rules.
#[derive(Debug, Clone, Serialize)]
pub struct CapabilityNativeSpec {
    pub native: Vec<String>,
    pub native_status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
}

/// One tool-policy statement as the resolved harness spells it: the rule's own
/// `{capability, pattern, action}` (see [`ToolRule`]'s serialization) plus the names.
#[derive(Debug, Clone, Serialize)]
pub struct ToolRuleSpec {
    #[serde(flatten)]
    pub rule: ToolRule,
    /// The harness's names for this rule: one per native tool the capability covers, with the
    /// pattern appended as `name(pattern)`. Empty unless `native_status` is `known`, and
    /// possibly empty for a known rule shadowed by a stricter or more specific rule.
    pub native: Vec<String>,
    /// The unscoped harness tool names this rule concerns. Permission engines can
    /// name a scoped category rather than the tool being made available (for example Claude
    /// `write(path)` uses `Edit(path)` but concerns the `Write` tool). Empty whenever `native`
    /// is empty, with the same `native_status` applying to both fields.
    pub native_tools: Vec<String>,
    /// `known`: `native` names it. `absent`: the harness has no such tool, so there is nothing
    /// to enforce. `unknown`: Mars cannot name it for this harness, a gap to report. Omitted
    /// for `*`, which names no tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_status: Option<&'static str>,
    /// Why a consumer cannot rely on this rule alone, and the only channel for a permission
    /// gap: Mars adds nothing to `warnings[]`. Present when `native_status` is `unknown` (the
    /// reason Mars could not name the rule, for example "codex loads skills by reading files;
    /// no switch") and on a `known` rule whose projection has broader effects or whose native
    /// permission is shared with another rule.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Skills {
    pub loaded: Vec<LoadedSkill>,
    pub available: Vec<AvailableSkill>,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoadedSkill {
    pub name: String,
    pub skill_type: String,
    pub source_path: PathBuf,
    /// Raw skill body without heading. Consumers render headings themselves.
    pub body: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AvailableSkill {
    pub name: String,
    pub skill_type: String,
    pub description: String,
    pub source_path: PathBuf,
}
