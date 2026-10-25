//! Agent definition reader.

use std::collections::BTreeMap;

use crate::HarnessId;
use serde_yaml::Value;

use crate::frontmatter::{Frontmatter, FrontmatterError, SkillsSpec};
use crate::invocability::parse_invocability_axis;
use crate::model_policy::ModelPolicyRule;
use crate::tool_policy::{self, EffectiveToolPolicy};

// ---------------------------------------------------------------------------
// Field enums
// ---------------------------------------------------------------------------

/// Agent execution mode — how the agent is launched.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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
pub enum EffortLevel {
    None,
    Low,
    Medium,
    High,
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
    pub had_model_invocable_field: bool,
    /// `true` when source frontmatter explicitly set `user-invocable`.
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
    pub fn effective_skills(&self, _harness: &HarnessId) -> &SkillsSpec {
        &self.skills
    }

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
    field: impl Into<String>,
    value: impl Into<String>,
    allowed: &'static str,
) {
    use crate::diagnostic::{SchemaDiagnostic, Severity, key_path};
    let field = field.into();
    let value = value.into();
    let severity = if field.starts_with("harness-overrides") {
        Severity::Warning
    } else {
        Severity::Error
    };
    diags.push(SchemaDiagnostic::new(
        "field.value",
        severity,
        key_path(&field),
        format!("agent field `{field}` has invalid value `{value}`; allowed: {allowed}"),
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
    use crate::diagnostic::{SchemaDiagnostic, Severity, key_path};
    diags.push(SchemaDiagnostic::new(
        "harness.unknown",
        Severity::Warning,
        key_path(&format!("harness-overrides.{value}")),
        format!("unknown harness override `{value}`; preserving passthrough block"),
    ));
}

fn push_agent_tool_invalid(
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
    field: &str,
    value: &str,
    allowed: &'static str,
) {
    push_agent_invalid(diags, field, value, allowed);
}

fn parse_native_config_value(
    field: &str,
    value: &Value,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) -> Option<serde_json::Value> {
    match value {
        Value::Null => {
            push_agent_invalid(
                diags,
                field.to_string(),
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
                            field.to_string(),
                            float.to_string(),
                            "finite JSON number",
                        );
                        None
                    }
                }
            } else {
                push_agent_invalid(
                    diags,
                    field.to_string(),
                    format!("{value:?}"),
                    "JSON number",
                );
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
                    push_agent_invalid(diags, field.to_string(), format!("{key:?}"), "string keys");
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
            push_agent_invalid(
                diags,
                field.to_string(),
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
    let mut out = HarnessOverrides::default();
    let Some(mapping) = val.as_mapping() else {
        push_agent_invalid(
            diags,
            "harness-overrides".to_string(),
            format!("{val:?}"),
            "mapping of harness name to target-native config mapping",
        );
        return out;
    };

    for (k, v) in mapping {
        let Some(harness_name) = k.as_str() else {
            push_agent_invalid(
                diags,
                "harness-overrides".to_string(),
                format!("{k:?}"),
                "string harness keys",
            );
            continue;
        };
        let Some(sub_mapping) = v.as_mapping() else {
            push_agent_invalid(
                diags,
                format!("harness-overrides.{harness_name}"),
                format!("{v:?}"),
                "mapping of target-native keys",
            );
            continue;
        };
        if mars_schema_harness_known(harness_name).is_none() {
            push_unknown_harness_override(diags, harness_name);
        }
        let mut parsed = serde_json::Map::new();
        for (target_key, target_value) in sub_mapping {
            let Some(key_text) = target_key.as_str() else {
                push_agent_invalid(
                    diags,
                    format!("harness-overrides.{harness_name}"),
                    format!("{target_key:?}"),
                    "string target-native keys",
                );
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
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
    field: impl Into<String>,
    value: impl Into<String>,
    allowed: &'static str,
) {
    push_agent_invalid(diags, field, value, allowed);
}

fn push_model_policy_parse_error(
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
    position: usize,
    error: crate::model_policy::ModelPolicyRuleParseError,
) {
    let rule_field = format!("model-policies[{position}]");
    match error {
        crate::model_policy::ModelPolicyRuleParseError::RuleMustBeMapping { found } => {
            push_model_policy_invalid(diags, rule_field, found, "mapping with match and override");
        }
        crate::model_policy::ModelPolicyRuleParseError::MatchMissing => {
            push_model_policy_invalid(
                diags,
                format!("{rule_field}.match"),
                "<missing>",
                "mapping with exactly one of model, alias, model-glob",
            );
        }
        crate::model_policy::ModelPolicyRuleParseError::MatchMustBeMapping { found } => {
            push_model_policy_invalid(
                diags,
                format!("{rule_field}.match"),
                found,
                "mapping with exactly one of model, alias, model-glob",
            );
        }
        crate::model_policy::ModelPolicyRuleParseError::MatchMustContainExactlyOne { found } => {
            push_model_policy_invalid(
                diags,
                format!("{rule_field}.match"),
                found,
                "exactly one of model, alias, model-glob",
            );
        }
        crate::model_policy::ModelPolicyRuleParseError::MatchKeyMustBeString { found } => {
            push_model_policy_invalid(
                diags,
                format!("{rule_field}.match"),
                found,
                "model, alias, model-glob",
            );
        }
        crate::model_policy::ModelPolicyRuleParseError::UnknownMatchKey { key } => {
            push_model_policy_invalid(
                diags,
                format!("{rule_field}.match"),
                key,
                "model, alias, model-glob",
            );
        }
        crate::model_policy::ModelPolicyRuleParseError::MatchValueMustBeString { key, found } => {
            push_model_policy_invalid(
                diags,
                format!("{rule_field}.match.{key}"),
                found,
                "non-empty string",
            );
        }
        crate::model_policy::ModelPolicyRuleParseError::MatchValueEmpty { key } => {
            push_model_policy_invalid(
                diags,
                format!("{rule_field}.match.{key}"),
                "<empty>",
                "non-empty string",
            );
        }
        crate::model_policy::ModelPolicyRuleParseError::OverrideMustBeMapping { found } => {
            push_model_policy_invalid(diags, format!("{rule_field}.override"), found, "mapping");
        }
        crate::model_policy::ModelPolicyRuleParseError::UnknownOverrideKey { key } => {
            push_model_policy_invalid(
                diags,
                format!("{rule_field}.override.{key}"),
                key,
                "effort, sandbox, approval",
            );
        }
        crate::model_policy::ModelPolicyRuleParseError::OverrideValueInvalid {
            key,
            found,
            allowed,
        } => {
            push_model_policy_invalid(
                diags,
                format!("{rule_field}.override.{key}"),
                found,
                allowed,
            );
        }
        crate::model_policy::ModelPolicyRuleParseError::NoFallbackMustBeBoolean { found } => {
            push_model_policy_invalid(diags, format!("{rule_field}.no-fallback"), found, "boolean");
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
            "model-policies",
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
        push_agent_invalid(diags, field.to_string(), invalid, "boolean");
    }
    (value, had_field)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse an agent profile from a [`Frontmatter`].
///
/// Collects diagnostics without failing — the caller decides whether errors
/// are fatal. The parsed [`AgentProfile`] is always returned even when there
/// are validation errors; invalid fields are skipped (omitted from the profile).
fn parse_agent_profile_inner(
    fm: &Frontmatter,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) -> AgentProfile {
    let name = fm.name().map(str::trim).map(str::to_owned);
    let description = fm
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_owned);

    // harness:
    let harness = fm.get("harness").and_then(Value::as_str).and_then(|s| {
        if let Some(h) = crate::harness::parse(s) {
            Some(h)
        } else {
            push_unknown_harness(diags, s);
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
                push_agent_invalid(
                    diags,
                    "mode".to_string(),
                    other.to_string(),
                    "primary, subagent",
                );
                None
            }
        });

    // model-invocable / user-invocable:
    let (model_invocable, had_model_invocable_field) =
        parse_invocability_field("model-invocable", fm.get("model-invocable"), diags);
    let (user_invocable, had_user_invocable_field) =
        parse_invocability_field("user-invocable", fm.get("user-invocable"), diags);

    // approval:
    let approval = fm.get("approval").and_then(Value::as_str).and_then(|s| {
        if let Some(a) = ApprovalMode::parse(s) {
            Some(a)
        } else {
            push_agent_invalid(
                diags,
                "approval".to_string(),
                s.to_string(),
                "default, auto, confirm, never",
            );
            None
        }
    });

    // sandbox:
    let sandbox = fm.get("sandbox").and_then(Value::as_str).and_then(|s| {
        if let Some(sb) = SandboxMode::parse(s) {
            Some(sb)
        } else {
            push_agent_invalid(
                diags,
                "sandbox".to_string(),
                s.to_string(),
                "default, read-only, workspace-write, danger-full-access",
            );
            None
        }
    });

    // effort:
    let effort = fm.get("effort").and_then(Value::as_str).and_then(|s| {
        if let Some(e) = EffortLevel::parse(s) {
            Some(e)
        } else {
            push_agent_invalid(diags, "effort", s, "low, medium, high, xhigh, none");
            None
        }
    });

    // skills/subagents/tools/disallowed-tools:
    let skills = fm.skills_structured();
    let subagents = fm
        .get("subagents")
        .map(tool_policy::yaml_str_list)
        .unwrap_or_default();
    let tools = {
        let mut invalid = |field: &str, value: &str, allowed: &'static str| {
            push_agent_tool_invalid(diags, field, value, allowed);
        };
        tool_policy::parse_tool_policy(fm.get("tools"), None, &mut invalid)
    };

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
        skills,
        subagents,
        tools,
        harness_overrides,
        model_policies,
    }
}

pub fn parse_agent_profile(
    fm: &Frontmatter,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) -> AgentProfile {
    strict_agent_diagnostics(fm, diags);
    parse_agent_profile_inner(fm, diags)
}

fn strict_agent_diagnostics(
    fm: &Frontmatter,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) {
    use crate::diagnostic::{SchemaDiagnostic, Severity, key_path};
    const KEPT: &[&str] = &[
        "name",
        "description",
        "mode",
        "model",
        "effort",
        "skills",
        "subagents",
        "tools",
        "model-invocable",
        "user-invocable",
        "sandbox",
        "approval",
        "harness-overrides",
        "model-policies",
        "harness",
    ];
    const REMOVED: &[(&str, &str)] = &[
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
    for key in fm.keys() {
        if let Some((_, replacement)) = REMOVED.iter().find(|(removed, _)| *removed == key) {
            diags.push(SchemaDiagnostic::new(
                "field.removed",
                Severity::Error,
                key_path(&key),
                format!("`{key}` was removed; {replacement}"),
            ));
        } else if !KEPT.contains(&key.as_str()) {
            diags.push(SchemaDiagnostic::new(
                "field.unknown",
                Severity::Warning,
                key_path(&key),
                format!("unknown field `{key}` is preserved"),
            ));
        }
    }
    if matches!(fm.get("tools"), Some(Value::Sequence(_))) {
        diags.push(SchemaDiagnostic::new(
            "tools.list-form",
            Severity::Error,
            key_path("tools"),
            "`tools` must be an action map; list form was removed",
        ));
    }
    if matches!(fm.get("approval"), Some(Value::String(value)) if value == "yolo") {
        diags.push(SchemaDiagnostic::new(
            "field.removed",
            Severity::Error,
            key_path("approval"),
            "approval value `yolo` was removed; use `never`",
        ));
    }
    for key in [
        "name",
        "description",
        "model",
        "mode",
        "effort",
        "sandbox",
        "approval",
        "harness",
    ] {
        if let Some(value) = fm.get(key)
            && !value.is_string()
        {
            diags.push(SchemaDiagnostic::new(
                "field.type",
                Severity::Error,
                key_path(key),
                format!("`{key}` must be a string"),
            ));
        }
    }
    for key in ["name", "model"] {
        if matches!(fm.get(key), Some(Value::String(value)) if value.trim().is_empty()) {
            diags.push(SchemaDiagnostic::new(
                "field.value",
                Severity::Error,
                key_path(key),
                format!("`{key}` must be a non-empty string"),
            ));
        }
    }
    validate_skills(fm.get("skills"), diags);
    validate_string_list("subagents", fm.get("subagents"), diags);
    for key in ["model-invocable", "user-invocable"] {
        if let Some(value) = fm.get(key)
            && !value.is_bool()
        {
            diags.push(SchemaDiagnostic::new(
                "field.type",
                Severity::Error,
                key_path(key),
                format!("`{key}` must be true or false"),
            ));
        }
    }
}

fn validate_skills(value: Option<&Value>, diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>) {
    let Some(value) = value else { return };
    if let Value::Mapping(mapping) = value {
        for field in ["load", "available"] {
            validate_string_list(
                &format!("skills.{field}"),
                mapping.get(Value::String(field.to_string())),
                diags,
            );
        }
    } else {
        validate_string_list("skills", Some(value), diags);
    }
}

fn validate_string_list(
    field: &str,
    value: Option<&Value>,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) {
    use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity, key_path};
    let Some(value) = value else { return };
    let Some(values) = value.as_sequence() else {
        diags.push(SchemaDiagnostic::new(
            "field.type",
            Severity::Error,
            key_path(field),
            format!("`{field}` must be a list of non-empty strings"),
        ));
        return;
    };
    for (index, value) in values.iter().enumerate() {
        if !matches!(value, Value::String(text) if !text.trim().is_empty()) {
            let mut path = key_path(field);
            path.push(PathSegment::Index(index));
            diags.push(SchemaDiagnostic::new(
                if value.is_string() {
                    "field.value"
                } else {
                    "field.type"
                },
                Severity::Error,
                path,
                format!("`{field}` entry {} must be a non-empty string", index + 1),
            ));
        }
    }
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
    match Frontmatter::parse(content) {
        Ok(frontmatter) => {
            let fields = parse_agent_profile(&frontmatter, &mut diagnostics);
            const KNOWN: &[&str] = &[
                "name",
                "description",
                "mode",
                "model",
                "effort",
                "skills",
                "subagents",
                "tools",
                "model-invocable",
                "user-invocable",
                "sandbox",
                "approval",
                "harness-overrides",
                "model-policies",
                "harness",
            ];
            let unknown = frontmatter
                .keys()
                .into_iter()
                .filter(|key| !KNOWN.contains(&key.as_str()))
                .filter_map(|key| frontmatter.get(&key).cloned().map(|value| (key, value)))
                .collect();
            crate::read::DefinitionRead {
                fields: Some(fields),
                body: Some(frontmatter.body().to_string()),
                unknown,
                diagnostics,
            }
        }
        Err(error) => crate::read::DefinitionRead {
            fields: None,
            body: None,
            unknown: Vec::new(),
            diagnostics: vec![crate::diagnostic::frontmatter_error(&error)],
        },
    }
}
