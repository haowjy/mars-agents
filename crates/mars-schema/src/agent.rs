//! Agent definition reader.

use std::collections::BTreeMap;

use crate::HarnessId;
use serde_yaml::Value;

use crate::frontmatter::{Frontmatter, FrontmatterError, SkillsSpec};
use crate::invocability::parse_invocability_axis;
use crate::model_policy::ModelPolicyRule;
use crate::tool_policy::{self, EffectiveToolPolicy};

const REMOVED_AGENT_FIELDS: &[(&str, &str)] = &[
    ("disallowed-tools", "use `tools: {<capability>: deny}`"),
    ("fanout", "use consumer `[settings.meridian.fanout]`"),
    ("models", "use `model-policies`"),
    ("mcp-tools", "put MCP capability references in `tools`"),
    ("model_invocable", "use `model-invocable`"),
    ("user_invocable", "use `user-invocable`"),
    ("network", "remove this unenforced field"),
    ("network-allow", "remove this unenforced field"),
    ("network-deny", "remove this unenforced field"),
    ("autocompact", "configure compaction on the model alias"),
    ("autocompact_pct", "configure compaction on the model alias"),
    (
        "permission",
        "move it under `harness-overrides.flow.permission`",
    ),
];

// ---------------------------------------------------------------------------
// Field enums
// ---------------------------------------------------------------------------

/// Agent execution mode — how the agent is launched.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
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

/// Approval policy field.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalMode {
    Default,
    Auto,
    Confirm,
    Never,
}

impl ApprovalMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "default" => Some(Self::Default),
            "auto" => Some(Self::Auto),
            "confirm" => Some(Self::Confirm),
            "never" => Some(Self::Never),
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    Default,
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

impl SandboxMode {
    pub fn parse(s: &str) -> Option<Self> {
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EffortLevel {
    None,
    Low,
    Medium,
    High,
    #[serde(rename = "xhigh")]
    XHigh,
}

impl EffortLevel {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" | "max" => Some(Self::XHigh),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::None => "none",
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
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(transparent)]
pub struct HarnessOverrides {
    pub entries: BTreeMap<String, serde_json::Map<String, serde_json::Value>>,
}

fn mars_schema_harness_known(value: &str) -> Option<HarnessId> {
    crate::harness::parse(value)
}

fn harness_key(harness: &HarnessId) -> &'static str {
    harness.as_str()
}

impl HarnessOverrides {
    pub fn get(&self, harness: &HarnessId) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.entries.get(harness_key(harness))
    }
}

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
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct AgentProfile {
    // --- Identity fields ---
    pub name: Option<String>,
    pub description: Option<String>,

    // --- Routing fields ---
    pub harness: Option<HarnessId>,

    // --- Model fields ---
    pub model: Option<String>,

    // --- Runtime policy fields ---
    pub mode: Option<AgentMode>,
    pub model_invocable: bool,
    /// Whether the user can invoke this agent directly (slash command, picker, etc.).
    pub user_invocable: bool,
    /// `true` when source frontmatter explicitly set `model-invocable`.
    #[serde(skip)]
    pub had_model_invocable_field: bool,
    /// `true` when source frontmatter explicitly set `user-invocable`.
    #[serde(skip)]
    pub had_user_invocable_field: bool,
    pub approval: Option<ApprovalMode>,
    pub sandbox: Option<SandboxMode>,
    pub effort: Option<EffortLevel>,

    // --- Tool fields ---
    pub skills: SkillsSpec,
    pub subagents: Vec<String>,
    /// `tools:` and `disallowed-tools:` as one rule list. An allowlist has an explicit
    /// `'*': deny`; deprecated list form only grants its entries.
    pub tools: EffectiveToolPolicy,

    // --- Override tables ---
    pub harness_overrides: HarnessOverrides,
    pub model_policies: Vec<ModelPolicyRule>,
}

impl AgentProfile {
    pub fn effective_native_config(
        &self,
        harness: &HarnessId,
    ) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.harness_overrides
            .get(harness)
            .filter(|map| !map.is_empty())
    }
}
// ---------------------------------------------------------------------------
// Validation warnings/errors
// ---------------------------------------------------------------------------

