/// Per-target agent lowering — translates a parsed [`AgentProfile`] into
/// harness-native format bytes.
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
use crate::compiler::agents::{
    AgentProfile, HarnessKind, OverrideFields, SandboxMode, ToolAction, ToolRule, ToolsField,
};
use crate::frontmatter::Frontmatter;

// ---------------------------------------------------------------------------
// Lossiness result types
// ---------------------------------------------------------------------------

/// A field that was dropped or only approximately lowered in the native artifact.
#[derive(Debug, Clone)]
pub struct LossyField {
    pub field: String,
    pub target: String,
    pub classification: Lossiness,
}

/// Lossiness classification for a single field in a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lossiness {
    Approximate { note: &'static str },
    Dropped,
    MeridianOnly,
}

/// Output from a single lowering pass.
pub struct LoweredOutput {
    /// Serialized bytes for the native artifact.
    pub bytes: Vec<u8>,
    /// Lossiness findings for fields that were dropped or approximated.
    pub lossy_fields: Vec<LossyField>,
}

// ---------------------------------------------------------------------------
// Effective field resolution — applies harness-overrides before lowering
// ---------------------------------------------------------------------------

/// Effective field values after merging profile defaults + harness override.
struct Effective<'a> {
    profile: &'a AgentProfile,
    over: Option<&'a OverrideFields>,
}

impl<'a> Effective<'a> {
    fn new(profile: &'a AgentProfile, harness: &HarnessKind) -> Self {
        let over = profile.harness_overrides.get(harness);
        Self { profile, over }
    }

    fn effort(&self) -> Option<&crate::compiler::agents::EffortLevel> {
        self.over
            .and_then(|o| o.effort.as_ref())
            .or(self.profile.effort.as_ref())
    }

    fn approval(&self) -> Option<&crate::compiler::agents::ApprovalMode> {
        self.over
            .and_then(|o| o.approval.as_ref())
            .or(self.profile.approval.as_ref())
    }

    fn sandbox(&self) -> Option<&crate::compiler::agents::SandboxMode> {
        self.over
            .and_then(|o| o.sandbox.as_ref())
            .or(self.profile.sandbox.as_ref())
    }

    fn skills(&self) -> &[String] {
        if let Some(ov) = self.over.and_then(|o| o.skills.as_ref()) {
            return ov;
        }
        &self.profile.skills
    }

    fn tools(&self) -> Option<&ToolsField> {
        self.over
            .and_then(|o| o.tools.as_ref())
            .or(self.profile.tools.as_ref())
    }

    fn autocompact_pct(&self) -> Option<u8> {
        self.over
            .and_then(|o| o.autocompact_pct)
            .or(self.profile.autocompact_pct)
    }
}

const CAPABILITY_TO_CLAUDE_TOOLS: &[(&str, &[&str])] = &[
    ("bash", &["Bash"]),
    ("read", &["Read"]),
    ("edit", &["Edit", "Write"]),
    ("glob", &["Glob"]),
    ("grep", &["Grep"]),
    ("task", &["Agent"]),
    ("web", &["WebSearch", "WebFetch"]),
    ("lsp", &["LSP"]),
];

fn push_lossy(
    lossy: &mut Vec<LossyField>,
    field: impl Into<String>,
    target: &str,
    classification: Lossiness,
) {
    lossy.push(LossyField {
        field: field.into(),
        target: target.to_string(),
        classification,
    });
}

fn push_unique(dest: &mut Vec<String>, values: &[String]) {
    for value in values {
        if !dest.contains(value) {
            dest.push(value.clone());
        }
    }
}

fn cap_to_claude_tools(cap: &str) -> Vec<String> {
    CAPABILITY_TO_CLAUDE_TOOLS
        .iter()
        .find_map(|(capability, tools)| {
            (*capability == cap).then_some(tools.iter().map(|s| (*s).to_string()).collect())
        })
        .unwrap_or_else(|| vec![cap.to_string()])
}

