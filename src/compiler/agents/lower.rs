use crate::compiler::agents::{AgentProfile, HarnessKind};
/// Per-target agent lowering — translates a parsed [`AgentProfile`] into
///
/// # Lossiness classification (per agent-compilation-mapping.md §6)
///
/// Every field lowering is classified as:
/// - **exact** — field maps 1:1 to a native equivalent with identical semantics
/// - **approximate** — semantic equivalent exists but gap is noted
/// - **dropped** — no native equivalent; value is discarded in native artifact
/// - **meridian-only** — consumed exclusively by Meridian; never lowered
///
/// Dropped fields with non-default values emit [`LossyField`] diagnostics.
pub use crate::compiler::lossiness::{Lossiness, LossyField, LoweredOutput};
use crate::compiler::mcp_ref::{McpRef, McpUnsupportedReason, project_mcp_ref_tokens};
use crate::compiler::tool_names::{ToolProjectionStatus, project_tool_for_harness};
pub use crate::compiler::tool_policy::EffectiveToolPolicy;
use crate::frontmatter::Frontmatter;

// ---------------------------------------------------------------------------
// Effective field access for target lowering
// ---------------------------------------------------------------------------

/// Effective field values read from top-level Mars semantics.
struct Effective<'a> {
    harness: &'a HarnessKind,
    profile: &'a AgentProfile,
    tools: EffectiveToolPolicy,
}

impl<'a> Effective<'a> {
    fn new(profile: &'a AgentProfile, harness: &'a HarnessKind) -> Self {
        let tools = profile.effective_tool_policy(harness);
        Self {
            harness,
            profile,
            tools,
        }
    }

    fn effort(&self) -> Option<&crate::compiler::agents::EffortLevel> {
        self.profile.effort.as_ref()
    }

    fn approval(&self) -> Option<&crate::compiler::agents::ApprovalMode> {
        self.profile.approval.as_ref()
    }

    fn sandbox(&self) -> Option<&crate::compiler::agents::SandboxMode> {
        self.profile.sandbox.as_ref()
    }

    fn skills(&self) -> Vec<String> {
        self.profile.effective_skills(self.harness).all()
    }

    fn tools(&self) -> &[String] {
        &self.tools.allowed
    }

    fn disallowed_tools(&self) -> &[String] {
        &self.tools.disallowed
    }

    fn mcp_allowed(&self) -> &[McpRef] {
        &self.tools.mcp_allowed
    }

    fn mcp_disallowed(&self) -> &[McpRef] {
        &self.tools.mcp_disallowed
    }

    fn autocompact_pct(&self) -> Option<u8> {
        self.profile.autocompact_pct
    }

    fn native_config(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.profile.effective_native_config(self.harness)
    }
}

fn normalize_tools_for_harness(
    tools: &[String],
    harness: &str,
    field: &'static str,
    lossy: &mut Vec<LossyField>,
) -> Vec<String> {
    tools
        .iter()
        .map(|tool| {
            let projected = project_tool_for_harness(tool, harness);
            if projected.status == ToolProjectionStatus::UnknownProjected {
                lossy.push(LossyField {
                    field: field.into(),
                    target: harness.into(),
                    classification: Lossiness::Approximate {
                        note: "unknown tool projected via harness naming convention",
                    },
                });
            }
            projected.name
        })
        .collect()
}

fn record_mcp_projection_lossiness(
    unsupported: &[(String, McpUnsupportedReason)],
    field: &str,
    target: &str,
    lossy: &mut Vec<LossyField>,
) {
    for (_canonical, reason) in unsupported {
        lossy.push(LossyField {
            field: field.into(),
            target: target.into(),
            classification: Lossiness::Approximate {
                note: reason.message(),
            },
        });
    }
}

fn has_mcp_policy(eff: &Effective<'_>) -> bool {
    !eff.mcp_allowed().is_empty() || !eff.mcp_disallowed().is_empty()
}

fn record_native_mcp_lossiness(
    eff: &Effective<'_>,
    target: &str,
    note: &'static str,
    lossy: &mut Vec<LossyField>,
) {
    if has_mcp_policy(eff) {
        lossy.push(LossyField {
            field: "mcp".into(),
            target: target.into(),
            classification: Lossiness::Approximate { note },
        });
    }
}

