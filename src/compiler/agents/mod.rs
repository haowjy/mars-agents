/// Agent-profile schema, parser, and validation.
///
/// Parses agent markdown frontmatter into strongly-typed [`AgentProfile`] fields.
/// Used by the dual-surface compilation pipeline to:
/// - Validate agent profiles at compile time
/// - Route agents to the correct harness-native output surface
/// - Report lossiness diagnostics when fields cannot be expressed in a target format
pub mod lower;

use serde_yaml::Value;

use crate::frontmatter::{Frontmatter, FrontmatterError};

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
        &[
            Self::Claude,
            Self::Codex,
            Self::OpenCode,
            Self::Cursor,
            Self::Pi,
        ]
    }

    /// Parse from a frontmatter string value.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "opencode" => Some(Self::OpenCode),
            "cursor" => Some(Self::Cursor),
            "pi" => Some(Self::Pi),
            _ => None,
        }
    }

    /// Target directory root for harness-native artifacts.
    pub fn target_dir(&self) -> &str {
        match self {
            Self::Claude => ".claude",
            Self::Codex => ".codex",
            Self::OpenCode => ".opencode",
            Self::Cursor => ".cursor",
            Self::Pi => ".pi",
        }
    }
}

/// Approval policy field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalMode {
    Default,
    Auto,
    Confirm,
    Yolo,
}

impl ApprovalMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "default" => Some(Self::Default),
            "auto" => Some(Self::Auto),
            "confirm" => Some(Self::Confirm),
            "yolo" => Some(Self::Yolo),
            _ => None,
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
// Override table types
// ---------------------------------------------------------------------------

/// A set of overridable field values for one harness or model override entry.
/// Only fields explicitly specified in the override block are present.
#[derive(Debug, Clone, Default)]
pub struct OverrideFields {
    pub effort: Option<EffortLevel>,
    pub autocompact: Option<u32>,
    pub autocompact_pct: Option<u8>,
    pub approval: Option<ApprovalMode>,
    pub sandbox: Option<SandboxMode>,
    pub skills: Option<Vec<String>>,
    pub tools: Option<Vec<String>>,
    pub tools_denied: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
    pub mcp_tools: Option<Vec<String>>,
    pub native_config: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Per-harness override table (`harness-overrides:`).
#[derive(Debug, Clone, Default)]
pub struct HarnessOverrides {
    pub claude: Option<OverrideFields>,
    pub codex: Option<OverrideFields>,
    pub opencode: Option<OverrideFields>,
    pub cursor: Option<OverrideFields>,
    pub pi: Option<OverrideFields>,
}

impl HarnessOverrides {
    pub fn get(&self, harness: &HarnessKind) -> Option<&OverrideFields> {
        match harness {
            HarnessKind::Claude => self.claude.as_ref(),
            HarnessKind::Codex => self.codex.as_ref(),
            HarnessKind::OpenCode => self.opencode.as_ref(),
            HarnessKind::Cursor => self.cursor.as_ref(),
            HarnessKind::Pi => self.pi.as_ref(),
        }
    }
}

/// Parsed `model-policies:` entry.
///
/// Per the spec (D43), model-policies are consumed by Meridian at runtime.
/// Mars parses them at compile time only for validation and preservation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPolicyEntry {
    pub match_type: ModelPolicyMatchType,
    pub match_value: String,
    pub no_fallback: bool,
    pub overrides: serde_yaml::Mapping,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelPolicyMatchType {
    Model,
    Alias,
    ModelGlob,
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
/// - Compile-time validation (mode values, non-overridable fields in overrides)
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
    pub approval: Option<ApprovalMode>,
    pub sandbox: Option<SandboxMode>,
    pub effort: Option<EffortLevel>,
    pub autocompact: Option<u32>,
    pub autocompact_pct: Option<u8>,

    // --- Tool fields ---
    pub skills: Vec<String>,
    pub tools: Vec<String>,
    pub tools_denied: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub mcp_tools: Vec<String>,

    // --- Override tables ---
    pub harness_overrides: HarnessOverrides,
    pub model_policies: Vec<ModelPolicyEntry>,
    pub fanout: Vec<FanoutEntry>,
}

/// Portable tool policy after applying harness override replacement semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveToolPolicy {
    pub allowed: Vec<String>,
    pub disallowed: Vec<String>,
    pub mcp: Vec<String>,
}

impl AgentProfile {
    pub fn effective_skills(&self, harness: &HarnessKind) -> &[String] {
        self.harness_overrides
            .get(harness)
            .and_then(|entry| entry.skills.as_ref())
            .unwrap_or(&self.skills)
    }