fn compile_tools_for_claude(
    tools: Option<&ToolsField>,
    lossy: &mut Vec<LossyField>,
    target: &str,
) -> (Option<Vec<String>>, Option<Vec<String>>) {
    let Some(tools) = tools else {
        return (None, None);
    };

    match tools {
        ToolsField::Shorthand(ToolAction::Allow) => (None, None),
        ToolsField::Shorthand(ToolAction::Deny) => (Some(Vec::new()), None),
        ToolsField::Shorthand(ToolAction::Ask) => {
            push_lossy(
                lossy,
                "tools",
                target,
                Lossiness::Approximate {
                    note: "Claude cannot preserve ask policy; lowered as allow",
                },
            );
            (None, None)
        }
        ToolsField::Map(map) => {
            let default_action = match map.get("*") {
                Some(ToolRule::Action(action)) => action.clone(),
                Some(ToolRule::Scoped(_)) => {
                    push_lossy(lossy, "tools.*", target, Lossiness::Dropped);
                    ToolAction::Allow
                }
                None => ToolAction::Allow,
            };
            if default_action == ToolAction::Ask {
                push_lossy(
                    lossy,
                    "tools.*",
                    target,
                    Lossiness::Approximate {
                        note: "Claude cannot preserve ask policy; lowered as allow",
                    },
                );
            }

            let mut allow = Vec::new();
            let mut deny = Vec::new();

            for (cap, rule) in map {
                if cap == "*" {
                    continue;
                }
                match rule {
                    ToolRule::Scoped(_) => {
                        push_lossy(lossy, format!("tools.{cap}"), target, Lossiness::Dropped);
                    }
                    ToolRule::Action(action) => {
                        if *action == ToolAction::Ask {
                            push_lossy(
                                lossy,
                                format!("tools.{cap}"),
                                target,
                                Lossiness::Approximate {
                                    note: "Claude cannot preserve ask policy; lowered as allow",
                                },
                            );
                        }
                        let mapped = cap_to_claude_tools(cap);
                        let action_as_allow = *action != ToolAction::Deny;
                        let default_allows = default_action != ToolAction::Deny;
                        if action_as_allow && !default_allows {
                            push_unique(&mut allow, &mapped);
                        } else if !action_as_allow && default_allows {
                            push_unique(&mut deny, &mapped);
                        }
                    }
                }
            }

            let tools_list = if default_action == ToolAction::Deny || !allow.is_empty() {
                Some(allow)
            } else {
                None
            };
            let deny_list = (!deny.is_empty()).then_some(deny);
            (tools_list, deny_list)
        }
    }
}

fn default_tool_action(map: &std::collections::BTreeMap<String, ToolRule>) -> ToolAction {
    match map.get("*") {
        Some(ToolRule::Action(action)) => action.clone(),
        _ => ToolAction::Allow,
    }
}

fn action_for_capability(tools: &ToolsField, capability: &str) -> ToolAction {
    match tools {
        ToolsField::Shorthand(action) => action.clone(),
        ToolsField::Map(map) => match map.get(capability) {
            Some(ToolRule::Action(action)) => action.clone(),
            _ => default_tool_action(map),
        },
    }
}

fn explicit_action_for_capability(tools: &ToolsField, capability: &str) -> Option<ToolAction> {
    match tools {
        ToolsField::Shorthand(_) => None,
        ToolsField::Map(map) => match map.get(capability) {
            Some(ToolRule::Action(action)) => Some(action.clone()),
            _ => None,
        },
    }
}

fn has_wildcard_allow(tools: &ToolsField) -> bool {
    match tools {
        ToolsField::Shorthand(action) => *action != ToolAction::Deny,
        ToolsField::Map(map) => default_tool_action(map) != ToolAction::Deny,
    }
}

fn infer_codex_sandbox_from_tools(tools: Option<&ToolsField>) -> &'static str {
    let Some(tools) = tools else {
        return "read-only";
    };

    let bash_allowed = action_for_capability(tools, "bash") != ToolAction::Deny;
    let edit_allowed = action_for_capability(tools, "edit") != ToolAction::Deny;
    let external_directory_allowed =
        action_for_capability(tools, "external_directory") != ToolAction::Deny;
    let bash_denied = explicit_action_for_capability(tools, "bash") == Some(ToolAction::Deny);
    let edit_denied = explicit_action_for_capability(tools, "edit") == Some(ToolAction::Deny);

    if (external_directory_allowed || has_wildcard_allow(tools)) && !bash_denied && !edit_denied {
        return "danger-full-access";
    }

    if bash_allowed || edit_allowed {
        return "workspace-write";
    }

    "read-only"
}