/// Record invocation-axis lossiness for agent lowering.
///
/// Subagents have no native `disable-model-invocation` / `user-invocable` fields
/// (those are skill-only keys per findings-verified-schemas.md). Explicit `false`
/// values warn-drop on every harness in v1 — no agent frontmatter carries them.
fn record_agent_invocation_lossiness(
    profile: &AgentProfile,
    target: &str,
    lossy: &mut Vec<LossyField>,
) {
    if profile.had_model_invocable_field && !profile.model_invocable {
        lossy.push(LossyField {
            field: "model-invocable".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    if profile.had_user_invocable_field && !profile.user_invocable {
        lossy.push(LossyField {
            field: "user-invocable".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
}

// ---------------------------------------------------------------------------
// Claude native artifact
// ---------------------------------------------------------------------------

/// Lower an agent profile to Claude-native markdown format.
///
/// Per agent-compilation-mapping.md V0 §10:
/// - Preserved: name, description, model, skills, tools, disallowed-tools, body
/// - Dropped (launch-time): approval, sandbox, mode, harness, autocompact, autocompact_pct,
///   model-policies, harness-overrides,
///   fanout, legacy-models
///
/// What model field a lowered native agent should carry. The native compiler is the
/// sole caller; this replaces a bare `Option<&str>` so "emit no model" is expressible
/// authoritatively, without mutating the profile to strip its model.
#[derive(Debug, Clone)]
pub enum NativeModel {
    /// Emit the profile's own model verbatim (unpinned alias / fanout token).
    #[allow(dead_code)]
    Inherit,
    /// Emit this resolved model id.
    Set(String),
    /// Emit no model field (agent emitted to a harness its model does not resolve to).
    Clear,
}

impl NativeModel {
    /// The concrete model string to render for `profile`, or `None` for "no model".
    fn resolve<'a>(&'a self, profile: &'a AgentProfile) -> Option<&'a str> {
        match self {
            NativeModel::Inherit => profile.model.as_deref(),
            NativeModel::Set(model) => Some(model),
            NativeModel::Clear => None,
        }
    }
}

pub fn lower_to_claude(
    profile: &AgentProfile,
    _fm: &Frontmatter,
    body: &str,
    model_field: &NativeModel,
) -> LoweredOutput {
    let eff = Effective::new(profile, &HarnessKind::Claude);
    let mut lossy = Vec::new();
    let target = "Claude";

    // Build the native frontmatter mapping
    let mut yaml = serde_yaml::Mapping::new();
    let yk = |s: &str| serde_yaml::Value::String(s.to_string());
    let yv = |s: &str| serde_yaml::Value::String(s.to_string());

    // name — exact
    if let Some(name) = &profile.name {
        yaml.insert(yk("name"), yv(name));
    }
    // description — exact
    if let Some(desc) = &profile.description {
        yaml.insert(yk("description"), yv(desc));
    }
    // model — exact (compile-time alias resolution may supply model_field)
    if let Some(model) = model_field.resolve(profile) {
        yaml.insert(yk("model"), yv(model));
    }
    // skills — exact (Claude reads skills natively from .claude/skills/)
    let skills = eff.skills();
    if !skills.is_empty() {
        let seq: serde_yaml::Value =
            serde_yaml::Value::Sequence(skills.iter().map(|s| yv(s)).collect());
        yaml.insert(yk("skills"), seq);
    }
    // tools — native spelling plus projected MCP grants
    let mut tools = normalize_tools_for_harness(eff.tools(), "claude", "tools", &mut lossy);
    let (mcp_allowed, mcp_allowed_unsupported) =
        project_mcp_ref_tokens(eff.mcp_allowed(), "claude");
    record_mcp_projection_lossiness(&mcp_allowed_unsupported, "tools", target, &mut lossy);
    tools.extend(mcp_allowed);
    if !tools.is_empty() {
        let seq: serde_yaml::Value =
            serde_yaml::Value::Sequence(tools.iter().map(|s| yv(s)).collect());
        yaml.insert(yk("tools"), seq);
    }
    // disallowed-tools — native spelling plus projected MCP denials
    let mut dt = normalize_tools_for_harness(
        eff.disallowed_tools(),
        "claude",
        "disallowed-tools",
        &mut lossy,
    );
    let (mcp_disallowed, mcp_disallowed_unsupported) =
        project_mcp_ref_tokens(eff.mcp_disallowed(), "claude");
    record_mcp_projection_lossiness(
        &mcp_disallowed_unsupported,
        "disallowed-tools",
        target,
        &mut lossy,
    );
    dt.extend(mcp_disallowed);
    if !dt.is_empty() {
        let seq: serde_yaml::Value =
            serde_yaml::Value::Sequence(dt.iter().map(|s| yv(s)).collect());
        yaml.insert(yk("disallowed-tools"), seq);
    }

    // effort — exact (passed as frontmatter hint; Claude reads it)
    if let Some(effort) = eff.effort() {
        yaml.insert(yk("effort"), yv(effort.claude_str()));
    }

    // --- Dropped / meridian-only fields ---
    if profile.approval.is_some() {
        lossy.push(LossyField {
            field: "approval".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    if profile.sandbox.is_some() {
        lossy.push(LossyField {
            field: "sandbox".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    if profile.mode.is_some() {
        lossy.push(LossyField {
            field: "mode".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    if profile.autocompact.is_some() {
        lossy.push(LossyField {
            field: "autocompact".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if eff.autocompact_pct().is_some() {
        lossy.push(LossyField {
            field: "autocompact_pct".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if !profile.model_policies.is_empty() {
        lossy.push(LossyField {
            field: "model-policies".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if !profile.fanout.is_empty() {
        lossy.push(LossyField {
            field: "fanout".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if eff.native_config().is_some() {
        lossy.push(LossyField {
            field: "native-config".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    // harness: field is dropped (the native artifact's location IS the harness)
    // harness-overrides: launch-bundle passthrough only, dropped from native artifacts
    record_agent_invocation_lossiness(profile, target, &mut lossy);

    // Serialize
    let yaml_str = if yaml.is_empty() {
        String::new()
    } else {
        let mut s = serde_yaml::to_string(&yaml).unwrap_or_default();
        if let Some(stripped) = s.strip_prefix("---\n") {
            s = stripped.to_string();
        }
        s
    };

    let out = if yaml.is_empty() && body.is_empty() {
        String::new()
    } else if yaml.is_empty() {
        body.to_string()
    } else {
        format!("---\n{}---\n{}", yaml_str, body)
    };

    LoweredOutput {
        bytes: out.into_bytes(),
        lossy_fields: lossy,
    }
}

// ---------------------------------------------------------------------------
// Codex native artifact (TOML)
// ---------------------------------------------------------------------------

/// Lower an agent profile to Codex-native TOML format.
///
/// Per agent-compilation-mapping.md V0 §5.4 and §10:
/// - Preserved: name, description, model, effort (as model_reasoning_effort),
///   sandbox (as sandbox_mode), approval (as approval_policy), body
///   (as developer_instructions)
/// - Dropped: skills (no native field), tools (no allowlist), disallowed-tools,
///   mcp (approximate), mode, autocompact, model-policies, fanout
/// - harness-overrides.codex is launch-bundle passthrough only, not native lowering input
pub fn lower_to_codex(
    profile: &AgentProfile,
    body: &str,
    model_field: &NativeModel,
) -> LoweredOutput {
    let eff = Effective::new(profile, &HarnessKind::Codex);
    let mut lossy = Vec::new();
    let target = "Codex";

    // Effort — exact (lowered to model_reasoning_effort)
    let effort_str = eff.effort().map(|e| e.as_str());

    // Sandbox — exact
    let sandbox_str = eff.sandbox().map(|s| s.as_str());

    // Approval — exact (lowered to approval_policy)
    let approval_policy = eff.approval().and_then(|a| {
        use crate::compiler::agents::ApprovalMode;
        match a {
            ApprovalMode::Default => None,
            ApprovalMode::Auto => Some("on-request"),
            ApprovalMode::Confirm => Some("untrusted"),
            ApprovalMode::Never => Some("never"),
        }
    });

    // Dropped fields
    let skills = eff.skills();
    if !skills.is_empty() {
        lossy.push(LossyField {
            field: "skills".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    let tools = eff.tools();
    if !tools.is_empty() {
        lossy.push(LossyField {
            field: "tools".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    let dt = eff.disallowed_tools();
    if !dt.is_empty() {
        lossy.push(LossyField {
            field: "disallowed-tools".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    record_native_mcp_lossiness(
        &eff,
        target,
        "Codex per-tool MCP gating lives in server config, not the tool list",
        &mut lossy,
    );
    if profile.mode.is_some() {
        lossy.push(LossyField {
            field: "mode".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    if profile.autocompact.is_some() {
        lossy.push(LossyField {
            field: "autocompact".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if eff.autocompact_pct().is_some() {
        lossy.push(LossyField {
            field: "autocompact_pct".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if !profile.model_policies.is_empty() {
        lossy.push(LossyField {
            field: "model-policies".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if !profile.fanout.is_empty() {
        lossy.push(LossyField {
            field: "fanout".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if eff.native_config().is_some() {
        lossy.push(LossyField {
            field: "native-config".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    record_agent_invocation_lossiness(profile, target, &mut lossy);

    #[derive(serde::Serialize)]
    struct CodexAgentToml<'a> {
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model_reasoning_effort: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        sandbox_mode: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        approval_policy: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        developer_instructions: Option<&'a str>,
    }

    let doc = CodexAgentToml {
        name: profile.name.as_deref(),
        description: profile.description.as_deref(),
        model: model_field.resolve(profile),
        model_reasoning_effort: effort_str,
        sandbox_mode: sandbox_str,
        approval_policy,
        developer_instructions: (!body.trim().is_empty()).then_some(body.trim_end()),
    };

    let out = toml::to_string_pretty(&doc).unwrap_or_default();

    LoweredOutput {
        bytes: out.into_bytes(),
        lossy_fields: lossy,
    }
}

// ---------------------------------------------------------------------------
// OpenCode native artifact
// ---------------------------------------------------------------------------

/// Lower an agent profile to OpenCode-native markdown format.
///
/// Per agent-compilation-mapping.md V0 §5.5 and §10:
/// - Preserved: name, description, model (normalized to provider/model), mode
///   (approximate — same field name), body
/// - Dropped: most policy fields (approval, sandbox, tools, disallowed-tools,
///   effort, mcp, autocompact)
/// - Meridian-only: model-policies, fanout
fn lower_to_opencode_like(
    profile: &AgentProfile,
    body: &str,
    harness: HarnessKind,
    target: &str,
    model_field: &NativeModel,
) -> LoweredOutput {
    let eff = Effective::new(profile, &harness);
    let mut lossy = Vec::new();

    let mut yaml = serde_yaml::Mapping::new();
    let yk = |s: &str| serde_yaml::Value::String(s.to_string());
    let yv = |s: &str| serde_yaml::Value::String(s.to_string());

    if let Some(name) = &profile.name {
        yaml.insert(yk("name"), yv(name));
    }
    if let Some(desc) = &profile.description {
        yaml.insert(yk("description"), yv(desc));
    }
    if let Some(model) = model_field.resolve(profile) {
        yaml.insert(yk("model"), yv(model));
    }
    // mode — approximate (OpenCode has a mode concept: primary/subagent)
    if let Some(mode) = &profile.mode {
        yaml.insert(yk("mode"), yv(mode.as_str()));
        lossy.push(LossyField {
            field: "mode".into(),
            target: target.into(),
            classification: Lossiness::Approximate {
                note: "OpenCode uses the same mode concept",
            },
        });
    }

    // Dropped fields
    if eff.approval().is_some() {
        lossy.push(LossyField {
            field: "approval".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    if eff.sandbox().is_some() {
        lossy.push(LossyField {
            field: "sandbox".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    if !eff.tools().is_empty() {
        lossy.push(LossyField {
            field: "tools".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    if !eff.disallowed_tools().is_empty() {
        lossy.push(LossyField {
            field: "disallowed-tools".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    if eff.effort().is_some() {
        lossy.push(LossyField {
            field: "effort".into(),
            target: target.into(),
            classification: Lossiness::Approximate {
                note: "effort maps to --variant on subprocess only",
            },
        });
    }
    record_native_mcp_lossiness(
        &eff,
        target,
        "MCP grants on subprocess errors; streaming uses session payload",
        &mut lossy,
    );
    if profile.autocompact.is_some() {
        lossy.push(LossyField {
            field: "autocompact".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if eff.autocompact_pct().is_some() {
        lossy.push(LossyField {
            field: "autocompact_pct".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if !profile.model_policies.is_empty() {
        lossy.push(LossyField {
            field: "model-policies".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if !profile.fanout.is_empty() {
        lossy.push(LossyField {
            field: "fanout".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if eff.native_config().is_some() {
        lossy.push(LossyField {
            field: "native-config".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    record_agent_invocation_lossiness(profile, target, &mut lossy);

    // Serialize
    let yaml_str = if yaml.is_empty() {
        String::new()
    } else {
        let mut s = serde_yaml::to_string(&yaml).unwrap_or_default();
        if let Some(stripped) = s.strip_prefix("---\n") {
            s = stripped.to_string();
        }
        s
    };

    let out = if yaml.is_empty() {
        body.to_string()
    } else {
        format!("---\n{}---\n{}", yaml_str, body)
    };

    LoweredOutput {
        bytes: out.into_bytes(),
        lossy_fields: lossy,
    }
}

pub fn lower_to_opencode(
    profile: &AgentProfile,
    body: &str,
    model_field: &NativeModel,
) -> LoweredOutput {
    lower_to_opencode_like(
        profile,
        body,
        HarnessKind::OpenCode,
        "OpenCode",
        model_field,
    )
}

fn normalize_cursor_description(description: &str) -> String {
    description.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn lower_to_cursor_with_model(
    profile: &AgentProfile,
    body: &str,
    model_field: &NativeModel,
) -> LoweredOutput {
    let eff = Effective::new(profile, &HarnessKind::Cursor);
    let mut lossy = Vec::new();
    let target = "Cursor";

    let mut yaml = serde_yaml::Mapping::new();
    let yk = |s: &str| serde_yaml::Value::String(s.to_string());
    let yv = |s: &str| serde_yaml::Value::String(s.to_string());

    if let Some(name) = &profile.name {
        yaml.insert(yk("name"), yv(name));
    }
    if let Some(desc) = &profile.description {
        yaml.insert(yk("description"), yv(&normalize_cursor_description(desc)));
    }
    if let Some(model) = model_field.resolve(profile) {
        yaml.insert(yk("model"), yv(model));
    }
    let skills = eff.skills();
    if !skills.is_empty() {
        let seq: serde_yaml::Value =
            serde_yaml::Value::Sequence(skills.iter().map(|skill| yv(skill)).collect());
        yaml.insert(yk("skills"), seq);
    }
    // mode — approximate (Cursor may use the same mode concept)
    if let Some(mode) = &profile.mode {
        yaml.insert(yk("mode"), yv(mode.as_str()));
        lossy.push(LossyField {
            field: "mode".into(),
            target: target.into(),
            classification: Lossiness::Approximate {
                note: "Cursor may use the same mode concept",
            },
        });
    }

    // approval — approximate: auto maps to --force, yolo to --yolo; confirm has no Cursor
    // equivalent and falls back to default.
    if eff.approval().is_some() {
        lossy.push(LossyField {
            field: "approval".into(),
            target: target.into(),
            classification: Lossiness::Approximate {
                note: "auto maps to --force, yolo to --yolo; confirm has no Cursor equivalent and falls back to default",
            },
        });
    }
    // sandbox — approximate: Cursor only supports enabled/disabled; workspace-write and
    // danger-full-access both map to --sandbox disabled.
    if eff.sandbox().is_some() {
        lossy.push(LossyField {
            field: "sandbox".into(),
            target: target.into(),
            classification: Lossiness::Approximate {
                note: "Cursor only supports enabled/disabled; workspace-write and danger-full-access both map to disabled",
            },
        });
    }
    if !eff.tools().is_empty() {
        lossy.push(LossyField {
            field: "tools".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    if !eff.disallowed_tools().is_empty() {
        lossy.push(LossyField {
            field: "disallowed-tools".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    if eff.effort().is_some() {
        lossy.push(LossyField {
            field: "effort".into(),
            target: target.into(),
            classification: Lossiness::Approximate {
                note: "effort maps to --variant on subprocess only",
            },
        });
    }
    record_native_mcp_lossiness(
        &eff,
        target,
        "MCP grants on subprocess errors; streaming uses session payload",
        &mut lossy,
    );
    if profile.autocompact.is_some() {
        lossy.push(LossyField {
            field: "autocompact".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if eff.autocompact_pct().is_some() {
        lossy.push(LossyField {
            field: "autocompact_pct".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if !profile.model_policies.is_empty() {
        lossy.push(LossyField {
            field: "model-policies".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if !profile.fanout.is_empty() {
        lossy.push(LossyField {
            field: "fanout".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if eff.native_config().is_some() {
        lossy.push(LossyField {
            field: "native-config".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    record_agent_invocation_lossiness(profile, target, &mut lossy);

    let yaml_str = if yaml.is_empty() {
        String::new()
    } else {
        let mut s = serde_yaml::to_string(&yaml).unwrap_or_default();
        if let Some(stripped) = s.strip_prefix("---\n") {
            s = stripped.to_string();
        }
        s
    };

    let out = if yaml.is_empty() {
        body.to_string()
    } else {
        format!("---\n{}---\n{}", yaml_str, body)
    };

    LoweredOutput {
        bytes: out.into_bytes(),
        lossy_fields: lossy,
    }
}

// ---------------------------------------------------------------------------
// Pi native artifact
// ---------------------------------------------------------------------------

/// Lower an agent profile to Pi-native markdown format.
///
/// Pi's format is similar to OpenCode: markdown + YAML frontmatter with a
/// minimal subset of fields. Per agent-compilation-mapping.md §6, all policy
/// fields are dropped.
pub fn lower_to_pi(profile: &AgentProfile, body: &str, model_field: &NativeModel) -> LoweredOutput {
    let mut lossy = Vec::new();
    let target = "Pi";

    let mut yaml = serde_yaml::Mapping::new();
    let yk = |s: &str| serde_yaml::Value::String(s.to_string());
    let yv = |s: &str| serde_yaml::Value::String(s.to_string());

    if let Some(name) = &profile.name {
        yaml.insert(yk("name"), yv(name));
    }
    if let Some(desc) = &profile.description {
        yaml.insert(yk("description"), yv(desc));
    }
    if let Some(model) = model_field.resolve(profile) {
        yaml.insert(yk("model"), yv(model));
    }
    // mode — approximate
    if let Some(mode) = &profile.mode {
        yaml.insert(yk("mode"), yv(mode.as_str()));
        lossy.push(LossyField {
            field: "mode".into(),
            target: target.into(),
            classification: Lossiness::Approximate {
                note: "Pi may use the same mode concept",
            },
        });
    }

    // Everything else is dropped
    let eff = Effective::new(profile, &HarnessKind::Pi);
    if eff.approval().is_some() {
        lossy.push(LossyField {
            field: "approval".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    if eff.sandbox().is_some() {
        lossy.push(LossyField {
            field: "sandbox".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    if !eff.tools().is_empty() {
        lossy.push(LossyField {
            field: "tools".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    if !eff.disallowed_tools().is_empty() {
        lossy.push(LossyField {
            field: "disallowed-tools".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    if eff.effort().is_some() {
        lossy.push(LossyField {
            field: "effort".into(),
            target: target.into(),
            classification: Lossiness::Approximate {
                note: "Pi effort semantics unverified",
            },
        });
    }
    if has_mcp_policy(&eff) {
        lossy.push(LossyField {
            field: "mcp".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    if profile.autocompact.is_some() {
        lossy.push(LossyField {
            field: "autocompact".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if eff.autocompact_pct().is_some() {
        lossy.push(LossyField {
            field: "autocompact_pct".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if !profile.model_policies.is_empty() {
        lossy.push(LossyField {
            field: "model-policies".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if !profile.fanout.is_empty() {
        lossy.push(LossyField {
            field: "fanout".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if eff.native_config().is_some() {
        lossy.push(LossyField {
            field: "native-config".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    record_agent_invocation_lossiness(profile, target, &mut lossy);

    let yaml_str = if yaml.is_empty() {
        String::new()
    } else {
        let mut s = serde_yaml::to_string(&yaml).unwrap_or_default();
        if let Some(stripped) = s.strip_prefix("---\n") {
            s = stripped.to_string();
        }
        s
    };

    let out = if yaml.is_empty() {
        body.to_string()
    } else {
        format!("---\n{}---\n{}", yaml_str, body)
    };

    LoweredOutput {
        bytes: out.into_bytes(),
        lossy_fields: lossy,
    }
}

// ---------------------------------------------------------------------------
// Dispatch: lower for a given harness
// ---------------------------------------------------------------------------

/// Lower an agent to the native format for the given harness.
///
/// Returns `None` for unknown harnesses (should not happen if the profile was
/// validated, but guards against future harness additions).
pub fn lower_for_harness_with_model(
    harness: &HarnessKind,
    profile: &AgentProfile,
    fm: &Frontmatter,
    body: &str,
    model_field: &NativeModel,
) -> LoweredOutput {
    match harness {
        HarnessKind::Claude => lower_to_claude(profile, fm, body, model_field),
        HarnessKind::Codex => lower_to_codex(profile, body, model_field),
        HarnessKind::OpenCode => lower_to_opencode(profile, body, model_field),
        HarnessKind::Cursor => lower_to_cursor_with_model(profile, body, model_field),
        HarnessKind::Pi => lower_to_pi(profile, body, model_field),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::agents::{AgentDiagnostic, parse_agent_content};

    fn profile_from(content: &str) -> (AgentProfile, Frontmatter, Vec<AgentDiagnostic>) {
        let mut diags = Vec::new();
        let (profile, fm) = parse_agent_content(content, &mut diags).unwrap();
        (profile, fm, diags)
    }

    // --- 3.3: Claude lowering ---

    #[test]
    fn claude_lowering_preserves_name_description_model_skills_tools_body() {
        let content = "---\nname: coder\ndescription: Code impl agent\nmodel: gpt55\nharness: claude\nskills: [dev-principles]\ntools: [Bash, Write]\n---\n# Coder\nYou write code.";
        let (profile, fm, _) = profile_from(content);
        let body = fm.body();
        let out = lower_to_claude(&profile, &fm, body, &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("name: coder"), "name missing: {text}");
        assert!(
            text.contains("description: Code impl agent"),
            "desc missing"
        );
        assert!(text.contains("model: gpt55"), "model missing");
        assert!(text.contains("skills"), "skills missing");
        assert!(text.contains("tools"), "tools missing");
        assert!(text.contains("# Coder"), "body missing");
    }

    #[test]
    fn claude_lowering_projects_canonical_tool_names() {
        let content = "---\nname: coder\nharness: claude\ntools: [Bash, AskUser]\ndisallowed-tools: [Bash(git reset *)]\n---\n# Body";
        let (profile, fm, diags) = profile_from(content);
        assert!(diags.is_empty());
        let out = lower_to_claude(&profile, &fm, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();

        assert!(text.contains("- Bash"), "Bash not projected: {text}");
        assert!(text.contains("- AskUser"), "AskUser not projected: {text}");
        assert!(
            text.contains("- Bash(git reset *)"),
            "scoped Bash not projected while preserving payload: {text}"
        );
    }

    #[test]
    fn claude_lowering_projects_unknown_custom_tools_and_preserves_mcp_wire_identifiers() {
        let content = "---\nname: coder\nharness: claude\ntools: [my_custom_tool, mcp__server__Tool]\n---\n# Body";
        let (profile, fm, diags) = profile_from(content);
        assert!(diags.is_empty());
        let out = lower_to_claude(&profile, &fm, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();

        assert!(
            text.contains("- MyCustomTool"),
            "unknown custom tool should be convention-projected: {text}"
        );
        assert!(
            text.contains("- mcp__server__Tool"),
            "mcp__ wire identifier should be preserved verbatim: {text}"
        );
        assert!(out.lossy_fields.iter().any(|field| {
            field.field == "tools"
                && field.target == "claude"
                && matches!(
                    field.classification,
                    Lossiness::Approximate {
                        note: "unknown tool projected via harness naming convention"
                    }
                )
        }));
    }

    #[test]
    fn codex_and_cursor_agent_lowering_drops_tools_without_native_projection() {
        let content = "---\nname: coder\nharness: codex\ntools: [my_custom_tool, mcp__server__Tool]\n---\n# Body";
        let (profile, fm, _) = profile_from(content);

        let codex = lower_to_codex(&profile, fm.body(), &NativeModel::Inherit);
        let codex_text = String::from_utf8(codex.bytes).unwrap();
        assert!(
            !codex_text.contains("my_custom_tool") && !codex_text.contains("mcp__server__Tool"),
            "codex native agent artifacts drop tools entirely: {codex_text}"
        );
        assert!(codex.lossy_fields.iter().any(|field| {
            field.field == "tools" && matches!(field.classification, Lossiness::Dropped)
        }));

        let cursor = lower_to_cursor_with_model(&profile, fm.body(), &NativeModel::Inherit);
        let cursor_text = String::from_utf8(cursor.bytes).unwrap();
        assert!(
            !cursor_text.contains("MyCustomTool")
                && !cursor_text.contains("my_custom_tool")
                && !cursor_text.contains("mcp__server__Tool"),
            "cursor native agent artifacts drop tools entirely: {cursor_text}"
        );
        assert!(cursor.lossy_fields.iter().any(|field| {
            field.field == "tools" && matches!(field.classification, Lossiness::Dropped)
        }));
    }

    #[test]
    fn claude_lowering_drops_approval_sandbox_mode_autocompact() {
        let content = "---\nname: coder\nharness: claude\napproval: auto\nsandbox: read-only\nmode: subagent\nautocompact: 50\nautocompact_pct: 80\n---\n# Body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(!text.contains("approval:"), "approval leaked: {text}");
        assert!(!text.contains("sandbox:"), "sandbox leaked: {text}");
        assert!(!text.contains("autocompact:"), "autocompact leaked: {text}");
        // Lossiness should report dropped fields
        let dropped: Vec<_> = out.lossy_fields.iter().map(|f| f.field.as_str()).collect();
        assert!(
            dropped.contains(&"approval"),
            "approval not in lossy: {dropped:?}"
        );
        assert!(
            dropped.contains(&"sandbox"),
            "sandbox not in lossy: {dropped:?}"
        );
        assert!(
            dropped.contains(&"autocompact"),
            "autocompact not in lossy: {dropped:?}"
        );
        assert!(
            dropped.contains(&"autocompact_pct"),
            "autocompact_pct not in lossy: {dropped:?}"
        );
    }

    #[test]
    fn claude_harness_override_does_not_replace_skills_before_lowering() {
        let content = "---\nname: r\nharness: claude\nskills: [base-skill]\nharness-overrides:\n  claude:\n    skills: [override-skill]\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            text.contains("base-skill"),
            "top-level skill should be lowered: {text}"
        );
        assert!(
            !text.contains("override-skill"),
            "harness-overrides passthrough should not replace skills: {text}"
        );
    }

    #[test]
    fn claude_harness_override_does_not_replace_mcp() {
        let content = "---\nname: r\nharness: claude\ntools: [mcp(plugin:base)]\nharness-overrides:\n  claude:\n    mcp-tools: [plugin:claude]\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("mcp-tools:"),
            "MCP grants belong in tools:, not a separate field: {text}"
        );
        assert!(
            text.contains("mcp__plugin:base__*"),
            "base mcp should project into tools: {text}"
        );
        assert!(
            !text.contains("plugin:claude"),
            "harness-overrides passthrough should not replace profile mcp: {text}"
        );
    }

    #[test]
    fn claude_agent_projects_mcp_refs_into_tools_and_disallowed() {
        let content = "---\nname: r\nharness: claude\ntools: [mcp(github/create_issue), mcp(context7)]\ndisallowed-tools: [mcp(github/delete_repo), mcp(*/cross_server_tool)]\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            text.contains("mcp__github__create_issue"),
            "per-tool MCP should land in tools: {text}"
        );
        assert!(
            text.contains("mcp__context7__*"),
            "whole-server MCP should land in tools: {text}"
        );
        assert!(
            text.contains("mcp__github__delete_repo"),
            "disallowed MCP should project into disallowed-tools: {text}"
        );
        assert!(
            !text.contains("mcp__*__cross_server_tool"),
            "cross-server MCP must not be emitted: {text}"
        );
        assert!(out.lossy_fields.iter().any(|field| {
            field.field == "disallowed-tools"
                && matches!(
                    field.classification,
                    Lossiness::Approximate {
                        note: "Claude cannot scope a single MCP tool across all servers"
                    }
                )
        }));
    }

    #[test]
    fn pi_agent_records_mcp_lossiness_when_mcp_refs_present() {
        let content = "---\nname: r\nharness: pi\ntools: [mcp(plugin:demo)]\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_pi(&profile, fm.body(), &NativeModel::Inherit);
        assert!(out.lossy_fields.iter().any(|field| {
            field.field == "mcp"
                && field.target == "Pi"
                && matches!(field.classification, Lossiness::Dropped)
        }));
    }

    #[test]
    fn claude_meridian_only_fields_dropped() {
        let content = "---\nname: r\nharness: claude\nmodel-policies:\n  - match:\n      model: gpt55\n    override:\n      harness: codex\nfanout:\n  - alias: opus\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("model-policies:"),
            "model-policies leaked: {text}"
        );
        assert!(!text.contains("fanout:"), "fanout leaked: {text}");
        let meridian_only: Vec<_> = out
            .lossy_fields
            .iter()
            .filter(|f| matches!(f.classification, Lossiness::MeridianOnly))
            .map(|f| f.field.as_str())
            .collect();
        assert!(meridian_only.contains(&"model-policies"));
        assert!(meridian_only.contains(&"fanout"));
    }

    #[test]
    fn claude_agent_model_invocable_false_warn_drops_without_skill_key() {
        let content = "---\nname: coder\nharness: claude\nmodel-invocable: false\n---\n# Body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("disable-model-invocation"),
            "subagents have no invocation frontmatter; must not emit skill-only key: {text}"
        );
        assert!(out.lossy_fields.iter().any(|f| {
            f.field == "model-invocable"
                && f.target == "Claude"
                && f.classification == Lossiness::Dropped
        }));
    }

    #[test]
    fn claude_agent_user_invocable_false_warn_drops() {
        let content = "---\nname: coder\nharness: claude\nuser-invocable: false\n---\n# Body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("user-invocable"),
            "no native agent key: {text}"
        );
        assert!(out.lossy_fields.iter().any(|f| {
            f.field == "user-invocable"
                && f.target == "Claude"
                && f.classification == Lossiness::Dropped
        }));
    }

    // --- 3.3: Codex lowering ---

    #[test]
    fn codex_lowering_produces_top_level_toml() {
        let content = "---\nname: coder\ndescription: Code agent\nmodel: gpt55\nharness: codex\neffort: high\nsandbox: workspace-write\napproval: auto\n---\n# Coder\nYou code.";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("[agent]"),
            "legacy [agent] table leaked: {text}"
        );
        assert!(text.contains("name = \"coder\""), "name missing");
        assert!(text.contains("model = \"gpt55\""), "model missing");
        assert!(
            text.contains("model_reasoning_effort = \"high\""),
            "effort missing"
        );
        assert!(
            text.contains("sandbox_mode = \"workspace-write\""),
            "sandbox missing"
        );
        assert!(
            text.contains("approval_policy = \"on-request\""),
            "approval missing"
        );
        assert!(
            text.contains("developer_instructions ="),
            "developer instructions missing"
        );

        let parsed: toml::Value = toml::from_str(&text).expect("lowered TOML should parse");
        assert!(
            parsed.get("agent").is_none(),
            "nested [agent] table present"
        );
        assert_eq!(parsed.get("name").and_then(|v| v.as_str()), Some("coder"));
    }

    #[test]
    fn codex_lowering_drops_skills_and_tools() {
        let content = "---\nname: r\nharness: codex\nskills: [review]\ntools: [Bash]\ndisallowed-tools: [Agent]\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body(), &NativeModel::Inherit);
        let dropped: Vec<_> = out
            .lossy_fields
            .iter()
            .filter(|f| matches!(f.classification, Lossiness::Dropped))
            .map(|f| f.field.as_str())
            .collect();
        assert!(dropped.contains(&"skills"));
        assert!(dropped.contains(&"tools"));
        assert!(dropped.contains(&"disallowed-tools"));
    }

    #[test]
    fn codex_harness_override_does_not_replace_execution_policy() {
        let content = "---\nname: r\nharness: codex\neffort: low\nharness-overrides:\n  codex:\n    effort: high\n    sandbox: workspace-write\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            text.contains("model_reasoning_effort = \"low\""),
            "top-level effort should be lowered: {text}"
        );
        assert!(
            !text.contains("sandbox_mode = \"workspace-write\""),
            "harness-overrides passthrough should not replace sandbox: {text}"
        );
    }

    #[test]
    fn codex_mcp_lossiness_uses_top_level_policy() {
        let content = "---\nname: r\nharness: codex\ntools: [mcp(plugin:base)]\nharness-overrides:\n  codex:\n    mcp-tools: []\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body(), &NativeModel::Inherit);
        assert!(
            out.lossy_fields.iter().any(|field| field.field == "mcp"),
            "top-level mcp should remain lossy for codex: {:?}",
            out.lossy_fields
                .iter()
                .map(|field| field.field.clone())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn codex_native_config_lossiness_uses_matching_override() {
        let content = "---\nname: r\nharness-overrides:\n  codex:\n    native-config:\n      sandbox_workspace_write.network_access: true\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body(), &NativeModel::Inherit);
        assert!(
            out.lossy_fields.iter().any(|field| {
                field.field == "native-config"
                    && field.target == "Codex"
                    && matches!(field.classification, Lossiness::MeridianOnly)
            }),
            "native-config should be reported as meridian-only in codex lowering: {:?}",
            out.lossy_fields
                .iter()
                .map(|field| (&field.field, &field.target))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn codex_lowering_multiline_instructions_are_parseable() {
        let content = "---\nname: explorer\ndescription: \"Line one\\nLine two\"\nharness: codex\napproval: yolo\n---\n# Explore\nUse \"quotes\" and backslashes \\\\\nKeep going.";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        let parsed: toml::Value = toml::from_str(&text).expect("lowered TOML should parse");

        assert_eq!(
            parsed.get("approval_policy").and_then(|v| v.as_str()),
            Some("never")
        );
        assert_eq!(
            parsed
                .get("developer_instructions")
                .and_then(|v| v.as_str())
                .unwrap_or_default(),
            "# Explore\nUse \"quotes\" and backslashes \\\\\nKeep going."
        );
    }

    // --- 3.3: OpenCode lowering ---

    #[test]
    fn opencode_lowering_preserves_name_description_model_mode() {
        let content = "---\nname: r\ndescription: Reviewer\nmodel: gpt55\nmode: primary\nharness: opencode\n---\n# Reviewer\nbody";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_opencode(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("name: r"), "name missing");
        assert!(text.contains("description: Reviewer"), "desc missing");
        assert!(text.contains("model: gpt55"), "model missing");
        assert!(text.contains("mode: primary"), "mode missing");
    }

    #[test]
    fn cursor_lowering_uses_top_level_policy_and_matching_passthrough() {
        let content = "---\nname: r\nharness: cursor\ntools: [Read, mcp(plugin:base)]\nharness-overrides:\n  opencode:\n    tools: []\n    mcp-tools: []\n    native-config:\n      opencode.only: true\n  cursor:\n    tools: [Bash]\n    mcp-tools: [plugin:cursor]\n    native-config:\n      cursor.only: true\n---\n# body";
        let (profile, fm, _) = profile_from(content);

        let opencode = lower_to_opencode(&profile, fm.body(), &NativeModel::Inherit);
        assert!(
            opencode
                .lossy_fields
                .iter()
                .any(|field| field.field == "tools"),
            "harness-overrides passthrough should not clear tools lossiness",
        );
        assert!(
            opencode
                .lossy_fields
                .iter()
                .any(|field| field.field == "mcp"),
            "harness-overrides passthrough should not clear mcp lossiness",
        );

        let cursor = lower_to_cursor_with_model(&profile, fm.body(), &NativeModel::Inherit);
        assert!(
            cursor
                .lossy_fields
                .iter()
                .any(|field| field.field == "tools"),
            "cursor override should keep tools lossiness",
        );
        assert!(
            cursor.lossy_fields.iter().any(|field| field.field == "mcp"),
            "cursor override should keep mcp lossiness",
        );
        assert!(
            cursor.lossy_fields.iter().any(|field| {
                field.field == "native-config"
                    && field.target == "Cursor"
                    && matches!(field.classification, Lossiness::MeridianOnly)
            }),
            "cursor native-config should be reported as meridian-only",
        );
    }

    #[test]
    fn cursor_lowering_normalizes_multiline_description_to_one_line() {
        let content = "---\nname: cursor-agent\ndescription: |\n  Cursor agent\n  with   lots\t of\n  whitespace\nharness: cursor\n---\n# body";
        let (profile, fm, _) = profile_from(content);

        let out = lower_to_cursor_with_model(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();

        assert!(
            text.contains("description: Cursor agent with lots of whitespace")
                || text.contains("description: \"Cursor agent with lots of whitespace\""),
            "cursor description should be one line: {text}"
        );
        assert!(
            !text.contains("description: |\n"),
            "block description should be flattened: {text}"
        );
    }

    #[test]
    fn cursor_sandbox_is_approximate_not_dropped() {
        let content = "---\nname: r\nharness: cursor\nsandbox: read-only\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_cursor_with_model(&profile, fm.body(), &NativeModel::Inherit);

        // sandbox must not appear in the emitted YAML artifact
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("sandbox:"),
            "sandbox leaked into artifact: {text}"
        );

        // lossiness must be Approximate, not Dropped
        let field = out
            .lossy_fields
            .iter()
            .find(|f| f.field == "sandbox")
            .expect("sandbox should appear in lossy_fields");
        assert_eq!(field.target, "Cursor");
        assert!(
            matches!(field.classification, Lossiness::Approximate { .. }),
            "expected Approximate, got {:?}",
            field.classification
        );
        if let Lossiness::Approximate { note } = field.classification {
            assert!(
                note.contains("workspace-write") && note.contains("disabled"),
                "note should document workspace-write mapping: {note}"
            );
        }
    }

    #[test]
    fn cursor_approval_is_approximate_not_dropped() {
        let content = "---\nname: r\nharness: cursor\napproval: auto\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_cursor_with_model(&profile, fm.body(), &NativeModel::Inherit);

        // approval must not appear in the emitted YAML artifact
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("approval:"),
            "approval leaked into artifact: {text}"
        );

        // lossiness must be Approximate, not Dropped
        let field = out
            .lossy_fields
            .iter()
            .find(|f| f.field == "approval")
            .expect("approval should appear in lossy_fields");
        assert_eq!(field.target, "Cursor");
        assert!(
            matches!(field.classification, Lossiness::Approximate { .. }),
            "expected Approximate, got {:?}",
            field.classification
        );
        if let Lossiness::Approximate { note } = field.classification {
            assert!(
                note.contains("--force") && note.contains("--yolo"),
                "note should document --force/--yolo mapping: {note}"
            );
        }
    }

    // --- 3.3: Pi lowering ---

    #[test]
    fn pi_lowering_preserves_name_description_model() {
        let content = "---\nname: pi-agent\ndescription: Pi agent\nmodel: gpt55\nharness: pi\n---\n# Pi\nbody";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_pi(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("name: pi-agent"), "name missing");
        assert!(text.contains("description: Pi agent"), "desc missing");
    }

    #[test]
    fn lower_for_harness_with_model_field_emits_pinned_id_for_claude_and_codex() {
        let claude_content = "---\nname: coder\nmodel: gpt55\nharness: claude\n---\n# Coder\nbody";
        let (claude_profile, claude_fm, _) = profile_from(claude_content);
        let claude_out = lower_for_harness_with_model(
            &HarnessKind::Claude,
            &claude_profile,
            &claude_fm,
            claude_fm.body(),
            &NativeModel::Set("o3".to_string()),
        );
        let claude_text = String::from_utf8(claude_out.bytes).unwrap();
        assert!(
            claude_text.contains("model: o3"),
            "claude override: {claude_text}"
        );
        assert!(
            !claude_text.contains("model: gpt55"),
            "alias should not leak when override set: {claude_text}"
        );

        let codex_content = "---\nname: coder\nmodel: gpt55\nharness: codex\n---\n# body";
        let (codex_profile, codex_fm, _) = profile_from(codex_content);
        let codex_out = lower_for_harness_with_model(
            &HarnessKind::Codex,
            &codex_profile,
            &codex_fm,
            codex_fm.body(),
            &NativeModel::Set("o3".to_string()),
        );
        let codex_text = String::from_utf8(codex_out.bytes).unwrap();
        assert!(
            codex_text.contains("model = \"o3\""),
            "codex override: {codex_text}"
        );
        assert!(
            !codex_text.contains("model = \"gpt55\""),
            "alias should not leak when override set: {codex_text}"
        );
    }

    // --- 3.3: Dispatch ---

    #[test]
    fn lower_for_harness_dispatches_correctly() {
        let content = "---\nname: coder\nmodel: gpt55\nharness: claude\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let body = fm.body().to_string();
        let out = lower_for_harness_with_model(
            &HarnessKind::Claude,
            &profile,
            &fm,
            &body,
            &NativeModel::Inherit,
        );
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("---"), "not markdown format");

        let content2 = "---\nname: coder\nmodel: gpt55\nharness: codex\n---\n# body";
        let (profile2, fm2, _) = profile_from(content2);
        let body2 = fm2.body().to_string();
        let out2 = lower_for_harness_with_model(
            &HarnessKind::Codex,
            &profile2,
            &fm2,
            &body2,
            &NativeModel::Inherit,
        );
        let text2 = String::from_utf8(out2.bytes).unwrap();
        assert!(text2.contains("name = \"coder\""), "not TOML format");
        assert!(
            !text2.contains("[agent]"),
            "legacy nested agent table emitted"
        );

        let content3 = "---\nname: cursor-agent\nmodel: gpt55\nharness: cursor\n---\n# body";
        let (profile3, fm3, _) = profile_from(content3);
        let out3 = lower_for_harness_with_model(
            &HarnessKind::Cursor,
            &profile3,
            &fm3,
            fm3.body(),
            &NativeModel::Inherit,
        );
        let text3 = String::from_utf8(out3.bytes).unwrap();
        assert!(
            text3.contains("name: cursor-agent"),
            "cursor lowering missing name"
        );
    }
}