fn push_agent_invalid(
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
    path: Vec<crate::diagnostic::PathSegment>,
    value: impl Into<String>,
    allowed: &'static str,
) {
    use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity, display_path};
    let value = value.into();
    let severity = if matches!(path.first(), Some(PathSegment::Key(key)) if key == "harness-overrides")
    {
        Severity::Warning
    } else {
        Severity::Error
    };
    let display_field = if matches!(path.first(), Some(PathSegment::Key(key)) if key == "model-policies")
    {
        path.iter()
            .map(|segment| match segment {
                PathSegment::Key(key) => key.clone(),
                PathSegment::Index(index) => format!("[{}]", index + 1),
            })
            .fold(String::new(), |mut display, segment| {
                if !display.is_empty() && !segment.starts_with('[') {
                    display.push('.');
                }
                display.push_str(&segment);
                display
            })
    } else {
        display_path(&path)
    };
    diags.push(SchemaDiagnostic::new(
        "field.value",
        severity,
        path,
        format!("agent field `{display_field}` has invalid value `{value}`; allowed: {allowed}"),
    ));
}

fn push_unknown_harness(diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>, value: &str) {
    use crate::diagnostic::{SchemaDiagnostic, Severity, key_path};
    diags.push(SchemaDiagnostic::new(
        "harness.unknown",
        Severity::Error,
        key_path("harness"),
        format!("unknown harness `{value}`; known: claude, codex, opencode, cursor, pi"),
    ));
}

fn push_unknown_harness_override(
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
    value: &str,
) {
    use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity, key_path};
    let mut path = key_path("harness-overrides");
    path.push(PathSegment::Key(value.to_owned()));
    diags.push(SchemaDiagnostic::new(
        "harness.unknown",
        Severity::Warning,
        path,
        format!("unknown harness override `{value}`; preserving passthrough block"),
    ));
}

fn push_agent_tool_invalid(
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
    path: Vec<crate::diagnostic::PathSegment>,
    value: &str,
    allowed: &'static str,
) {
    push_agent_invalid(diags, path, value, allowed);
}

fn parse_native_config_value(
    path: &[crate::diagnostic::PathSegment],
    value: &Value,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) -> Option<serde_json::Value> {
    match value {
        Value::Null => {
            push_agent_invalid(
                diags,
                path.to_vec(),
                "null".to_string(),
                "non-null scalar, array, or map value",
            );
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
                        push_agent_invalid(
                            diags,
                            path.to_vec(),
                            float.to_string(),
                            "finite JSON number",
                        );
                        None
                    }
                }
            } else {
                push_agent_invalid(diags, path.to_vec(), format!("{value:?}"), "JSON number");
                None
            }
        }
        Value::Sequence(seq) => {
            let mut out = Vec::with_capacity(seq.len());
            for (index, entry) in seq.iter().enumerate() {
                let mut child_path = path.to_vec();
                child_path.push(crate::diagnostic::PathSegment::Index(index));
                if let Some(parsed) = parse_native_config_value(&child_path, entry, diags) {
                    out.push(parsed);
                }
            }
            Some(serde_json::Value::Array(out))
        }
        Value::Mapping(mapping) => {
            let mut out = serde_json::Map::new();
            for (key, entry) in mapping {
                let Some(key_text) = key.as_str() else {
                    push_agent_invalid(diags, path.to_vec(), format!("{key:?}"), "string keys");
                    continue;
                };
                let mut child_path = path.to_vec();
                child_path.push(crate::diagnostic::PathSegment::Key(key_text.to_owned()));
                if let Some(parsed) = parse_native_config_value(&child_path, entry, diags) {
                    out.insert(key_text.to_string(), parsed);
                }
            }
            Some(serde_json::Value::Object(out))
        }
        _ => {
            push_agent_invalid(
                diags,
                path.to_vec(),
                format!("{value:?}"),
                "YAML/TOML/JSON-serializable value",
            );
            None
        }
    }
}

