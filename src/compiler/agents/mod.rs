/// Agent-profile schema, parser, and validation.
///
/// Parses agent markdown frontmatter into strongly-typed [`AgentProfile`] fields.
/// Used by the dual-surface compilation pipeline to:
/// - Validate agent profiles at compile time
/// - Route agents to the correct harness-native output surface
/// - Report lossiness diagnostics when fields cannot be expressed in a target format
pub mod lower;

use std::collections::BTreeMap;

use serde_yaml::Value;

use crate::compiler::invocability::{find_invocability_field, parse_invocability_axis};
use crate::compiler::tool_policy::{self, EffectiveToolPolicy, ParsedToolsField};
pub use crate::config::{ModelPolicyMatchType, ModelPolicyRule};
use crate::frontmatter::{Frontmatter, FrontmatterError, SkillsSpec};

// ---------------------------------------------------------------------------
// Field enums
// ---------------------------------------------------------------------------

/// Agent execution mode — how the agent is launched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentMode {
    Primary,
    Subagent,
}

impl AgentMode {
    pub fn as_str(&self) -> &str {
        match self {
            AgentMode::Primary => "primary",
            AgentMode::Subagent => "subagent",
        }
    }
}

impl std::fmt::Display for AgentMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Known harness execution targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessKind {
    Claude,
    Codex,
    OpenCode,
    Cursor,
    Pi,
}

impl HarnessKind {
    pub fn all() -> &'static [Self] {
        const ALL: &[HarnessKind] = &[
            HarnessKind::Claude,
            HarnessKind::Codex,
            HarnessKind::Pi,
            HarnessKind::OpenCode,
            HarnessKind::Cursor,
        ];
        ALL
    }

    /// Parse from a frontmatter string value.
    pub fn from_str(s: &str) -> Option<Self> {
        crate::harness::registry::parse(s).map(Self::from_harness_id)
    }

    /// Target directory root for harness-native artifacts.
    pub fn target_dir(&self) -> &str {
        self.to_harness_id().default_target()
    }

    pub fn to_harness_id(&self) -> crate::harness::registry::HarnessId {
        match self {
            Self::Claude => crate::harness::registry::HarnessId::Claude,
            Self::Codex => crate::harness::registry::HarnessId::Codex,
            Self::OpenCode => crate::harness::registry::HarnessId::OpenCode,
            Self::Cursor => crate::harness::registry::HarnessId::Cursor,
            Self::Pi => crate::harness::registry::HarnessId::Pi,
        }
    }

    pub fn from_harness_id(id: crate::harness::registry::HarnessId) -> Self {
        match id {
            crate::harness::registry::HarnessId::Claude => Self::Claude,
            crate::harness::registry::HarnessId::Codex => Self::Codex,
            crate::harness::registry::HarnessId::Pi => Self::Pi,
            crate::harness::registry::HarnessId::OpenCode => Self::OpenCode,
            crate::harness::registry::HarnessId::Cursor => Self::Cursor,
        }
    }

    /// Resolve a linked target directory (e.g. `.claude`) to its harness kind.
    pub fn from_target_dir(target_root: &str) -> Option<Self> {
        Self::all()
            .iter()
            .find(|harness| harness.target_dir() == target_root)
            .cloned()
    }

    /// Inbound dialect for this harness (`None` for Pi — no foreign import surface).
    pub fn to_dialect(&self) -> Option<crate::dialect::Dialect> {
        crate::dialect::Dialect::from_harness_id(self.to_harness_id())
    }

    /// Compiler harness for an inbound dialect (`None` for `MarsNative`).
    pub fn from_dialect(dialect: crate::dialect::Dialect) -> Option<Self> {
        dialect.to_harness_id().map(Self::from_harness_id)
    }
}

/// Approval policy field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalMode {
    Default,
    Auto,
    Confirm,
    Never,
}