    pub fn effective_native_config(
        &self,
        harness: &HarnessKind,
    ) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.harness_overrides
            .get(harness)
            .and_then(|entry| entry.native_config.as_ref())
            .filter(|map| !map.is_empty())
    }

    pub fn effective_tool_policy(&self, harness: &HarnessKind) -> EffectiveToolPolicy {
        let overrides = self.harness_overrides.get(harness);
        let allowed = overrides
            .and_then(|entry| entry.tools.clone())
            .unwrap_or_else(|| self.tools.clone());
        let tools_denied = overrides
            .and_then(|entry| entry.tools_denied.clone())
            .unwrap_or_else(|| self.tools_denied.clone());
        let explicit_disallowed = overrides
            .and_then(|entry| entry.disallowed_tools.clone())
            .unwrap_or_else(|| self.disallowed_tools.clone());
        let mcp = overrides
            .and_then(|entry| entry.mcp_tools.clone())
            .unwrap_or_else(|| self.mcp_tools.clone());

        EffectiveToolPolicy {
            allowed: dedupe_ordered(allowed),
            disallowed: dedupe_ordered(
                tools_denied
                    .into_iter()
                    .chain(explicit_disallowed)
                    .collect(),
            ),
            mcp: dedupe_ordered(mcp),
        }
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
    /// Unknown harness name — not one of claude/codex/opencode/pi.
    UnknownHarness { value: String },
    /// Non-overridable field appears inside an override block.
    NonOverridableFieldInOverride { field: String, table: String },
    /// `native-config` key collides with a known portable policy field name.
    NativeConfigPortableKeyCollision { key: String, table: String },
}

impl AgentDiagnostic {
    pub fn is_error(&self) -> bool {
        matches!(
            self,
            AgentDiagnostic::InvalidFieldValue { .. }
                | AgentDiagnostic::UnknownHarness { .. }
                | AgentDiagnostic::NonOverridableFieldInOverride { .. }
        )
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
            AgentDiagnostic::UnknownHarness { value } => {
                format!("unknown harness `{value}`; known: claude, codex, opencode, cursor, pi")
            }
            AgentDiagnostic::NonOverridableFieldInOverride { field, table } => {
                format!("field `{field}` is not overridable; remove from `{table}`")
            }
            AgentDiagnostic::NativeConfigPortableKeyCollision { key, table } => format!(
                "native-config key `{key}` in `{table}` collides with a portable field name; preserving as native-config"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Non-overridable field names (compile error if inside an override block)
// ---------------------------------------------------------------------------

const NON_OVERRIDABLE: &[&str] = &[
    "name",
    "description",
    "model",
    "harness",
    "mode",
    "model-invocable",
    "model-overrides",
    "harness-overrides",
];

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

fn yaml_str_list(val: &Value) -> Vec<String> {
    match val {
        Value::Sequence(seq) => seq
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::to_owned)
            .collect(),
        Value::String(s) => vec![s.clone()],
        _ => vec![],
    }
}

fn normalize_tool_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let (head, tail) = match trimmed.find('(') {
        Some(index) => (&trimmed[..index], &trimmed[index..]),
        None => (trimmed, ""),
    };
    let canonical = match head {
        value if value.eq_ignore_ascii_case("bash") => "Bash",
        value if value.eq_ignore_ascii_case("read") => "Read",
        value if value.eq_ignore_ascii_case("write") => "Write",
        value if value.eq_ignore_ascii_case("edit") => "Edit",
        value if value.eq_ignore_ascii_case("agent") => "Agent",
        _ => head,
    };
    format!("{canonical}{tail}")
}

fn dedupe_ordered(values: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = trimmed.to_string();
        if seen.insert(key.clone()) {
            out.push(key);
        }
    }
    out
}

fn yaml_tool_list(val: &Value) -> Vec<String> {
    dedupe_ordered(
        yaml_str_list(val)
            .into_iter()
            .map(|tool| normalize_tool_name(&tool))
            .collect(),
    )
}

#[derive(Default)]
struct ParsedToolsField {
    allowed: Vec<String>,
    denied: Vec<String>,
}

fn parse_tools_field(
    field: &str,
    val: &Value,
    diags: &mut Vec<AgentDiagnostic>,
) -> ParsedToolsField {
    match val {
        Value::Mapping(mapping) => {
            let mut allowed = Vec::new();
            let mut denied = Vec::new();
            for (key, value) in mapping {
                let Some(tool_name) = key.as_str() else {
                    diags.push(AgentDiagnostic::InvalidFieldValue {
                        field: field.to_string(),
                        value: format!("{key:?}"),
                        allowed: "string tool keys",
                    });
                    continue;
                };

                let Some(policy) = value.as_str() else {
                    diags.push(AgentDiagnostic::InvalidFieldValue {
                        field: format!("{field}.{tool_name}"),
                        value: format!("{value:?}"),
                        allowed: "allow or deny",
                    });
                    continue;
                };

                let normalized_tool = normalize_tool_name(tool_name);
                if policy.eq_ignore_ascii_case("allow") {
                    allowed.push(normalized_tool);
                } else if policy.eq_ignore_ascii_case("deny") {
                    denied.push(normalized_tool);
                } else {
                    diags.push(AgentDiagnostic::InvalidFieldValue {
                        field: format!("{field}.{tool_name}"),
                        value: policy.to_string(),
                        allowed: "allow or deny",
                    });
                }
            }
            ParsedToolsField {
                allowed: dedupe_ordered(allowed),
                denied: dedupe_ordered(denied),
            }
        }
        _ => ParsedToolsField {
            allowed: yaml_tool_list(val),
            denied: vec![],
        },
    }
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
                let parsed = parse_native_config_value(&child_field, entry, diags)?;
                out.push(parsed);
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
                    return None;
                };
                let child_field = format!("{field}.{key_text}");
                let parsed = parse_native_config_value(&child_field, entry, diags)?;
                out.insert(key_text.to_string(), parsed);
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