fn parse_harness_overrides(
    val: &Value,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) -> HarnessOverrides {
    use crate::diagnostic::{PathSegment, key_path};
    let root = key_path("harness-overrides");
    let mut out = HarnessOverrides::default();
    let Some(mapping) = val.as_mapping() else {
        push_agent_invalid(
            diags,
            root.clone(),
            format!("{val:?}"),
            "mapping of harness name to target-native config mapping",
        );
        return out;
    };

    for (k, v) in mapping {
        let Some(harness_name) = k.as_str() else {
            push_agent_invalid(diags, root.clone(), format!("{k:?}"), "string harness keys");
            continue;
        };
        let mut harness_path = root.clone();
        harness_path.push(PathSegment::Key(harness_name.to_owned()));
        let Some(sub_mapping) = v.as_mapping() else {
            push_agent_invalid(
                diags,
                harness_path,
                format!("{v:?}"),
                "mapping of target-native keys",
            );
            continue;
        };
        if harness_name != "flow" && mars_schema_harness_known(harness_name).is_none() {
            push_unknown_harness_override(diags, harness_name);
        }
        let mut parsed = serde_json::Map::new();
        for (target_key, target_value) in sub_mapping {
            let Some(key_text) = target_key.as_str() else {
                push_agent_invalid(
                    diags,
                    harness_path.clone(),
                    format!("{target_key:?}"),
                    "string target-native keys",
                );
                continue;
            };
            let mut value_path = harness_path.clone();
            value_path.push(PathSegment::Key(key_text.to_owned()));
            if let Some(value) = parse_native_config_value(&value_path, target_value, diags) {
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
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
    path: Vec<crate::diagnostic::PathSegment>,
    value: impl Into<String>,
    allowed: &'static str,
) {
    push_agent_invalid(diags, path, value, allowed);
}

fn push_model_policy_parse_error(
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
    position: usize,
    error: crate::model_policy::ModelPolicyRuleParseError,
) {
    use crate::diagnostic::{PathSegment, key_path};
    let mut rule_path = key_path("model-policies");
    rule_path.push(PathSegment::Index(position));
    let child = |key: &str| {
        let mut path = rule_path.clone();
        path.push(PathSegment::Key(key.to_owned()));
        path
    };
    let grandchild = |first: &str, second: &str| {
        let mut path = child(first);
        path.push(PathSegment::Key(second.to_owned()));
        path
    };
    match error {
        crate::model_policy::ModelPolicyRuleParseError::RuleMustBeMapping { found } => {
            push_model_policy_invalid(diags, rule_path, found, "mapping with match and override");
        }
        crate::model_policy::ModelPolicyRuleParseError::MatchMissing => {
            push_model_policy_invalid(
                diags,
                child("match"),
                "<missing>",
                "mapping with exactly one of model, alias, model-glob",
            );
        }
        crate::model_policy::ModelPolicyRuleParseError::MatchMustBeMapping { found } => {
            push_model_policy_invalid(
                diags,
                child("match"),
                found,
                "mapping with exactly one of model, alias, model-glob",
            );
        }
        crate::model_policy::ModelPolicyRuleParseError::MatchMustContainExactlyOne { found } => {
            push_model_policy_invalid(
                diags,
                child("match"),
                found,
                "exactly one of model, alias, model-glob",
            );
        }
        crate::model_policy::ModelPolicyRuleParseError::MatchKeyMustBeString { found } => {
            push_model_policy_invalid(diags, child("match"), found, "model, alias, model-glob");
        }
        crate::model_policy::ModelPolicyRuleParseError::UnknownMatchKey { key } => {
            push_model_policy_invalid(diags, child("match"), key, "model, alias, model-glob");
        }
        crate::model_policy::ModelPolicyRuleParseError::MatchValueMustBeString { key, found } => {
            push_model_policy_invalid(diags, grandchild("match", &key), found, "non-empty string");
        }
        crate::model_policy::ModelPolicyRuleParseError::MatchValueEmpty { key } => {
            push_model_policy_invalid(
                diags,
                grandchild("match", &key),
                "<empty>",
                "non-empty string",
            );
        }
        crate::model_policy::ModelPolicyRuleParseError::OverrideMustBeMapping { found } => {
            push_model_policy_invalid(diags, child("override"), found, "mapping");
        }
        crate::model_policy::ModelPolicyRuleParseError::UnknownOverrideKey { key } => {
            push_model_policy_invalid(
                diags,
                grandchild("override", &key),
                key,
                "effort, sandbox, approval",
            );
        }
        crate::model_policy::ModelPolicyRuleParseError::OverrideValueInvalid {
            key,
            found,
            allowed,
        } => {
            push_model_policy_invalid(diags, grandchild("override", &key), found, allowed);
        }
        crate::model_policy::ModelPolicyRuleParseError::NoFallbackMustBeBoolean { found } => {
            push_model_policy_invalid(diags, child("no-fallback"), found, "boolean");
        }
    }
}

fn parse_model_policies(
    val: &Value,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) -> Vec<ModelPolicyRule> {
    let Some(seq) = val.as_sequence() else {
        push_model_policy_invalid(
            diags,
            crate::diagnostic::key_path("model-policies"),
            format!("{val:?}"),
            "sequence of rules",
        );
        return vec![];
    };

    let mut out = Vec::new();
    for (index, entry) in seq.iter().enumerate() {
        match crate::model_policy::parse_model_policy_rule_value(entry) {
            Ok(rule) => out.push(rule),
            Err(error) => push_model_policy_parse_error(diags, index, error),
        }
    }
    out
}

fn parse_invocability_field(
    field: &str,
    raw: Option<&Value>,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) -> (bool, bool) {
    let (value, had_field, invalid) = parse_invocability_axis(raw);
    if let Some(invalid) = invalid {
        push_agent_invalid(
            diags,
            crate::diagnostic::key_path(field),
            invalid,
            "boolean",
        );
    }
    (value, had_field)
}

fn read_string(
    value: &Value,
    field: &'static str,
    non_empty: bool,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) -> Option<String> {
    let Some(value) = value.as_str() else {
        diags.push(crate::diagnostic::SchemaDiagnostic::new(
            "field.type",
            crate::diagnostic::Severity::Error,
            crate::diagnostic::key_path(field),
            format!("`{field}` must be a string"),
        ));
        return None;
    };
    if non_empty {
        let value = value.trim();
        if value.is_empty() {
            diags.push(crate::diagnostic::SchemaDiagnostic::new(
                "field.value",
                crate::diagnostic::Severity::Error,
                crate::diagnostic::key_path(field),
                format!("`{field}` must be a non-empty string"),
            ));
            return None;
        }
        return Some(value.to_owned());
    }
    Some(value.to_owned())
}

fn read_string_list(
    value: Option<&Value>,
    path: Vec<crate::diagnostic::PathSegment>,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) -> Vec<String> {
    use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity, display_path};
    let Some(value) = value else {
        return Vec::new();
    };
    let Some(values) = value.as_sequence() else {
        let display = display_path(&path);
        diags.push(SchemaDiagnostic::new(
            "field.type",
            Severity::Error,
            path,
            format!("`{display}` must be a list of non-empty strings"),
        ));
        return Vec::new();
    };
    let display = display_path(&path);
    values
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            if let Value::String(text) = value
                && !text.trim().is_empty()
            {
                return Some(text.clone());
            }
            let mut entry_path = path.clone();
            entry_path.push(PathSegment::Index(index));
            diags.push(SchemaDiagnostic::new(
                if value.is_string() {
                    "field.value"
                } else {
                    "field.type"
                },
                Severity::Error,
                entry_path,
                format!("`{display}` entry {} must be a non-empty string", index + 1),
            ));
            None
        })
        .collect()
}