impl ApprovalMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "default" => Some(Self::Default),
            "auto" => Some(Self::Auto),
            "confirm" => Some(Self::Confirm),
            "never" | "yolo" => Some(Self::Never),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Default => "default",
            Self::Auto => "auto",
            Self::Confirm => "confirm",
            Self::Never => "never",
        }
    }
}

/// Sandbox mode field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxMode {
    Default,
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

impl SandboxMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "default" => Some(Self::Default),
            "read-only" => Some(Self::ReadOnly),
            "workspace-write" => Some(Self::WorkspaceWrite),
            "danger-full-access" => Some(Self::DangerFullAccess),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Default => "default",
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }
}

/// Effort level field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffortLevel {
    Low,
    Medium,
    High,
    XHigh,
}

impl EffortLevel {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" | "max" => Some(Self::XHigh),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
        }
    }

    /// Normalized value for Claude ("xhigh" → "max").
    pub fn claude_str(&self) -> &str {
        match self {
            Self::XHigh => "max",
            other => other.as_str(),
        }
    }
}

// ---------------------------------------------------------------------------
// Harness-native passthrough overrides
// ---------------------------------------------------------------------------

/// Per-harness target-native passthrough blocks (`harness-overrides:`).
///
/// Mars validates only shape/serializability. Nested keys are owned by the
/// target harness and are not interpreted as Mars semantic fields.
#[derive(Debug, Clone, Default)]
pub struct HarnessOverrides {
    pub entries: BTreeMap<String, serde_json::Map<String, serde_json::Value>>,
}

fn harness_key(harness: &HarnessKind) -> &'static str {
    match harness {
        HarnessKind::Claude => "claude",
        HarnessKind::Codex => "codex",
        HarnessKind::OpenCode => "opencode",
        HarnessKind::Cursor => "cursor",
        HarnessKind::Pi => "pi",
    }
}

impl HarnessOverrides {
    pub fn get(
        &self,
        harness: &HarnessKind,
    ) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.entries.get(harness_key(harness))
    }
}

/// Marker for a validated fanout inventory entry (`fanout:`).
///
/// Fanout is metadata-only (D43): it never gains lowering behavior.
/// Mars parses it for validation and preservation; no harness-native artifact
/// receives fanout entries.
#[derive(Debug, Clone)]
pub struct FanoutEntry;

// ---------------------------------------------------------------------------
// AgentProfile — the fully parsed frontmatter
// ---------------------------------------------------------------------------

/// Strongly-typed representation of an agent profile's frontmatter.
///
/// Parsed from YAML frontmatter by [`parse_agent_profile`].
/// Used for:
/// - Compile-time validation (mode values, passthrough shape)
/// - Dual-surface routing (harness → output target)
/// - Per-target lowering (field lowering per agent-compilation-mapping.md)
#[derive(Debug, Clone)]
pub struct AgentProfile {
    // --- Identity fields ---
    pub name: Option<String>,
    pub description: Option<String>,

    // --- Routing fields ---
    pub harness: Option<HarnessKind>,

    // --- Model fields ---
    pub model: Option<String>,

    // --- Runtime policy fields ---
    pub mode: Option<AgentMode>,
    pub model_invocable: bool,
    /// Whether the user can invoke this agent directly (slash command, picker, etc.).
    pub user_invocable: bool,
    /// `true` when source frontmatter explicitly set `model-invocable` (kebab or snake).
    pub had_model_invocable_field: bool,
    /// `true` when source frontmatter explicitly set `user-invocable` (kebab or snake).
    pub had_user_invocable_field: bool,
    pub approval: Option<ApprovalMode>,
    pub sandbox: Option<SandboxMode>,
    pub effort: Option<EffortLevel>,
    pub autocompact: Option<u32>,
    pub autocompact_pct: Option<u8>,

    // --- Tool fields ---
    pub skills: SkillsSpec,
    pub subagents: Vec<String>,
    pub tools: Vec<String>,
    pub tools_denied: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub mcp_tools: Vec<String>,