fn parse_native_config_map(
    field: &str,
    val: &Value,
    diags: &mut Vec<AgentDiagnostic>,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    const PORTABLE_FIELD_NAMES: &[&str] = &[
        "sandbox",
        "approval",
        "effort",
        "autocompact",
        "autocompact_pct",
        "skills",
        "tools",
        "disallowed-tools",
        "mcp-tools",
    ];

    let Some(mapping) = val.as_mapping() else {
        diags.push(AgentDiagnostic::InvalidFieldValue {
            field: field.to_string(),
            value: format!("{val:?}"),
            allowed: "mapping with string keys and non-null serializable values",
        });
        return None;
    };

    let mut out = serde_json::Map::new();
    for (key, value) in mapping {
        let Some(key_text) = key.as_str() else {
            diags.push(AgentDiagnostic::InvalidFieldValue {
                field: field.to_string(),
                value: format!("{key:?}"),
                allowed: "string keys",
            });
            return None;
        };

        if PORTABLE_FIELD_NAMES.contains(&key_text) {
            diags.push(AgentDiagnostic::NativeConfigPortableKeyCollision {
                key: key_text.to_string(),
                table: field.to_string(),
            });
        }

        let value_field = format!("{field}.{key_text}");
        let parsed = parse_native_config_value(&value_field, value, diags)?;
        out.insert(key_text.to_string(), parsed);
    }

    Some(out)
}

fn parse_override_fields(
    mapping: &serde_yaml::Mapping,
    table_name: &str,
    diags: &mut Vec<AgentDiagnostic>,
) -> OverrideFields {
    let mut out = OverrideFields::default();

    for (k, v) in mapping {
        let key = match k.as_str() {
            Some(s) => s,
            None => continue,
        };

        if NON_OVERRIDABLE.contains(&key) {
            diags.push(AgentDiagnostic::NonOverridableFieldInOverride {
                field: key.to_string(),
                table: table_name.to_string(),
            });
            continue;
        }

        match key {
            "effort" => {
                if let Some(s) = v.as_str() {
                    if let Some(e) = EffortLevel::from_str(s) {
                        out.effort = Some(e);
                    } else {
                        diags.push(AgentDiagnostic::InvalidFieldValue {
                            field: format!("{table_name}.effort"),
                            value: s.to_string(),
                            allowed: "low, medium, high, xhigh",
                        });
                    }
                }
            }
            "autocompact" => {
                if let Some(n) = v.as_u64() {
                    match u32::try_from(n) {
                        Ok(v32) => out.autocompact = Some(v32),
                        Err(_) => diags.push(AgentDiagnostic::InvalidFieldValue {
                            field: format!("{table_name}.autocompact"),
                            value: n.to_string(),
                            allowed: "integer 0–4294967295",
                        }),
                    }
                } else {
                    diags.push(AgentDiagnostic::InvalidFieldValue {
                        field: format!("{table_name}.autocompact"),
                        value: format!("{v:?}"),
                        allowed: "integer (token count)",
                    });
                }
            }
            "autocompact_pct" => {
                if let Some(n) = v.as_u64() {
                    if (1..=100).contains(&n) {
                        out.autocompact_pct = Some(n as u8);
                    } else {
                        diags.push(AgentDiagnostic::InvalidFieldValue {
                            field: format!("{table_name}.autocompact_pct"),
                            value: n.to_string(),
                            allowed: "integer 1–100",
                        });
                    }
                } else {
                    diags.push(AgentDiagnostic::InvalidFieldValue {
                        field: format!("{table_name}.autocompact_pct"),
                        value: format!("{v:?}"),
                        allowed: "integer 1–100",
                    });
                }
            }
            "approval" => {
                if let Some(s) = v.as_str() {
                    if let Some(a) = ApprovalMode::from_str(s) {
                        out.approval = Some(a);
                    } else {
                        diags.push(AgentDiagnostic::InvalidFieldValue {
                            field: format!("{table_name}.approval"),
                            value: s.to_string(),
                            allowed: "default, auto, confirm, yolo",
                        });
                    }
                }
            }
            "sandbox" => {
                if let Some(s) = v.as_str() {
                    if let Some(sb) = SandboxMode::from_str(s) {
                        out.sandbox = Some(sb);
                    } else {
                        diags.push(AgentDiagnostic::InvalidFieldValue {
                            field: format!("{table_name}.sandbox"),
                            value: s.to_string(),
                            allowed: "default, read-only, workspace-write, danger-full-access",
                        });
                    }
                }
            }
            "skills" => {
                out.skills = Some(yaml_str_list(v));
            }
            "tools" => {
                let parsed = parse_tools_field(&format!("{table_name}.tools"), v, diags);
                out.tools = Some(parsed.allowed);
                out.tools_denied = Some(parsed.denied);
            }
            "disallowed-tools" => {
                out.disallowed_tools = Some(yaml_tool_list(v));
            }
            "mcp-tools" => {
                out.mcp_tools = Some(yaml_str_list(v));
            }
            "native-config" => {
                out.native_config =
                    parse_native_config_map(&format!("{table_name}.native-config"), v, diags);
            }
            _ => {
                // Unknown override field — tolerate (forward compat).
            }
        }
    }

    out
}