fn read_skills(value: &Value, diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>) -> SkillsSpec {
    use crate::diagnostic::{PathSegment, key_path};
    let root = key_path("skills");
    let Value::Mapping(mapping) = value else {
        return SkillsSpec {
            load: read_string_list(Some(value), root, diags),
            available: Vec::new(),
        };
    };
    let read_axis = |name: &str, diags: &mut Vec<_>| {
        let mut path = root.clone();
        path.push(PathSegment::Key(name.to_owned()));
        read_string_list(mapping.get(Value::String(name.to_owned())), path, diags)
    };
    SkillsSpec {
        load: read_axis("load", diags),
        available: read_axis("available", diags),
    }
}

fn parse_agent_profile_inner(
    fm: &Frontmatter,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) -> AgentProfile {
    use crate::diagnostic::{SchemaDiagnostic, Severity, key_path};

    let mut profile = AgentProfile {
        name: None,
        description: None,
        harness: None,
        model: None,
        mode: None,
        model_invocable: true,
        user_invocable: true,
        had_model_invocable_field: false,
        had_user_invocable_field: false,
        approval: None,
        sandbox: None,
        effort: None,
        skills: SkillsSpec::default(),
        subagents: Vec::new(),
        tools: EffectiveToolPolicy::default(),
        harness_overrides: HarnessOverrides::default(),
        model_policies: Vec::new(),
    };

    for key in fm.keys() {
        let value = fm.get(&key).expect("frontmatter key came from mapping");
        match key.as_str() {
            "name" => profile.name = read_string(value, "name", true, diags),
            "description" => {
                profile.description = read_string(value, "description", false, diags);
            }
            "harness" => {
                profile.harness = read_string(value, "harness", false, diags).and_then(|name| {
                    crate::harness::parse(&name).or_else(|| {
                        push_unknown_harness(diags, &name);
                        None
                    })
                });
            }
            "model" => profile.model = read_string(value, "model", true, diags),
            "mode" => {
                profile.mode =
                    read_string(value, "mode", false, diags).and_then(|mode| match mode.as_str() {
                        "primary" => Some(AgentMode::Primary),
                        "subagent" => Some(AgentMode::Subagent),
                        other => {
                            push_agent_invalid(
                                diags,
                                key_path("mode"),
                                other.to_string(),
                                "primary, subagent",
                            );
                            None
                        }
                    });
            }
            "model-invocable" => {
                (profile.model_invocable, profile.had_model_invocable_field) =
                    parse_invocability_field("model-invocable", Some(value), diags);
            }
            "user-invocable" => {
                (profile.user_invocable, profile.had_user_invocable_field) =
                    parse_invocability_field("user-invocable", Some(value), diags);
            }
            "approval" => {
                profile.approval =
                    read_string(value, "approval", false, diags).and_then(|approval| {
                        if approval == "yolo" {
                            diags.push(SchemaDiagnostic::new(
                                "field.removed",
                                Severity::Error,
                                key_path("approval"),
                                "approval value `yolo` was removed; use `never`",
                            ));
                        }
                        ApprovalMode::parse(&approval).or_else(|| {
                            push_agent_invalid(
                                diags,
                                key_path("approval"),
                                approval,
                                "default, auto, confirm, never",
                            );
                            None
                        })
                    });
            }
            "sandbox" => {
                profile.sandbox = read_string(value, "sandbox", false, diags).and_then(|sandbox| {
                    SandboxMode::parse(&sandbox).or_else(|| {
                        push_agent_invalid(
                            diags,
                            key_path("sandbox"),
                            sandbox,
                            "default, read-only, workspace-write, danger-full-access",
                        );
                        None
                    })
                });
            }
            "effort" => {
                profile.effort = read_string(value, "effort", false, diags).and_then(|effort| {
                    EffortLevel::parse(&effort).or_else(|| {
                        push_agent_invalid(
                            diags,
                            key_path("effort"),
                            effort,
                            "low, medium, high, xhigh, none",
                        );
                        None
                    })
                });
            }
            "skills" => profile.skills = read_skills(value, diags),
            "subagents" => {
                profile.subagents = read_string_list(Some(value), key_path("subagents"), diags);
            }
            "tools" => {
                if value.is_sequence() {
                    diags.push(SchemaDiagnostic::new(
                        "tools.list-form",
                        Severity::Error,
                        key_path("tools"),
                        "`tools` must be an action map; list form was removed",
                    ));
                } else {
                    let mut invalid = |path, value: &str, allowed: &'static str| {
                        push_agent_tool_invalid(diags, path, value, allowed);
                    };
                    profile.tools = tool_policy::parse_tool_policy(Some(value), &mut invalid);
                }
            }
            "harness-overrides" => {
                profile.harness_overrides = parse_harness_overrides(value, diags);
            }
            "model-policies" => profile.model_policies = parse_model_policies(value, diags),
            _ => {
                if let Some((_, replacement)) = REMOVED_AGENT_FIELDS
                    .iter()
                    .find(|(removed, _)| *removed == key)
                {
                    diags.push(SchemaDiagnostic::new(
                        "field.removed",
                        Severity::Error,
                        key_path(&key),
                        format!("`{key}` was removed; {replacement}"),
                    ));
                } else {
                    diags.push(SchemaDiagnostic::new(
                        "field.unknown",
                        Severity::Warning,
                        key_path(&key),
                        format!("unknown field `{key}` is preserved"),
                    ));
                }
            }
        }
    }

    profile
}