    // --- Override tables ---
    pub harness_overrides: HarnessOverrides,
    pub model_policies: Vec<ModelPolicyRule>,
    pub fanout: Vec<FanoutEntry>,
}

impl AgentProfile {
    pub fn effective_skills(&self, _harness: &HarnessKind) -> &SkillsSpec {
        &self.skills
    }

    pub fn effective_native_config(
        &self,
        harness: &HarnessKind,
    ) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.harness_overrides
            .get(harness)
            .filter(|map| !map.is_empty())
    }

    pub fn effective_tool_policy(&self, _harness: &HarnessKind) -> EffectiveToolPolicy {
        tool_policy::effective_tool_policy(
            &self.tools,
            &self.tools_denied,
            &self.disallowed_tools,
            &self.mcp_tools,
        )
    }
}
// ---------------------------------------------------------------------------
// Validation warnings/errors
// ---------------------------------------------------------------------------

/// A validation finding from agent profile parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentDiagnostic {
    /// Field value is not in the allowed set.
    InvalidFieldValue {
        field: String,
        value: String,
        allowed: &'static str,
    },
    /// Deprecated `models:` field was found (use `model-overrides:` instead).
    LegacyModelsField,
    /// Deprecated `approval: yolo` was found (use `approval: never` instead).
    DeprecatedApprovalYolo,
    /// Unknown top-level harness name — not one of claude/codex/opencode/cursor/pi.
    UnknownHarness { value: String },
    /// Unknown harness key under `harness-overrides`; preserved for forward compatibility.
    UnknownHarnessOverride { value: String },
}

impl AgentDiagnostic {
    pub fn is_error(&self) -> bool {
        match self {
            AgentDiagnostic::InvalidFieldValue { field, .. } => {
                !field.starts_with("harness-overrides")
            }
            AgentDiagnostic::UnknownHarness { .. } => true,
            AgentDiagnostic::LegacyModelsField
            | AgentDiagnostic::DeprecatedApprovalYolo
            | AgentDiagnostic::UnknownHarnessOverride { .. } => false,
        }
    }

    pub fn message(&self) -> String {
        match self {
            AgentDiagnostic::InvalidFieldValue {
                field,
                value,
                allowed,
            } => {
                format!("agent field `{field}` has invalid value `{value}`; allowed: {allowed}")
            }
            AgentDiagnostic::LegacyModelsField => {
                "agent uses deprecated `models:` field; rename to `model-overrides:`".to_string()
            }
            AgentDiagnostic::DeprecatedApprovalYolo => {
                "agent uses deprecated `approval: yolo`; use `approval: never` instead".to_string()
            }
            AgentDiagnostic::UnknownHarness { value } => {
                format!("unknown harness `{value}`; known: claude, codex, opencode, cursor, pi")
            }
            AgentDiagnostic::UnknownHarnessOverride { value } => {
                format!("unknown harness override `{value}`; preserving passthrough block")
            }
        }
    }
}

fn push_agent_tool_invalid(
    diags: &mut Vec<AgentDiagnostic>,
    field: &str,
    value: &str,
    allowed: &'static str,
) {
    diags.push(AgentDiagnostic::InvalidFieldValue {
        field: field.to_string(),
        value: value.to_string(),
        allowed,
    });
}

fn yaml_tool_list(field: &str, val: &Value, diags: &mut Vec<AgentDiagnostic>) -> Vec<String> {
    let mut push = |field: &str, value: &str, allowed: &'static str| {
        push_agent_tool_invalid(diags, field, value, allowed);
    };
    tool_policy::yaml_tool_list(field, val, &mut push)
}

fn parse_tools_field(
    field: &str,
    val: &Value,
    diags: &mut Vec<AgentDiagnostic>,
) -> ParsedToolsField {
    let mut push = |field: &str, value: &str, allowed: &'static str| {
        push_agent_tool_invalid(diags, field, value, allowed);
    };
    tool_policy::parse_tools_field(field, val, &mut push)
}