fn parse_harness_overrides(val: &Value, diags: &mut Vec<AgentDiagnostic>) -> HarnessOverrides {
    let mut out = HarnessOverrides::default();
    let Some(mapping) = val.as_mapping() else {
        return out;
    };

    for (k, v) in mapping {
        let harness_name = match k.as_str() {
            Some(s) => s,
            None => continue,
        };
        let sub_mapping = match v.as_mapping() {
            Some(m) => m,
            None => continue,
        };
        let table_name = format!("harness-overrides.{harness_name}");
        let fields = parse_override_fields(sub_mapping, &table_name, diags);
        match harness_name {
            "claude" => out.claude = Some(fields),
            "codex" => out.codex = Some(fields),
            "opencode" => out.opencode = Some(fields),
            "cursor" => out.cursor = Some(fields),
            "pi" => out.pi = Some(fields),
            other => {
                diags.push(AgentDiagnostic::UnknownHarness {
                    value: other.to_string(),
                });
            }
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

fn parse_model_policies(val: &Value, diags: &mut Vec<AgentDiagnostic>) -> Vec<ModelPolicyEntry> {
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
        let Some(rule) = entry.as_mapping() else {
            push_model_policy_invalid(
                diags,
                format!("model-policies[{position}]"),
                format!("{entry:?}"),
                "mapping with match and override",
            );
            continue;
        };

        let match_value = rule.get(Value::String("match".to_string()));
        let Some(match_mapping) = match_value.and_then(Value::as_mapping) else {
            push_model_policy_invalid(
                diags,
                format!("model-policies[{position}].match"),
                match_value
                    .map(|value| format!("{value:?}"))
                    .unwrap_or_else(|| "<missing>".to_string()),
                "mapping with exactly one of model, alias, model-glob",
            );
            continue;
        };

        let normalized_match_keys: Vec<&str> =
            match_mapping.keys().filter_map(Value::as_str).collect();
        if normalized_match_keys.len() != 1 {
            push_model_policy_invalid(
                diags,
                format!("model-policies[{position}].match"),
                format!("{match_mapping:?}"),
                "exactly one of model, alias, model-glob",
            );
            continue;
        }
        let match_key = normalized_match_keys[0];
        if !matches!(match_key, "model" | "alias" | "model-glob") {
            push_model_policy_invalid(
                diags,
                format!("model-policies[{position}].match"),
                match_key,
                "model, alias, model-glob",
            );
            continue;
        }
        let raw_match_value = match_mapping.get(Value::String(match_key.to_string()));
        let Some(match_text) = raw_match_value.and_then(Value::as_str).map(str::trim) else {
            push_model_policy_invalid(
                diags,
                format!("model-policies[{position}].match.{match_key}"),
                raw_match_value
                    .map(|value| format!("{value:?}"))
                    .unwrap_or_else(|| "<missing>".to_string()),
                "non-empty string",
            );
            continue;
        };
        if match_text.is_empty() {
            push_model_policy_invalid(
                diags,
                format!("model-policies[{position}].match.{match_key}"),
                "<empty>",
                "non-empty string",
            );
            continue;
        }

        let override_value = rule.get(Value::String("override".to_string()));
        let empty_override = serde_yaml::Mapping::new();
        let override_mapping = match override_value {
            None | Some(Value::Null) => &empty_override,
            Some(value) => {
                let Some(mapping) = value.as_mapping() else {
                    push_model_policy_invalid(
                        diags,
                        format!("model-policies[{position}].override"),
                        format!("{value:?}"),
                        "mapping",
                    );
                    continue;
                };
                mapping
            }
        };

        let no_fallback = match rule.get(Value::String("no-fallback".to_string())) {
            None | Some(Value::Null) => false,
            Some(Value::Bool(value)) => *value,
            Some(value) => {
                push_model_policy_invalid(
                    diags,
                    format!("model-policies[{position}].no-fallback"),
                    format!("{value:?}"),
                    "boolean",
                );
                continue;
            }
        };

        let match_type = match match_key {
            "model" => ModelPolicyMatchType::Model,
            "alias" => ModelPolicyMatchType::Alias,
            "model-glob" => ModelPolicyMatchType::ModelGlob,
            _ => unreachable!("match_key was validated above"),
        };

        out.push(ModelPolicyEntry {
            match_type,
            match_value: match_text.to_string(),
            no_fallback,
            overrides: override_mapping.clone(),
        });
    }
    out
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

    // model-invocable:
    let model_invocable = match fm.get("model-invocable") {
        None => true,
        Some(Value::Bool(value)) => *value,
        Some(value) => {
            diags.push(AgentDiagnostic::InvalidFieldValue {
                field: "model-invocable".to_string(),
                value: format!("{value:?}"),
                allowed: "boolean",
            });
            true
        }
    };

    // approval:
    let approval = fm.get("approval").and_then(Value::as_str).and_then(|s| {
        if let Some(a) = ApprovalMode::from_str(s) {
            Some(a)
        } else {
            diags.push(AgentDiagnostic::InvalidFieldValue {
                field: "approval".to_string(),
                value: s.to_string(),
                allowed: "default, auto, confirm, yolo",
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
        if let Some(e) = EffortLevel::from_str(s) {
            Some(e)
        } else {
            diags.push(AgentDiagnostic::InvalidFieldValue {
                field: "effort".to_string(),
                value: s.to_string(),
                allowed: "low, medium, high, xhigh",
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

    // skills/tools/disallowed-tools/mcp-tools:
    let skills = fm.skills();
    let parsed_tools = fm
        .get("tools")
        .map(|value| parse_tools_field("tools", value, diags))
        .unwrap_or_default();
    let tools = parsed_tools.allowed;
    let tools_denied = parsed_tools.denied;
    let disallowed_tools = fm
        .get("disallowed-tools")
        .map(yaml_tool_list)
        .unwrap_or_default();
    let mcp_tools = fm.get("mcp-tools").map(yaml_str_list).unwrap_or_default();

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
        approval,
        sandbox,
        effort,
        autocompact,
        autocompact_pct,
        skills,
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
mod tests {
    use super::*;
    use crate::frontmatter::Frontmatter;

    fn parse(content: &str) -> (AgentProfile, Vec<AgentDiagnostic>) {
        let fm = Frontmatter::parse(content).unwrap();
        let mut diags = Vec::new();
        let profile = parse_agent_profile(&fm, &mut diags);
        (profile, diags)
    }

    // --- 3.1: Basic field parsing ---

    #[test]
    fn parses_name_and_description() {
        let (p, diags) = parse("---\nname: coder\ndescription: Code agent\n---\n# Body");
        assert!(diags.is_empty());
        assert_eq!(p.name.as_deref(), Some("coder"));
        assert_eq!(p.description.as_deref(), Some("Code agent"));
    }

    #[test]
    fn parses_mode_primary() {
        let (p, diags) = parse("---\nmode: primary\n---\n");
        assert!(diags.is_empty());
        assert_eq!(p.mode, Some(AgentMode::Primary));
    }

    #[test]
    fn parses_mode_subagent() {
        let (p, diags) = parse("---\nmode: subagent\n---\n");
        assert!(diags.is_empty());
        assert_eq!(p.mode, Some(AgentMode::Subagent));
    }

    #[test]
    fn model_invocable_defaults_true() {
        let (p, diags) = parse("---\nmode: subagent\n---\n");
        assert!(diags.is_empty());
        assert!(p.model_invocable);
    }

    #[test]
    fn parses_model_invocable_false() {
        let (p, diags) = parse("---\nmodel-invocable: false\n---\n");
        assert!(diags.is_empty());
        assert!(!p.model_invocable);
    }

    #[test]
    fn invalid_model_invocable_produces_diagnostic() {
        let (p, diags) = parse("---\nmodel-invocable: nope\n---\n");
        assert!(p.model_invocable);
        assert_eq!(diags.len(), 1);
        assert!(
            matches!(&diags[0], AgentDiagnostic::InvalidFieldValue { field, .. } if field == "model-invocable")
        );
    }

    #[test]
    fn invalid_mode_produces_diagnostic() {
        let (p, diags) = parse("---\nmode: invalid\n---\n");
        assert_eq!(p.mode, None);
        assert_eq!(diags.len(), 1);
        assert!(
            matches!(&diags[0], AgentDiagnostic::InvalidFieldValue { field, .. } if field == "mode")
        );
    }

    #[test]
    fn parses_harness_claude() {
        let (p, diags) = parse("---\nharness: claude\n---\n");
        assert!(diags.is_empty());
        assert_eq!(p.harness, Some(HarnessKind::Claude));
    }

    #[test]
    fn parses_harness_codex() {
        let (p, diags) = parse("---\nharness: codex\n---\n");
        assert!(diags.is_empty());
        assert_eq!(p.harness, Some(HarnessKind::Codex));
    }

    #[test]
    fn parses_harness_opencode() {
        let (p, diags) = parse("---\nharness: opencode\n---\n");
        assert!(diags.is_empty());
        assert_eq!(p.harness, Some(HarnessKind::OpenCode));
    }

    #[test]
    fn parses_harness_cursor() {
        let (p, diags) = parse("---\nharness: cursor\n---\n");
        assert!(diags.is_empty());
        assert_eq!(p.harness, Some(HarnessKind::Cursor));
    }

    #[test]
    fn unknown_harness_produces_diagnostic() {
        let (p, diags) = parse("---\nharness: unknown\n---\n");
        assert_eq!(p.harness, None);
        assert_eq!(diags.len(), 1);
        assert!(
            matches!(&diags[0], AgentDiagnostic::UnknownHarness { value } if value == "unknown")
        );
    }

    #[test]
    fn parses_effort_all_values() {
        for (s, expected) in [
            ("low", EffortLevel::Low),
            ("medium", EffortLevel::Medium),
            ("high", EffortLevel::High),
            ("xhigh", EffortLevel::XHigh),
        ] {
            let content = format!("---\neffort: {s}\n---\n");
            let (p, diags) = parse(&content);
            assert!(
                diags.is_empty(),
                "unexpected diags for effort={s}: {diags:?}"
            );
            assert_eq!(p.effort, Some(expected));
        }
    }

    #[test]
    fn parses_approval_all_values() {
        for s in ["default", "auto", "confirm", "yolo"] {
            let content = format!("---\napproval: {s}\n---\n");
            let (p, diags) = parse(&content);
            assert!(diags.is_empty(), "unexpected diags for approval={s}");
            assert!(p.approval.is_some());
        }
    }

    #[test]
    fn parses_sandbox_all_values() {
        for s in [
            "default",
            "read-only",
            "workspace-write",
            "danger-full-access",
        ] {
            let content = format!("---\nsandbox: {s}\n---\n");
            let (p, diags) = parse(&content);
            assert!(diags.is_empty(), "unexpected diags for sandbox={s}");
            assert!(p.sandbox.is_some());
        }
    }

    #[test]
    fn parses_autocompact() {
        let (p, diags) = parse("---\nautocompact: 50\n---\n");
        assert!(diags.is_empty());
        assert_eq!(p.autocompact, Some(50));
    }

    #[test]
    fn parses_autocompact_pct() {
        let (p, diags) = parse("---\nautocompact_pct: 80\n---\n");
        assert!(diags.is_empty());
        assert_eq!(p.autocompact_pct, Some(80));
    }

    #[test]
    fn autocompact_pct_out_of_range() {
        let (p, diags) = parse("---\nautocompact_pct: 101\n---\n");
        assert_eq!(p.autocompact_pct, None);
        assert_eq!(diags.len(), 1);
        assert!(
            matches!(&diags[0], AgentDiagnostic::InvalidFieldValue { field, .. } if field == "autocompact_pct")
        );
    }

    #[test]
    fn autocompact_pct_zero_out_of_range() {
        let (p, diags) = parse("---\nautocompact_pct: 0\n---\n");
        assert_eq!(p.autocompact_pct, None);
        assert_eq!(diags.len(), 1);
        assert!(
            matches!(&diags[0], AgentDiagnostic::InvalidFieldValue { field, .. } if field == "autocompact_pct")
        );
    }

    #[test]
    fn autocompact_pct_in_override() {
        let content = "---\nharness-overrides:\n  claude:\n    autocompact_pct: 75\n---\n";
        let (p, diags) = parse(content);
        assert!(diags.is_empty());
        let claude = p.harness_overrides.claude.as_ref().unwrap();
        assert_eq!(claude.autocompact_pct, Some(75));
    }

    #[test]
    fn autocompact_string_produces_diagnostic() {
        let (p, diags) = parse("---\nautocompact: \"50\"\n---\n");
        assert_eq!(p.autocompact, None);
        assert_eq!(diags.len(), 1);
        assert!(
            matches!(&diags[0], AgentDiagnostic::InvalidFieldValue { field, .. } if field == "autocompact")
        );
    }

    #[test]
    fn autocompact_pct_string_produces_diagnostic() {
        let (p, diags) = parse("---\nautocompact_pct: \"80\"\n---\n");
        assert_eq!(p.autocompact_pct, None);
        assert_eq!(diags.len(), 1);
        assert!(
            matches!(&diags[0], AgentDiagnostic::InvalidFieldValue { field, .. } if field == "autocompact_pct")
        );
    }

    #[test]
    fn parses_skills_tools_disallowed_mcp() {
        let content = "---\nskills: [review, dev-principles]\ntools: [Bash, Write]\ndisallowed-tools: [Agent]\nmcp-tools: [server]\n---\n";
        let (p, diags) = parse(content);
        assert!(diags.is_empty());
        assert_eq!(p.skills, vec!["review", "dev-principles"]);
        assert_eq!(p.tools, vec!["Bash", "Write"]);
        assert!(p.tools_denied.is_empty());
        assert_eq!(p.disallowed_tools, vec!["Agent"]);
        assert_eq!(p.mcp_tools, vec!["server"]);
    }

    #[test]
    fn parses_tools_map_allow_and_deny_with_name_normalization() {
        let content =
            "---\ntools:\n  bash: allow\n  \"bash(meridian spawn *)\": allow\n  agent: deny\n---\n";
        let (p, diags) = parse(content);
        assert!(diags.is_empty());
        assert_eq!(p.tools, vec!["Bash", "Bash(meridian spawn *)"]);
        assert_eq!(p.tools_denied, vec!["Agent"]);
    }

    #[test]
    fn effective_tool_policy_uses_harness_override_replacements() {
        let content = "---\ntools:\n  bash: allow\n  read: deny\ndisallowed-tools: [Edit]\nmcp-tools: [plugin:base]\nharness-overrides:\n  codex:\n    tools:\n      \"bash(meridian spawn *)\": allow\n      agent: deny\n    disallowed-tools: [Write]\n    mcp-tools: [plugin:codex]\n---\n";
        let (p, diags) = parse(content);
        assert!(diags.is_empty());

        let codex_policy = p.effective_tool_policy(&HarnessKind::Codex);
        assert_eq!(codex_policy.allowed, vec!["Bash(meridian spawn *)"]);
        assert_eq!(codex_policy.disallowed, vec!["Agent", "Write"]);
        assert_eq!(codex_policy.mcp, vec!["plugin:codex"]);

        let claude_policy = p.effective_tool_policy(&HarnessKind::Claude);
        assert_eq!(claude_policy.allowed, vec!["Bash"]);
        assert_eq!(claude_policy.disallowed, vec!["Read", "Edit"]);
        assert_eq!(claude_policy.mcp, vec!["plugin:base"]);
    }

    #[test]
    fn effective_skills_use_harness_override_replacement() {
        let content =
            "---\nskills: [base]\nharness-overrides:\n  codex:\n    skills: [codex-only]\n---\n";
        let (p, diags) = parse(content);
        assert!(diags.is_empty());

        assert_eq!(
            p.effective_skills(&HarnessKind::Codex),
            &vec!["codex-only".to_string()]
        );
        assert_eq!(
            p.effective_skills(&HarnessKind::Claude),
            &vec!["base".to_string()]
        );
    }

    #[test]
    fn effective_native_config_uses_matching_harness_override() {
        let content = "---\nharness-overrides:\n  claude:\n    native-config:\n      ui.theme: dark\n  codex:\n    native-config:\n      sandbox_workspace_write.network_access: true\n---\n";
        let (p, diags) = parse(content);
        assert!(diags.is_empty());

        assert_eq!(
            p.effective_native_config(&HarnessKind::Codex)
                .expect("codex native config"),
            &serde_json::Map::from_iter([(
                "sandbox_workspace_write.network_access".to_string(),
                serde_json::json!(true)
            )])
        );
        assert_eq!(
            p.effective_native_config(&HarnessKind::Claude)
                .expect("claude native config"),
            &serde_json::Map::from_iter([("ui.theme".to_string(), serde_json::json!("dark"))])
        );
        assert!(p.effective_native_config(&HarnessKind::OpenCode).is_none());
    }

    // --- 3.1: model-policies ---

    #[test]
    fn model_policies_are_parsed_as_raw_entries() {
        let content = "---\nmodel-policies:\n  - match:\n      model: gpt-5.5\n    override:\n      harness: codex\n---\n";
        let (p, diags) = parse(content);
        assert!(diags.is_empty());
        assert_eq!(p.model_policies.len(), 1);
        assert_eq!(p.model_policies[0].match_type, ModelPolicyMatchType::Model);
        assert_eq!(p.model_policies[0].match_value, "gpt-5.5");
        assert!(p.model_policies[0].overrides.contains_key("harness"));
    }

    #[test]
    fn model_policy_empty_override_is_valid_for_fallback_candidate() {
        let content =
            "---\nmodel-policies:\n  - match:\n      alias: gpt55\n    override: {}\n---\n";
        let (p, diags) = parse(content);
        assert!(diags.is_empty());
        assert_eq!(p.model_policies.len(), 1);
        assert!(p.model_policies[0].overrides.is_empty());
    }

    #[test]
    fn model_policy_empty_override_is_valid_for_no_fallback_rule() {
        let content = "---\nmodel-policies:\n  - match:\n      alias: gpt55\n    no-fallback: true\n    override: {}\n---\n";
        let (p, diags) = parse(content);
        assert!(diags.is_empty());
        assert_eq!(p.model_policies.len(), 1);
        assert!(p.model_policies[0].no_fallback);
        assert!(p.model_policies[0].overrides.is_empty());
    }

    #[test]
    fn model_policy_missing_override_is_valid() {
        let content = "---\nmodel-policies:\n  - match:\n      alias: gpt55\n---\n";
        let (p, diags) = parse(content);
        assert!(diags.is_empty());
        assert_eq!(p.model_policies.len(), 1);
        assert!(p.model_policies[0].overrides.is_empty());
    }

    #[test]
    fn malformed_model_policy_produces_diagnostic() {
        let content = "---\nmodel-policies:\n  - match:\n      model: gpt-5.5\n      alias: gpt55\n    override:\n      harness: codex\n---\n";
        let (p, diags) = parse(content);
        assert!(p.model_policies.is_empty());
        assert_eq!(diags.len(), 1);
        assert!(
            matches!(&diags[0], AgentDiagnostic::InvalidFieldValue { field, .. } if field == "model-policies[1].match")
        );
    }

    // --- 3.1: fanout ---

    #[test]
    fn fanout_entries_are_parsed_as_raw() {
        let content = "---\nfanout:\n  - alias: opus\n  - model: gpt-5.5\n---\n";
        let (p, diags) = parse(content);
        assert!(diags.is_empty());
        assert_eq!(p.fanout.len(), 2);
    }

    // --- 3.1: harness-overrides ---

    #[test]
    fn harness_overrides_parsed_for_claude_and_codex() {
        let content = "---\nharness-overrides:\n  claude:\n    approval: auto\n  codex:\n    sandbox: workspace-write\n    effort: high\n---\n";
        let (p, diags) = parse(content);
        assert!(diags.is_empty());
        let claude = p.harness_overrides.claude.as_ref().unwrap();
        assert_eq!(claude.approval, Some(ApprovalMode::Auto));
        let codex = p.harness_overrides.codex.as_ref().unwrap();
        assert_eq!(codex.sandbox, Some(SandboxMode::WorkspaceWrite));
        assert_eq!(codex.effort, Some(EffortLevel::High));
    }

    #[test]
    fn harness_override_native_config_parses_shape_only() {
        let content = "---\nharness-overrides:\n  codex:\n    native-config:\n      sandbox_workspace_write.network_access: true\n      limits:\n        max_tokens: 4096\n---\n";
        let (p, diags) = parse(content);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        let codex = p.harness_overrides.codex.as_ref().unwrap();
        let native_config = codex.native_config.as_ref().unwrap();
        assert_eq!(
            native_config["sandbox_workspace_write.network_access"],
            serde_json::json!(true)
        );
        assert_eq!(
            native_config["limits"],
            serde_json::json!({"max_tokens": 4096})
        );
    }

    #[test]
    fn harness_override_native_config_accepts_arrays_and_rejects_null_values() {
        let valid_content = "---\nharness-overrides:\n  codex:\n    native-config:\n      allowlist: [Bash, Read]\n      nested:\n        values: [1, 2]\n---\n";
        let (valid_profile, valid_diags) = parse(valid_content);
        assert!(
            valid_diags.is_empty(),
            "unexpected diagnostics: {valid_diags:?}"
        );
        let valid_native_config = valid_profile
            .harness_overrides
            .codex
            .as_ref()
            .unwrap()
            .native_config
            .as_ref()
            .unwrap();
        assert_eq!(
            valid_native_config["allowlist"],
            serde_json::json!(["Bash", "Read"])
        );
        assert_eq!(
            valid_native_config["nested"],
            serde_json::json!({"values": [1, 2]})
        );

        let null_content =
            "---\nharness-overrides:\n  codex:\n    native-config:\n      maybe_null: null\n---\n";
        let (null_profile, null_diags) = parse(null_content);
        let codex = null_profile.harness_overrides.codex.as_ref().unwrap();
        assert!(
            codex.native_config.is_none(),
            "native-config with a null value should be rejected"
        );
        assert!(
            null_diags.iter().any(|diag| {
                matches!(
                    diag,
                    AgentDiagnostic::InvalidFieldValue { field, .. }
                        if field == "harness-overrides.codex.native-config.maybe_null"
                )
            }),
            "missing nested null diagnostic: {null_diags:?}"
        );
    }

    #[test]
    fn harness_override_native_config_invalid_shape_produces_diagnostic() {
        let content = "---\nharness-overrides:\n  codex:\n    native-config: [1, 2]\n---\n";
        let (p, diags) = parse(content);
        let codex = p.harness_overrides.codex.as_ref().unwrap();
        assert!(codex.native_config.is_none());
        assert!(
            diags.iter().any(|diag| {
                matches!(diag, AgentDiagnostic::InvalidFieldValue { field, .. } if field == "harness-overrides.codex.native-config")
            }),
            "missing native-config invalid shape diagnostic: {diags:?}"
        );
    }

    #[test]
    fn harness_override_native_config_portable_key_collision_warns() {
        let content =
            "---\nharness-overrides:\n  codex:\n    native-config:\n      sandbox: true\n---\n";
        let (_p, diags) = parse(content);
        assert!(
            diags.iter().any(|diag| {
                matches!(diag, AgentDiagnostic::NativeConfigPortableKeyCollision { key, .. } if key == "sandbox")
            }),
            "expected portable key collision warning: {diags:?}"
        );
    }

    #[test]
    fn harness_override_with_non_overridable_field_produces_diagnostic() {
        let content = "---\nharness-overrides:\n  claude:\n    name: bad\n---\n";
        let (_p, diags) = parse(content);
        assert_eq!(diags.len(), 1);
        assert!(
            matches!(&diags[0], AgentDiagnostic::NonOverridableFieldInOverride { field, .. } if field == "name")
        );
    }

    // --- 3.1: legacy models field ---

    #[test]
    fn legacy_models_field_produces_deprecation_warning() {
        let content = "---\nmodels:\n  opus:\n    effort: high\n---\n";
        let (_p, diags) = parse(content);
        assert_eq!(diags.len(), 1);
        assert!(matches!(&diags[0], AgentDiagnostic::LegacyModelsField));
    }

    // --- Empty agent ---

    #[test]
    fn empty_agent_has_no_diagnostics() {
        let (p, diags) = parse("# Minimal agent\nno frontmatter");
        assert!(diags.is_empty());
        assert!(p.name.is_none());
        assert!(p.harness.is_none());
    }

    #[test]
    fn agent_without_harness_is_universal() {
        let (p, _) = parse("---\nname: planner\nmodel: gpt55\n---\n# Planner");
        assert!(p.harness.is_none());
    }
}
