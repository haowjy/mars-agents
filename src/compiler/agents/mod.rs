/// Agent-profile schema, parser, and validation.
///
/// Parses agent markdown frontmatter into strongly-typed [`AgentProfile`] fields.
/// Used by the dual-surface compilation pipeline to:
/// - Validate agent profiles at compile time
/// - Route agents to the correct harness-native output surface
/// - Report lossiness diagnostics when fields cannot be expressed in a target format
pub mod lower;

use serde_yaml::Value;

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
    pub skills: Option<SkillsSpec>,
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

/// Portable tool policy after applying harness override replacement semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveToolPolicy {
    pub allowed: Vec<String>,
    pub disallowed: Vec<String>,
    pub mcp: Vec<String>,
}

impl AgentProfile {
    pub fn effective_skills(&self, harness: &HarnessKind) -> &SkillsSpec {
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
    /// Deprecated `approval: yolo` was found (use `approval: never` instead).
    DeprecatedApprovalYolo,
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
            AgentDiagnostic::DeprecatedApprovalYolo => {
                "agent uses deprecated `approval: yolo`; use `approval: never` instead".to_string()
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

fn parse_skills_spec(val: &Value) -> SkillsSpec {
    match val {
        Value::Mapping(mapping) => SkillsSpec {
            load: mapping
                .get(Value::String("load".to_string()))
                .map(yaml_str_list)
                .unwrap_or_default(),
            available: mapping
                .get(Value::String("available".to_string()))
                .map(yaml_str_list)
                .unwrap_or_default(),
        },
        _ => SkillsSpec {
            load: yaml_str_list(val),
            available: Vec::new(),
        },
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
                    if s == "none" {
                        // "none" is a valid sentinel meaning "no effort level" (leave out.effort as None)
                    } else if let Some(e) = EffortLevel::from_str(s) {
                        out.effort = Some(e);
                    } else {
                        diags.push(AgentDiagnostic::InvalidFieldValue {
                            field: format!("{table_name}.effort"),
                            value: s.to_string(),
                            allowed: "low, medium, high, xhigh, none",
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
                        if s == "yolo" {
                            diags.push(AgentDiagnostic::DeprecatedApprovalYolo);
                        }
                        out.approval = Some(a);
                    } else {
                        diags.push(AgentDiagnostic::InvalidFieldValue {
                            field: format!("{table_name}.approval"),
                            value: s.to_string(),
                            allowed: "default, auto, confirm, never",
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
                out.skills = Some(parse_skills_spec(v));
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
    let subagents = fm.get("subagents").map(yaml_str_list).unwrap_or_default();
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