pub fn parse_agent_profile(
    fm: &Frontmatter,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) -> AgentProfile {
    parse_agent_profile_inner(fm, diags)
}

pub fn parse_agent_content(
    content: &str,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) -> Result<(AgentProfile, Frontmatter), FrontmatterError> {
    let fm = Frontmatter::parse(content)?;
    let profile = parse_agent_profile(&fm, diags);
    Ok((profile, fm))
}

/// Read an Agent definition through the versioned schema contract.
pub fn read(content: &str) -> crate::read::DefinitionRead<AgentProfile> {
    let mut diagnostics = Vec::new();
    match Frontmatter::parse_preserving_structure(content) {
        Ok((frontmatter, structural_errors)) => {
            diagnostics.extend(
                structural_errors
                    .iter()
                    .map(crate::diagnostic::frontmatter_error),
            );
            let fields = parse_agent_profile_inner(&frontmatter, &mut diagnostics);
            crate::read::DefinitionRead {
                frontmatter: crate::read::frontmatter_json(frontmatter.mapping(), &mut diagnostics),
                fields: Some(fields),
                body: Some(frontmatter.body().to_string()),
                diagnostics,
            }
        }
        Err(error) => crate::read::DefinitionRead {
            frontmatter: Vec::new(),
            fields: None,
            body: None,
            diagnostics: vec![crate::diagnostic::frontmatter_error(&error)],
        },
    }
}