fn parse_native_config_value(
    field: &str,
    value: &Value,
    diags: &mut Vec<AgentDiagnostic>,
) -> Option<serde_json::Value> {
    match value {
        Value::Null => {
            diags.push(AgentDiagnostic::InvalidFieldValue {
                field: field.to_string(),
                value: "null".to_string(),
                allowed: "non-null scalar, array, or map value",
            });
            None
        }
        Value::Bool(v) => Some(serde_json::Value::Bool(*v)),
        Value::String(v) => Some(serde_json::Value::String(v.clone())),
        Value::Number(v) => {
            if let Some(number) = v.as_i64().map(serde_json::Number::from) {
                Some(serde_json::Value::Number(number))
            } else if let Some(number) = v.as_u64().map(serde_json::Number::from) {
                Some(serde_json::Value::Number(number))
            } else if let Some(float) = v.as_f64() {
                match serde_json::Number::from_f64(float) {
                    Some(number) => Some(serde_json::Value::Number(number)),
                    None => {
                        diags.push(AgentDiagnostic::InvalidFieldValue {
                            field: field.to_string(),
                            value: float.to_string(),
                            allowed: "finite JSON number",
                        });
                        None
                    }
                }
            } else {
                diags.push(AgentDiagnostic::InvalidFieldValue {
                    field: field.to_string(),
                    value: format!("{value:?}"),
                    allowed: "JSON number",
                });
                None
            }
        }
        Value::Sequence(seq) => {
            let mut out = Vec::with_capacity(seq.len());
            for (index, entry) in seq.iter().enumerate() {
                let child_field = format!("{field}[{index}]");
                if let Some(parsed) = parse_native_config_value(&child_field, entry, diags) {
                    out.push(parsed);
                }
            }
            Some(serde_json::Value::Array(out))
        }
        Value::Mapping(mapping) => {
            let mut out = serde_json::Map::new();
            for (key, entry) in mapping {
                let Some(key_text) = key.as_str() else {
                    diags.push(AgentDiagnostic::InvalidFieldValue {
                        field: field.to_string(),
                        value: format!("{key:?}"),
                        allowed: "string keys",
                    });
                    continue;
                };
                let child_field = format!("{field}.{key_text}");
                if let Some(parsed) = parse_native_config_value(&child_field, entry, diags) {
                    out.insert(key_text.to_string(), parsed);
                }
            }
            Some(serde_json::Value::Object(out))
        }
        _ => {
            diags.push(AgentDiagnostic::InvalidFieldValue {
                field: field.to_string(),
                value: format!("{value:?}"),
                allowed: "YAML/TOML/JSON-serializable value",
            });
            None
        }
    }
}

fn parse_harness_overrides(val: &Value, diags: &mut Vec<AgentDiagnostic>) -> HarnessOverrides {
    let mut out = HarnessOverrides::default();
    let Some(mapping) = val.as_mapping() else {
        diags.push(AgentDiagnostic::InvalidFieldValue {
            field: "harness-overrides".to_string(),
            value: format!("{val:?}"),
            allowed: "mapping of harness name to target-native config mapping",
        });
        return out;
    };

    for (k, v) in mapping {
        let Some(harness_name) = k.as_str() else {
            diags.push(AgentDiagnostic::InvalidFieldValue {
                field: "harness-overrides".to_string(),
                value: format!("{k:?}"),
                allowed: "string harness keys",
            });
            continue;
        };
        let Some(sub_mapping) = v.as_mapping() else {
            diags.push(AgentDiagnostic::InvalidFieldValue {
                field: format!("harness-overrides.{harness_name}"),
                value: format!("{v:?}"),
                allowed: "mapping of target-native keys",
            });
            continue;
        };
        if !matches!(
            harness_name,
            "claude" | "codex" | "opencode" | "cursor" | "pi"
        ) {
            diags.push(AgentDiagnostic::UnknownHarnessOverride {
                value: harness_name.to_string(),
            });
        }
        let mut parsed = serde_json::Map::new();
        for (target_key, target_value) in sub_mapping {
            let Some(key_text) = target_key.as_str() else {
                diags.push(AgentDiagnostic::InvalidFieldValue {
                    field: format!("harness-overrides.{harness_name}"),
                    value: format!("{target_key:?}"),
                    allowed: "string target-native keys",
                });
                continue;
            };
            let value_field = format!("harness-overrides.{harness_name}.{key_text}");
            if let Some(value) = parse_native_config_value(&value_field, target_value, diags) {
                parsed.insert(key_text.to_string(), value);
            }
        }
        if !parsed.is_empty() {
            out.entries.insert(harness_name.to_string(), parsed);
        }
    }

    out
}

