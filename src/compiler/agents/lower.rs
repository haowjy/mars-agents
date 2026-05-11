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

fn infer_codex_sandbox_from_tools(tools: Option<&ToolsField>) -> &'static str {
    let Some(tools) = tools else {
        return "read-only";
    };

    let ext = action_for_capability(tools, "external_directory");
    if ext != ToolAction::Deny {
        return "danger-full-access";
    }

    if let ToolsField::Map(map) = tools {
        let default = default_tool_action(map);
        if default != ToolAction::Deny {
            let bash = action_for_capability(tools, "bash");
            let edit = action_for_capability(tools, "edit");
            if bash != ToolAction::Deny || edit != ToolAction::Deny {
                return "danger-full-access";
            }
        }
    }

    let bash = action_for_capability(tools, "bash");
    let edit = action_for_capability(tools, "edit");
    if bash != ToolAction::Deny || edit != ToolAction::Deny {
        "workspace-write"
    } else {
        "read-only"
    }
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
        let content = "---\nname: coder\ndescription: Code impl agent\nmodel: gpt55\nharness: claude\nskills: [dev-principles]\ntools:\n  \"*\": deny\n  bash: allow\n  edit: allow\n---\n# Coder\nYou write code.";
        let (profile, fm, _) = profile_from(content);
        let body = fm.body();
        let out = lower_to_claude(&profile, &fm, body);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("name: coder"), "name missing: {text}");
        assert!(
            text.contains("description: Code impl agent"),
            "desc missing"
        );
        assert!(text.contains("model: gpt55"), "model missing");
        assert!(text.contains("skills"), "skills missing");
        assert!(text.contains("tools"), "tools missing");
        assert!(text.contains("Bash"), "bash missing");
        assert!(text.contains("Edit"), "edit missing");
        assert!(text.contains("Write"), "write missing");
        assert!(text.contains("# Coder"), "body missing");
    }

    #[test]
    fn claude_tools_ask_is_approximate_and_scoped_is_dropped() {
        let content = "---\nname: coder\nharness: claude\ntools:\n  \"*\": deny\n  bash: ask\n  read:\n    \"*.env\": allow\n---\n# Body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body());
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("Bash"), "ask should lower to allow");
        assert!(!text.contains("*.env"), "scoped rule should be dropped");
        assert!(out.lossy_fields.iter().any(|lf| {
            lf.field == "tools.bash" && matches!(lf.classification, Lossiness::Approximate { .. })
        }));
        assert!(out.lossy_fields.iter().any(|lf| {
            lf.field == "tools.read" && matches!(lf.classification, Lossiness::Dropped)
        }));
    }

    #[test]
    fn claude_tools_map_default_allow_emits_disallowed_tools() {
        let content =
            "---\nname: r\nharness: claude\ntools:\n  \"*\": allow\n  task: deny\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body());
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(!text.contains("tools: []"), "should not emit allowlist");
        assert!(
            text.contains("disallowed-tools"),
            "deny list should be emitted"
        );
        assert!(text.contains("Agent"), "task deny should map to Agent");
    }

    #[test]
    fn claude_lowering_drops_approval_sandbox_mode_autocompact() {
        let content = "---\nname: coder\nharness: claude\napproval: auto\nsandbox: read-only\nmode: subagent\nautocompact: 50\nautocompact-pct: 80\n---\n# Body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body());
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
            dropped.contains(&"autocompact-pct"),
            "autocompact-pct not in lossy: {dropped:?}"
        );
    }

    #[test]
    fn claude_harness_override_applied_before_lowering() {
        let content = "---\nname: r\nharness: claude\nskills: [base-skill]\nharness-overrides:\n  claude:\n    skills: [override-skill]\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body());
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            text.contains("override-skill"),
            "override not applied: {text}"
        );
        assert!(
            !text.contains("base-skill"),
            "base skill not overridden: {text}"
        );
    }

    #[test]
    fn claude_meridian_only_fields_dropped() {
        let content = "---\nname: r\nharness: claude\nmodel-policies:\n  - match:\n      model: gpt55\n    override:\n      harness: codex\nfanout:\n  - alias: opus\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body());
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

    // --- 3.3: Codex lowering ---

    #[test]
    fn codex_lowering_produces_top_level_toml() {
        let content = "---\nname: coder\ndescription: Code agent\nmodel: gpt55\nharness: codex\neffort: high\nsandbox: workspace-write\napproval: auto\n---\n# Coder\nYou code.";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body());
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
    fn codex_lowering_drops_skills_and_inferrs_sandbox_from_tools() {
        let content = "---\nname: r\nharness: codex\nskills: [review]\ntools:\n  \"*\": deny\n  bash: allow\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body());
        let approx: Vec<_> = out
            .lossy_fields
            .iter()
            .filter(|f| matches!(f.classification, Lossiness::Approximate { .. }))
            .map(|f| f.field.as_str())
            .collect();
        assert!(approx.contains(&"tools.bash"));
        assert!(
            out.lossy_fields
                .iter()
                .any(|f| { f.field == "skills" && matches!(f.classification, Lossiness::Dropped) })
        );

        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            text.contains("sandbox_mode = \"workspace-write\""),
            "sandbox should be inferred from bash allow: {text}"
        );
    }

    #[test]
    fn codex_harness_override_applied() {
        let content = "---\nname: r\nharness: codex\ntools:\n  \"*\": deny\n  bash: allow\nharness-overrides:\n  codex:\n    effort: high\n    sandbox: read-only\n    tools:\n      \"*\": deny\n      read: allow\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body());
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            text.contains("model_reasoning_effort = \"high\""),
            "override not applied: {text}"
        );
        assert!(
            text.contains("sandbox_mode = \"read-only\""),
            "sandbox override not applied: {text}"
        );
        assert!(
            out.lossy_fields.iter().any(|lf| lf.field == "tools.read"
                && matches!(lf.classification, Lossiness::Approximate { .. })),
            "override tools should replace top-level tools"
        );
        assert!(
            !out.lossy_fields.iter().any(|lf| lf.field == "tools.bash"),
            "top-level tools should be replaced by override tools"
        );
    }

    #[test]
    fn codex_infers_danger_full_access_when_default_allow_and_no_write_denies() {
        let content = "---\nname: r\nharness: codex\ntools:\n  \"*\": allow\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body());
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            text.contains("sandbox_mode = \"danger-full-access\""),
            "sandbox should infer danger-full-access: {text}"
        );
    }

    #[test]
    fn codex_lowering_multiline_instructions_are_parseable() {
        let content = "---\nname: explorer\ndescription: \"Line one\\nLine two\"\nharness: codex\napproval: yolo\n---\n# Explore\nUse \"quotes\" and backslashes \\\\\nKeep going.";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body());
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
        let out = lower_to_opencode(&profile, fm.body());
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("name: r"), "name missing");
        assert!(text.contains("description: Reviewer"), "desc missing");
        assert!(text.contains("model: gpt55"), "model missing");
        assert!(text.contains("mode: primary"), "mode missing");
    }

    #[test]
    fn opencode_lowering_omits_tools_fields() {
        let content =
            "---\nname: r\nharness: opencode\ntools:\n  \"*\": deny\n  bash: allow\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_opencode(&profile, fm.body());
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(!text.contains("tools:"), "tools must not be emitted");
        assert!(
            !text.contains("disallowed-tools"),
            "legacy field must not appear"
        );
    }

    // --- 3.3: Pi lowering ---

    #[test]
    fn pi_lowering_preserves_name_description_model() {
        let content = "---\nname: pi-agent\ndescription: Pi agent\nmodel: gpt55\nharness: pi\n---\n# Pi\nbody";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_pi(&profile, fm.body());
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("name: pi-agent"), "name missing");
        assert!(text.contains("description: Pi agent"), "desc missing");
    }

    #[test]
    fn pi_lowering_emits_tools_and_flags_ask_and_scoped() {
        let content = "---\nname: pi-agent\nharness: pi\ntools:\n  \"*\": deny\n  bash: allow\n  web: ask\n  read:\n    \"*.env\": allow\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_pi(&profile, fm.body());
        let text = String::from_utf8(out.bytes).unwrap();
        // web expands to websearch, webfetch
        assert!(
            text.contains("tools: bash, websearch, webfetch"),
            "tools list missing: {text}"
        );
        assert!(out.lossy_fields.iter().any(|lf| {
            lf.field == "tools.web" && matches!(lf.classification, Lossiness::Approximate { .. })
        }));
        assert!(out.lossy_fields.iter().any(|lf| {
            lf.field == "tools.read" && matches!(lf.classification, Lossiness::Dropped)
        }));
    }

    // --- 3.3: Dispatch ---

    #[test]
    fn codex_no_tools_no_sandbox_omits_sandbox_mode() {
        let content = "---\nname: r\nharness: codex\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body());
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("sandbox_mode"),
            "sandbox_mode should be omitted when no tools and no sandbox: {text}"
        );
    }

    #[test]
    fn codex_sandbox_default_no_tools_omits_sandbox_mode() {
        let content = "---\nname: r\nharness: codex\nsandbox: default\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body());
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("sandbox_mode"),
            "sandbox: default with no tools should omit sandbox_mode: {text}"
        );
    }

    #[test]
    fn codex_wildcard_only_allow_no_extra_lossiness() {
        let content = "---\nname: r\nharness: codex\ntools:\n  \"*\": allow\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body());
        // Wildcard allow/deny maps exactly to sandbox — no Approximate entries
        let approx: Vec<_> = out
            .lossy_fields
            .iter()
            .filter(|f| matches!(f.classification, Lossiness::Approximate { .. }))
            .collect();
        assert!(
            approx.is_empty(),
            "wildcard-only allow should not produce Approximate lossiness: {approx:?}"
        );
    }

    #[test]
    fn pi_lowering_expands_edit_and_web_capabilities() {
        let content = "---\nname: pi-agent\nharness: pi\ntools:\n  \"*\": deny\n  edit: allow\n  web: allow\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_pi(&profile, fm.body());
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            text.contains("edit, write"),
            "edit should expand to edit, write: {text}"
        );
        assert!(
            text.contains("websearch, webfetch"),
            "web should expand to websearch, webfetch: {text}"
        );
    }

    #[test]
    fn lower_for_harness_dispatches_correctly() {
        let content = "---\nname: coder\nmodel: gpt55\nharness: claude\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let body = fm.body().to_string();
        let out = lower_for_harness(&HarnessKind::Claude, &profile, &fm, &body);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("---"), "not markdown format");

        let content2 = "---\nname: coder\nmodel: gpt55\nharness: codex\n---\n# body";
        let (profile2, fm2, _) = profile_from(content2);
        let body2 = fm2.body().to_string();
        let out2 = lower_for_harness(&HarnessKind::Codex, &profile2, &fm2, &body2);
        let text2 = String::from_utf8(out2.bytes).unwrap();
        assert!(text2.contains("name = \"coder\""), "not TOML format");
        assert!(
            !text2.contains("[agent]"),
            "legacy nested agent table emitted"
        );
    }
}