fn collect_codex_tools_lossiness(
    tools: Option<&ToolsField>,
    lossy: &mut Vec<LossyField>,
    target: &str,
) {
    let Some(tools) = tools else {
        return;
    };

    match tools {
        ToolsField::Shorthand(ToolAction::Ask) => push_lossy(
            lossy,
            "tools",
            target,
            Lossiness::Approximate {
                note: "Codex tools collapse to sandbox-only policy",
            },
        ),
        ToolsField::Shorthand(_) => {}
        ToolsField::Map(map) => {
            for (cap, rule) in map {
                // Wildcard allow/deny map exactly to sandbox semantics — no lossiness.
                if cap == "*" {
                    if let ToolRule::Action(ToolAction::Ask) = rule {
                        push_lossy(
                            lossy,
                            "tools.*",
                            target,
                            Lossiness::Approximate {
                                note: "ask lowered as allow in sandbox inference",
                            },
                        );
                    }
                    continue;
                }
                match rule {
                    ToolRule::Action(ToolAction::Ask) => {
                        push_lossy(
                            lossy,
                            format!("tools.{cap}"),
                            target,
                            Lossiness::Approximate {
                                note: "ask lowered as allow in sandbox inference",
                            },
                        );
                    }
                    ToolRule::Scoped(_) => {
                        push_lossy(lossy, format!("tools.{cap}"), target, Lossiness::Dropped);
                    }
                    _ => {
                        push_lossy(
                            lossy,
                            format!("tools.{cap}"),
                            target,
                            Lossiness::Approximate {
                                note: "Codex tools collapse to sandbox-only policy",
                            },
                        );
                    }
                }
            }
        }
    }
}

/// Capability expansions for Pi — capabilities that map to multiple tool names.
const PI_CAPABILITY_EXPANSIONS: &[(&str, &[&str])] = &[
    ("edit", &["edit", "write"]),
    ("web", &["websearch", "webfetch"]),
];

fn expand_pi_capability(cap: &str) -> Vec<String> {
    PI_CAPABILITY_EXPANSIONS
        .iter()
        .find_map(|(c, expanded)| {
            (*c == cap).then_some(expanded.iter().map(|s| (*s).to_string()).collect())
        })
        .unwrap_or_else(|| vec![cap.to_string()])
}