fn push_model_policy_invalid(
    diags: &mut Vec<AgentDiagnostic>,
    field: impl Into<String>,
    value: impl Into<String>,
    allowed: &'static str,
) {
    diags.push(AgentDiagnostic::InvalidFieldValue {
        field: field.into(),
        value: value.into(),
        allowed,
    });
}

fn push_model_policy_parse_error(
    diags: &mut Vec<AgentDiagnostic>,
    position: usize,
    error: crate::config::ModelPolicyRuleParseError,
) {
    let rule_field = format!("model-policies[{position}]");
    match error {
        crate::config::ModelPolicyRuleParseError::RuleMustBeMapping { found } => {
            push_model_policy_invalid(diags, rule_field, found, "mapping with match and override");
        }
        crate::config::ModelPolicyRuleParseError::MatchMissing => {
            push_model_policy_invalid(
                diags,
                format!("{rule_field}.match"),
                "<missing>",
                "mapping with exactly one of model, alias, model-glob",
            );
        }
        crate::config::ModelPolicyRuleParseError::MatchMustBeMapping { found } => {
            push_model_policy_invalid(
                diags,
                format!("{rule_field}.match"),
                found,
                "mapping with exactly one of model, alias, model-glob",
            );
        }
        crate::config::ModelPolicyRuleParseError::MatchMustContainExactlyOne { found } => {
            push_model_policy_invalid(
                diags,
                format!("{rule_field}.match"),
                found,
                "exactly one of model, alias, model-glob",
            );
        }
        crate::config::ModelPolicyRuleParseError::MatchKeyMustBeString { found } => {
            push_model_policy_invalid(
                diags,
                format!("{rule_field}.match"),
                found,
                "model, alias, model-glob",
            );
        }
        crate::config::ModelPolicyRuleParseError::UnknownMatchKey { key } => {
            push_model_policy_invalid(
                diags,
                format!("{rule_field}.match"),
                key,
                "model, alias, model-glob",
            );
        }
        crate::config::ModelPolicyRuleParseError::MatchValueMustBeString { key, found } => {
            push_model_policy_invalid(
                diags,
                format!("{rule_field}.match.{key}"),
                found,
                "non-empty string",
            );
        }
        crate::config::ModelPolicyRuleParseError::MatchValueEmpty { key } => {
            push_model_policy_invalid(
                diags,
                format!("{rule_field}.match.{key}"),
                "<empty>",
                "non-empty string",
            );
        }
        crate::config::ModelPolicyRuleParseError::OverrideMustBeMapping { found } => {
            push_model_policy_invalid(diags, format!("{rule_field}.override"), found, "mapping");
        }
        crate::config::ModelPolicyRuleParseError::NoFallbackMustBeBoolean { found } => {
            push_model_policy_invalid(diags, format!("{rule_field}.no-fallback"), found, "boolean");
        }
    }
}

fn parse_model_policies(val: &Value, diags: &mut Vec<AgentDiagnostic>) -> Vec<ModelPolicyRule> {
    let Some(seq) = val.as_sequence() else {
        push_model_policy_invalid(
            diags,
            "model-policies",
            format!("{val:?}"),
            "sequence of rules",
        );
        return vec![];
    };

    let mut out = Vec::new();
    for (index, entry) in seq.iter().enumerate() {
        let position = index + 1;
        match crate::config::parse_model_policy_rule_value(entry) {
            Ok(rule) => out.push(rule),
            Err(error) => push_model_policy_parse_error(diags, position, error),
        }
    }
    out
}