fn compile_tools_for_pi(
    tools: Option<&ToolsField>,
    lossy: &mut Vec<LossyField>,
    target: &str,
) -> Option<String> {
    let tools = tools?;

    match tools {
        ToolsField::Shorthand(ToolAction::Allow) => None,
        ToolsField::Shorthand(ToolAction::Deny) => Some(String::new()),
        ToolsField::Shorthand(ToolAction::Ask) => {
            push_lossy(
                lossy,
                "tools",
                target,
                Lossiness::Approximate {
                    note: "Pi cannot preserve ask policy; lowered as allow",
                },
            );
            None
        }
        ToolsField::Map(map) => {
            let mut allowed = Vec::new();
            for (cap, rule) in map {
                if cap == "*" {
                    if let ToolRule::Action(ToolAction::Ask) = rule {
                        push_lossy(
                            lossy,
                            "tools.*",
                            target,
                            Lossiness::Approximate {
                                note: "Pi cannot preserve ask policy; lowered as allow",
                            },
                        );
                    }
                    continue;
                }
                match rule {
                    ToolRule::Scoped(_) => {
                        push_lossy(lossy, format!("tools.{cap}"), target, Lossiness::Dropped);
                    }
                    ToolRule::Action(ToolAction::Allow) => {
                        push_unique(&mut allowed, &expand_pi_capability(cap));
                    }
                    ToolRule::Action(ToolAction::Ask) => {
                        push_unique(&mut allowed, &expand_pi_capability(cap));
                        push_lossy(
                            lossy,
                            format!("tools.{cap}"),
                            target,
                            Lossiness::Approximate {
                                note: "Pi cannot preserve ask policy; lowered as allow",
                            },
                        );
                    }
                    ToolRule::Action(ToolAction::Deny) => {}
                }
            }
            (!allowed.is_empty()).then_some(allowed.join(", "))
        }
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
///   model-policies, harness-overrides (claude entry merged before lowering),
///   fanout, legacy-models
///
/// `harness-overrides.claude` values are merged into top-level fields
/// before lowering (D42 — compile-time merge).
pub fn lower_to_claude(profile: &AgentProfile, _fm: &Frontmatter, body: &str) -> LoweredOutput {
    let eff = Effective::new(profile, &HarnessKind::Claude);
    let mut lossy = Vec::new();

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
    // model — exact (alias preserved; Claude resolves it)
    if let Some(model) = &profile.model {
        yaml.insert(yk("model"), yv(model));
    }
    // skills — exact (Claude reads skills natively from .claude/skills/)
    let skills = eff.skills();
    if !skills.is_empty() {
        let seq: serde_yaml::Value =
            serde_yaml::Value::Sequence(skills.iter().map(|s| yv(s)).collect());
        yaml.insert(yk("skills"), seq);
    }
    let (tools_allow, tools_deny) = compile_tools_for_claude(eff.tools(), &mut lossy, "Claude");
    if let Some(tools) = tools_allow {
        let seq: serde_yaml::Value =
            serde_yaml::Value::Sequence(tools.iter().map(|s| yv(s)).collect());
        yaml.insert(yk("tools"), seq);
    }
    if let Some(disallowed) = tools_deny {
        let seq: serde_yaml::Value =
            serde_yaml::Value::Sequence(disallowed.iter().map(|s| yv(s)).collect());
        yaml.insert(yk("disallowed-tools"), seq);
    }

    // mcp-tools — exact (pass through raw from source)
    let mcp = &profile.mcp_tools;
    if !mcp.is_empty() {
        let seq: serde_yaml::Value =
            serde_yaml::Value::Sequence(mcp.iter().map(|s| yv(s)).collect());
        yaml.insert(yk("mcp-tools"), seq);
    }

    // effort — exact (passed as frontmatter hint; Claude reads it)
    if let Some(effort) = eff.effort() {
        yaml.insert(yk("effort"), yv(effort.claude_str()));
    }

    // --- Dropped / meridian-only fields ---
    let target = "Claude";
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
            field: "autocompact-pct".into(),
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
    // harness: field is dropped (the native artifact's location IS the harness)
    // harness-overrides: merged above, then dropped

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
///   mcp-tools (approximate), mode, autocompact, model-policies, fanout
/// - Merged: harness-overrides.codex applied to top-level fields before lowering
pub fn lower_to_codex(profile: &AgentProfile, body: &str) -> LoweredOutput {
    let eff = Effective::new(profile, &HarnessKind::Codex);
    let mut lossy = Vec::new();
    let target = "Codex";

    // Effort — exact (lowered to model_reasoning_effort)
    let effort_str = eff.effort().map(|e| e.as_str());

    // Sandbox — explicit non-default policy wins; otherwise infer from tools
    // only when tools are present. No tools + no sandbox → omit sandbox_mode.
    let sandbox_str = match eff.sandbox() {
        Some(s) if *s != SandboxMode::Default => Some(s.as_str()),
        _ => eff
            .tools()
            .map(|_| infer_codex_sandbox_from_tools(eff.tools())),
    };

    // Approval — exact (lowered to approval_policy)
    let approval_policy = eff.approval().and_then(|a| {
        use crate::compiler::agents::ApprovalMode;
        match a {
            ApprovalMode::Default => None,
            ApprovalMode::Auto => Some("on-request"),
            ApprovalMode::Confirm => Some("untrusted"),
            ApprovalMode::Yolo => Some("never"),
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
    collect_codex_tools_lossiness(eff.tools(), &mut lossy, target);
    if !profile.mcp_tools.is_empty() {
        lossy.push(LossyField {
            field: "mcp-tools".into(),
            target: target.into(),
            classification: Lossiness::Approximate {
                note: "Codex uses -c mcp.servers.<name>.command",
            },
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
            field: "autocompact-pct".into(),
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
        model: profile.model.as_deref(),
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
///   effort, mcp-tools, autocompact)
/// - Meridian-only: model-policies, fanout
pub fn lower_to_opencode(profile: &AgentProfile, body: &str) -> LoweredOutput {
    let eff = Effective::new(profile, &HarnessKind::OpenCode);
    let mut lossy = Vec::new();
    let target = "OpenCode";

    let mut yaml = serde_yaml::Mapping::new();
    let yk = |s: &str| serde_yaml::Value::String(s.to_string());
    let yv = |s: &str| serde_yaml::Value::String(s.to_string());

    if let Some(name) = &profile.name {
        yaml.insert(yk("name"), yv(name));
    }
    if let Some(desc) = &profile.description {
        yaml.insert(yk("description"), yv(desc));
    }
    if let Some(model) = &profile.model {
        // OpenCode uses provider/model format — pass through alias as-is for V0;
        // full resolution requires the model catalog (out of scope for Phase 3).
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
    if eff.effort().is_some() {
        lossy.push(LossyField {
            field: "effort".into(),
            target: target.into(),
            classification: Lossiness::Approximate {
                note: "effort maps to --variant on subprocess only",
            },
        });
    }
    if !profile.mcp_tools.is_empty() {
        lossy.push(LossyField {
            field: "mcp-tools".into(),
            target: target.into(),
            classification: Lossiness::Approximate {
                note: "mcp-tools on subprocess errors; streaming uses session payload",
            },
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
            field: "autocompact-pct".into(),
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

// ---------------------------------------------------------------------------
// Pi native artifact
// ---------------------------------------------------------------------------

/// Lower an agent profile to Pi-native markdown format.
///
/// Pi's format is similar to OpenCode: markdown + YAML frontmatter with a
/// minimal subset of fields. Per agent-compilation-mapping.md §6, all policy
/// fields are dropped.
pub fn lower_to_pi(profile: &AgentProfile, body: &str) -> LoweredOutput {
    let mut lossy = Vec::new();
    let target = "Pi";
    let eff = Effective::new(profile, &HarnessKind::Pi);

    let mut yaml = serde_yaml::Mapping::new();
    let yk = |s: &str| serde_yaml::Value::String(s.to_string());
    let yv = |s: &str| serde_yaml::Value::String(s.to_string());

    if let Some(name) = &profile.name {
        yaml.insert(yk("name"), yv(name));
    }
    if let Some(desc) = &profile.description {
        yaml.insert(yk("description"), yv(desc));
    }
    if let Some(model) = &profile.model {
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

    if let Some(tools) = compile_tools_for_pi(eff.tools(), &mut lossy, target) {
        yaml.insert(yk("tools"), yv(&tools));
    }

    // Everything else is dropped
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
    if eff.effort().is_some() {
        lossy.push(LossyField {
            field: "effort".into(),
            target: target.into(),
            classification: Lossiness::Approximate {
                note: "Pi effort semantics unverified",
            },
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
            field: "autocompact-pct".into(),
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
pub fn lower_for_harness(
    harness: &HarnessKind,
    profile: &AgentProfile,
    fm: &Frontmatter,
    body: &str,
) -> LoweredOutput {
    match harness {
        HarnessKind::Claude => lower_to_claude(profile, fm, body),
        HarnessKind::Codex => lower_to_codex(profile, body),
        HarnessKind::OpenCode => lower_to_opencode(profile, body),
        HarnessKind::Pi => lower_to_pi(profile, body),
    }
}

#[cfg(test)]
mod tests {
    // qa-validated: mars-tools-abstraction
    use super::*;
    use crate::compiler::agents::{AgentDiagnostic, parse_agent_content};

    fn profile_from(content: &str) -> (AgentProfile, Frontmatter, Vec<AgentDiagnostic>) {
        let mut diags = Vec::new();
        let (profile, fm) = parse_agent_content(content, &mut diags).unwrap();
        (profile, fm, diags)
    }

    #[test]
    fn claude_lowering_preserves_supported_fields_and_maps_tools() {
        let content = r#"---
name: coder
description: Code impl agent
model: gpt55
harness: claude
skills: [dev-principles]
tools:
  "*": deny
  bash: allow
  edit: allow
---
# Coder
You write code."#;
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body());
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("name: coder"));
        assert!(text.contains("description: Code impl agent"));
        assert!(text.contains("model: gpt55"));
        assert!(text.contains("skills"));
        assert!(text.contains("Bash"));
        assert!(text.contains("Edit"));
        assert!(text.contains("Write"));
        assert!(text.contains("# Coder"));
    }

    #[test]
    fn claude_lowering_drops_non_native_fields_and_reports_lossiness() {
        let content = r#"---
name: coder
harness: claude
approval: auto
sandbox: read-only
mode: subagent
autocompact: 50
autocompact-pct: 80
tools:
  "*": deny
  bash: ask
  read:
    "*.env": allow
model-policies:
  - match:
      model: gpt55
    override:
      harness: codex
fanout:
  - alias: opus
---
# Body"#;
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body());
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(!text.contains("approval:"));
        assert!(!text.contains("sandbox:"));
        assert!(!text.contains("autocompact:"));
        assert!(!text.contains("model-policies:"));
        assert!(!text.contains("fanout:"));
        assert!(out.lossy_fields.iter().any(|lf| {
            lf.field == "tools.bash" && matches!(lf.classification, Lossiness::Approximate { .. })
        }));
        assert!(out.lossy_fields.iter().any(|lf| {
            lf.field == "tools.read" && matches!(lf.classification, Lossiness::Dropped)
        }));
        for field in [
            "approval",
            "sandbox",
            "mode",
            "autocompact",
            "autocompact-pct",
            "model-policies",
            "fanout",
        ] {
            assert!(out.lossy_fields.iter().any(|lf| lf.field == field));
        }
    }

    #[test]
    fn claude_harness_override_applied_before_lowering() {
        let content = r#"---
name: r
harness: claude
skills: [base-skill]
harness-overrides:
  claude:
    skills: [override-skill]
---
# body"#;
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body());
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("override-skill"));
        assert!(!text.contains("base-skill"));
    }

    #[test]
    fn codex_lowering_produces_parseable_top_level_toml() {
        let content = r#"---
name: explorer
description: "Line one
Line two"
model: gpt55
harness: codex
effort: high
sandbox: workspace-write
approval: yolo
---
# Explore
Use "quotes" and backslashes \
Keep going."#;
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body());
        let text = String::from_utf8(out.bytes).unwrap();
        let parsed: toml::Value = toml::from_str(&text).expect("lowered TOML should parse");

        assert!(parsed.get("agent").is_none());
        assert_eq!(
            parsed.get("name").and_then(|v| v.as_str()),
            Some("explorer")
        );
        assert_eq!(
            parsed
                .get("model_reasoning_effort")
                .and_then(|v| v.as_str()),
            Some("high")
        );
        assert_eq!(
            parsed.get("sandbox_mode").and_then(|v| v.as_str()),
            Some("workspace-write")
        );
        assert_eq!(
            parsed.get("approval_policy").and_then(|v| v.as_str()),
            Some("never")
        );
        assert_eq!(
            parsed
                .get("developer_instructions")
                .and_then(|v| v.as_str())
                .unwrap_or_default(),
            "# Explore\nUse \"quotes\" and backslashes \\\nKeep going."
        );
    }

    #[test]
    fn codex_sandbox_inference_matches_behavioral_cases() {
        let cases = [
            ("---\nname: r\nharness: codex\n---\n# body", None),
            (
                "---\nname: r\nharness: codex\ntools:\n  \"*\": allow\n---\n# body",
                Some("danger-full-access"),
            ),
            (
                "---\nname: r\nharness: codex\ntools:\n  \"*\": allow\n  bash: deny\n---\n# body",
                Some("workspace-write"),
            ),
            (
                "---\nname: r\nharness: codex\nsandbox: default\n---\n# body",
                None,
            ),
        ];

        for (content, expected_sandbox) in cases {
            let (profile, fm, _) = profile_from(content);
            let out = lower_to_codex(&profile, fm.body());
            let text = String::from_utf8(out.bytes).unwrap();
            let parsed: toml::Value = toml::from_str(&text).expect("lowered TOML should parse");
            assert_eq!(
                parsed.get("sandbox_mode").and_then(|v| v.as_str()),
                expected_sandbox,
                "unexpected sandbox inference for content:
{content}
{text}"
            );
        }
    }

    #[test]
    fn codex_harness_override_replaces_top_level_tools_and_fields() {
        let content = r#"---
name: r
harness: codex
tools:
  "*": deny
  bash: allow
harness-overrides:
  codex:
    effort: high
    sandbox: read-only
    tools:
      "*": deny
      read: allow
---
# body"#;
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body());
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("model_reasoning_effort = \"high\""));
        assert!(text.contains("sandbox_mode = \"read-only\""));
        assert!(out.lossy_fields.iter().any(|lf| {
            lf.field == "tools.read" && matches!(lf.classification, Lossiness::Approximate { .. })
        }));
        assert!(!out.lossy_fields.iter().any(|lf| lf.field == "tools.bash"));
    }

    #[test]
    fn codex_tools_lossiness_includes_ask_and_scoped_rules() {
        let content = r#"---
name: r
harness: codex
tools:
  "*": deny
  bash: ask
  read:
    "*.env": allow
---
# body"#;
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body());
        assert!(out.lossy_fields.iter().any(|lf| {
            lf.field == "tools.bash" && matches!(lf.classification, Lossiness::Approximate { .. })
        }));
        assert!(out.lossy_fields.iter().any(|lf| {
            lf.field == "tools.read" && matches!(lf.classification, Lossiness::Dropped)
        }));
    }

    #[test]
    fn opencode_lowering_preserves_supported_fields_and_omits_tools() {
        let content = r#"---
name: r
description: Reviewer
model: gpt55
mode: primary
harness: opencode
tools:
  "*": deny
  bash: allow
---
# Reviewer
body"#;
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_opencode(&profile, fm.body());
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("name: r"));
        assert!(text.contains("description: Reviewer"));
        assert!(text.contains("model: gpt55"));
        assert!(text.contains("mode: primary"));
        assert!(!text.contains("tools:"));
        assert!(!text.contains("disallowed-tools"));
    }

    #[test]
    fn pi_lowering_expands_tools_and_reports_lossiness() {
        let content = r#"---
name: pi-agent
description: Pi agent
model: gpt55
mode: subagent
harness: pi
tools:
  "*": deny
  edit: allow
  web: ask
  read:
    "*.env": allow
---
# body"#;
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_pi(&profile, fm.body());
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("description: Pi agent"));
        assert!(text.contains("mode: subagent"));
        assert!(text.contains("tools: edit, write, websearch, webfetch"));
        assert!(out.lossy_fields.iter().any(|lf| {
            lf.field == "tools.web" && matches!(lf.classification, Lossiness::Approximate { .. })
        }));
        assert!(out.lossy_fields.iter().any(|lf| {
            lf.field == "tools.read" && matches!(lf.classification, Lossiness::Dropped)
        }));
    }

    #[test]
    fn lower_for_harness_dispatches_to_native_formats() {
        let (claude_profile, claude_fm, _) = profile_from(
            "---
name: coder
model: gpt55
harness: claude
---
# body",
        );
        let claude = lower_for_harness(
            &HarnessKind::Claude,
            &claude_profile,
            &claude_fm,
            claude_fm.body(),
        );
        let claude_text = String::from_utf8(claude.bytes).unwrap();
        assert!(claude_text.contains("---"));

        let (codex_profile, codex_fm, _) = profile_from(
            "---
name: coder
model: gpt55
harness: codex
---
# body",
        );
        let codex = lower_for_harness(
            &HarnessKind::Codex,
            &codex_profile,
            &codex_fm,
            codex_fm.body(),
        );
        let codex_text = String::from_utf8(codex.bytes).unwrap();
        assert!(codex_text.contains("name = \"coder\""));
        assert!(!codex_text.contains("[agent]"));
    }
}