fn parse_invocability_field(
    field: &str,
    raw: Option<&Value>,
    diags: &mut Vec<AgentDiagnostic>,
) -> (bool, bool) {
    let (value, had_field, invalid) = parse_invocability_axis(raw);
    if let Some(invalid) = invalid {
        diags.push(AgentDiagnostic::InvalidFieldValue {
            field: field.to_string(),
            value: invalid,
            allowed: "boolean",
        });
    }
    (value, had_field)
}

fn parse_fanout(val: &Value) -> Vec<FanoutEntry> {
    match val {
        Value::Sequence(seq) => seq.iter().map(|_| FanoutEntry).collect(),
        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse an agent profile from a [`Frontmatter`].
///
/// Collects diagnostics without failing — the caller decides whether errors
/// are fatal. The parsed [`AgentProfile`] is always returned even when there
/// are validation errors; invalid fields are skipped (omitted from the profile).
pub fn parse_agent_profile(fm: &Frontmatter, diags: &mut Vec<AgentDiagnostic>) -> AgentProfile {
    let name = fm.name().map(str::to_owned);
    let description = fm
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_owned);

    // harness:
    let harness = fm.get("harness").and_then(Value::as_str).and_then(|s| {
        if let Some(h) = HarnessKind::from_str(s) {
            Some(h)
        } else {
            diags.push(AgentDiagnostic::UnknownHarness {
                value: s.to_string(),
            });
            None
        }
    });

    // model:
    let model = fm.get("model").and_then(Value::as_str).map(str::to_owned);

    // mode:
    let mode = fm
        .get("mode")
        .and_then(Value::as_str)
        .and_then(|s| match s {
            "primary" => Some(AgentMode::Primary),
            "subagent" => Some(AgentMode::Subagent),
            other => {
                diags.push(AgentDiagnostic::InvalidFieldValue {
                    field: "mode".to_string(),
                    value: other.to_string(),
                    allowed: "primary, subagent",
                });
                None
            }
        });

    // model-invocable / user-invocable:
    let model_invocability = find_invocability_field(fm, "model-invocable");
    let (model_invocable, had_model_invocable_field) = parse_invocability_field(
        "model-invocable",
        model_invocability.as_ref().map(|f| &f.value),
        diags,
    );
    let user_invocability = find_invocability_field(fm, "user-invocable");
    let (user_invocable, had_user_invocable_field) = parse_invocability_field(
        "user-invocable",
        user_invocability.as_ref().map(|f| &f.value),
        diags,
    );

    // approval:
    let approval = fm.get("approval").and_then(Value::as_str).and_then(|s| {
        if let Some(a) = ApprovalMode::from_str(s) {
            if s == "yolo" {
                diags.push(AgentDiagnostic::DeprecatedApprovalYolo);
            }
            Some(a)
        } else {
            diags.push(AgentDiagnostic::InvalidFieldValue {
                field: "approval".to_string(),
                value: s.to_string(),
                allowed: "default, auto, confirm, never",
            });
            None
        }
    });

    // sandbox:
    let sandbox = fm.get("sandbox").and_then(Value::as_str).and_then(|s| {
        if let Some(sb) = SandboxMode::from_str(s) {
            Some(sb)
        } else {
            diags.push(AgentDiagnostic::InvalidFieldValue {
                field: "sandbox".to_string(),
                value: s.to_string(),
                allowed: "default, read-only, workspace-write, danger-full-access",
            });
            None
        }
    });

    // effort:
    let effort = fm.get("effort").and_then(Value::as_str).and_then(|s| {
        if s == "none" {
            // "none" is a valid sentinel meaning "no effort level" (same as omitting the field)
            return None;
        }
        if let Some(e) = EffortLevel::from_str(s) {
            Some(e)
        } else {
            diags.push(AgentDiagnostic::InvalidFieldValue {
                field: "effort".to_string(),
                value: s.to_string(),
                allowed: "low, medium, high, xhigh, none",
            });
            None
        }
    });

    // autocompact:
    let autocompact = match fm.get("autocompact") {
        None => None,
        Some(v) => {
            if let Some(n) = v.as_u64() {
                match u32::try_from(n) {
                    Ok(v32) => Some(v32),
                    Err(_) => {
                        diags.push(AgentDiagnostic::InvalidFieldValue {
                            field: "autocompact".to_string(),
                            value: n.to_string(),
                            allowed: "integer 0–4294967295",
                        });
                        None
                    }
                }
            } else {
                diags.push(AgentDiagnostic::InvalidFieldValue {
                    field: "autocompact".to_string(),
                    value: format!("{v:?}"),
                    allowed: "integer (token count)",
                });
                None
            }
        }
    };

    // autocompact_pct:
    let autocompact_pct = match fm.get("autocompact_pct") {
        None => None,
        Some(v) => {
            if let Some(n) = v.as_u64() {
                if (1..=100).contains(&n) {
                    Some(n as u8)
                } else {
                    diags.push(AgentDiagnostic::InvalidFieldValue {
                        field: "autocompact_pct".to_string(),
                        value: n.to_string(),
                        allowed: "integer 1–100",
                    });
                    None
                }
            } else {
                diags.push(AgentDiagnostic::InvalidFieldValue {
                    field: "autocompact_pct".to_string(),
                    value: format!("{v:?}"),
                    allowed: "integer 1–100",
                });
                None
            }
        }
    };

    // skills/subagents/tools/disallowed-tools/mcp-tools:
    let skills = fm.skills_structured();
    let subagents = fm
        .get("subagents")
        .map(tool_policy::yaml_str_list)
        .unwrap_or_default();
    let parsed_tools = fm
        .get("tools")
        .map(|value| parse_tools_field("tools", value, diags))
        .unwrap_or_default();
    let tools = parsed_tools.allowed;
    let tools_denied = parsed_tools.denied;
    let disallowed_tools = fm
        .get("disallowed-tools")
        .map(|value| yaml_tool_list("disallowed-tools", value, diags))
        .unwrap_or_default();
    let mcp_tools = tool_policy::legacy_mcp_tools_from_frontmatter(fm);

    // harness-overrides:
    let harness_overrides = fm
        .get("harness-overrides")
        .map(|v| parse_harness_overrides(v, diags))
        .unwrap_or_default();

    // model-policies:
    let model_policies = fm
        .get("model-policies")
        .map(|value| parse_model_policies(value, diags))
        .unwrap_or_default();

    // fanout:
    let fanout = fm.get("fanout").map(parse_fanout).unwrap_or_default();

    // Legacy models: field
    if fm.get("models").is_some() {
        diags.push(AgentDiagnostic::LegacyModelsField);
    }

    AgentProfile {
        name,
        description,
        harness,
        model,
        mode,
        model_invocable,
        user_invocable,
        had_model_invocable_field,
        had_user_invocable_field,
        approval,
        sandbox,
        effort,
        autocompact,
        autocompact_pct,
        skills,
        subagents,
        tools,
        tools_denied,
        disallowed_tools,
        mcp_tools,
        harness_overrides,
        model_policies,
        fanout,
    }
}

/// Parse an agent profile from raw markdown content.
///
/// Convenience wrapper over [`parse_agent_profile`].
pub fn parse_agent_content(
    content: &str,
    diags: &mut Vec<AgentDiagnostic>,
) -> Result<(AgentProfile, Frontmatter), FrontmatterError> {
    let fm = Frontmatter::parse(content)?;
    let profile = parse_agent_profile(&fm, diags);
    Ok((profile, fm))
}

#[cfg(test)]
mod tests;
