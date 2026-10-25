//! Declarative per-harness agent lowering policies and shared pipeline.
use crate::compiler::agents::{AgentProfile, HarnessKind};
use crate::compiler::harness_descriptor::{self, AgentLoweringPolicyKind};
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
///
/// Launch-time fields (approval, sandbox, network, mode where not emitted, autocompact, …) are
/// classified [`Lossiness::MeridianOnly`] — Meridian enforces them at spawn, so omitting
/// them from native artifacts is not a behavioral loss. Target-enforced gaps stay
/// [`Lossiness::Dropped`] or [`Lossiness::Approximate`] and warn loudly. One exception:
/// Codex lowers `network` as [`Lossiness::Dropped`], because its native artifact carries
/// `sandbox_mode`, so a natively run Codex agent keeps Codex's own network default.
pub use crate::compiler::lossiness::{Lossiness, LossyField, LoweredOutput};
pub use crate::compiler::tool_policy::EffectiveToolPolicy;
use crate::compiler::tool_policy::{PolicyProjection, RuleKind, ToolAction};
use crate::frontmatter::{Frontmatter, SkillsSpec};

// ---------------------------------------------------------------------------
// Effective field access for target lowering
// ---------------------------------------------------------------------------

/// Effective field values read from top-level Mars semantics.
struct Effective<'a> {
    harness: HarnessKind,
    profile: &'a AgentProfile,
    tools: &'a EffectiveToolPolicy,
}

impl<'a> Effective<'a> {
    fn new(profile: &'a AgentProfile, harness: HarnessKind) -> Self {
        Self {
            harness,
            profile,
            tools: &profile.tools,
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

    fn network(&self) -> Option<&crate::compiler::agents::NetworkPolicy> {
        self.profile.network.as_ref()
    }

    fn has_network_allow_hosts(&self) -> bool {
        !self.profile.network_allow.entries().is_empty()
    }

    fn has_network_deny_hosts(&self) -> bool {
        !self.profile.network_deny.entries().is_empty()
    }

    fn skills(&self) -> &SkillsSpec {
        self.profile.effective_skills(&self.harness)
    }

    /// Whether the policy grants or asks about built-in tools, which only an allowlist
    /// or a native `tools:` field can carry.
    fn has_tool_allowance(&self) -> bool {
        self.tools.is_allowlist()
            || self.tools.has_rule(ToolAction::Allow, RuleKind::Builtin)
            || self.has_ask()
    }

    fn has_tool_denial(&self) -> bool {
        self.tools.has_rule(ToolAction::Deny, RuleKind::Builtin)
    }

    fn has_ask(&self) -> bool {
        self.tools.with_action(ToolAction::Ask).next().is_some()
    }

    fn has_mcp_policy(&self) -> bool {
        self.tools.rules.iter().any(|rule| rule.is_mcp())
    }

    fn autocompact_pct(&self) -> Option<u8> {
        self.profile.autocompact_pct
    }

    fn native_config(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.profile.effective_native_config(&self.harness)
    }
}

/// Native spellings of every `action` rule, recording a lossiness entry for each rule the
/// harness cannot name.
fn native_tools_for_action(
    projection: &PolicyProjection<'_>,
    action: ToolAction,
    field: &'static str,
    target: &str,
    lossy: &mut Vec<LossyField>,
) -> Vec<String> {
    for note in projection.gaps(action, RuleKind::All) {
        lossy.push(LossyField {
            field: field.into(),
            target: target.into(),
            classification: Lossiness::Approximate { note },
        });
    }
    projection.natives(action, RuleKind::All)
}

fn record_native_mcp_lossiness(
    eff: &Effective<'_>,
    target: &str,
    note: &'static str,
    lossy: &mut Vec<LossyField>,
) {
    if eff.has_mcp_policy() {
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
/// - Preserved: name, description, model, tools, disallowed-tools, body
/// - Skills: load is native-preloaded; body-guided channels are approximate
/// - Meridian-only (launch-time): approval, sandbox, network, mode, autocompact,
///   autocompact_pct, model-policies, fanout
/// - Dropped: harness, harness-overrides, legacy-models
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

#[derive(Debug, Clone, Copy)]
enum AgentMarkdownField {
    Identity,
    Model,
    ClaudeTools,
    ClaudeEffort,
    Mode,
}

#[derive(Debug, Clone, Copy)]
enum AgentLossinessStep {
    Approval,
    Sandbox,
    ModeDropped,
    ToolsDropped,
    DisallowedToolsDropped,
    EffortApproximate(&'static str),
    McpApproximate(&'static str),
    McpDropped,
    MeridianOnlyFields,
    InvocationAxes,
}

#[derive(Debug, Clone, Copy)]
struct AgentLoweringPolicy {
    target_name: &'static str,
    description: DescriptionPolicy,
    mode_note: Option<&'static str>,
    fields: &'static [AgentMarkdownField],
    lossiness: &'static [AgentLossinessStep],
}

#[derive(Debug, Clone, Copy)]
enum AgentSkillsPolicy {
    NativePreload,
    NativePreloadWithReminder,
    BodyGuidance,
}

impl AgentSkillsPolicy {
    fn native_preload(self) -> bool {
        !matches!(self, Self::BodyGuidance)
    }

    fn load_guidance(self) -> bool {
        !matches!(self, Self::NativePreload)
    }
}

fn agent_skills_policy(harness: HarnessKind) -> AgentSkillsPolicy {
    match harness {
        // Claude ignores `skills:` when this file is selected with `claude --agent`.
        HarnessKind::Claude => AgentSkillsPolicy::NativePreloadWithReminder,
        HarnessKind::Cursor => AgentSkillsPolicy::NativePreload,
        HarnessKind::Codex | HarnessKind::OpenCode | HarnessKind::Pi => {
            AgentSkillsPolicy::BodyGuidance
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum DescriptionPolicy {
    Preserve,
    CursorOneLine,
}

const CLAUDE_AGENT_POLICY: AgentLoweringPolicy = AgentLoweringPolicy {
    target_name: "Claude",
    description: DescriptionPolicy::Preserve,
    mode_note: None,
    fields: &[
        AgentMarkdownField::Identity,
        AgentMarkdownField::Model,
        AgentMarkdownField::ClaudeTools,
        AgentMarkdownField::ClaudeEffort,
    ],
    lossiness: &[
        AgentLossinessStep::Approval,
        AgentLossinessStep::Sandbox,
        AgentLossinessStep::ModeDropped,
        AgentLossinessStep::MeridianOnlyFields,
        AgentLossinessStep::InvocationAxes,
    ],
};

const OPENCODE_AGENT_POLICY: AgentLoweringPolicy = AgentLoweringPolicy {
    target_name: "OpenCode",
    description: DescriptionPolicy::Preserve,
    mode_note: None,
    fields: &[
        AgentMarkdownField::Identity,
        AgentMarkdownField::Model,
        AgentMarkdownField::Mode,
    ],
    lossiness: &[
        AgentLossinessStep::Approval,
        AgentLossinessStep::Sandbox,
        AgentLossinessStep::ToolsDropped,
        AgentLossinessStep::DisallowedToolsDropped,
        AgentLossinessStep::EffortApproximate("effort maps to --variant on subprocess only"),
        AgentLossinessStep::McpApproximate(
            "MCP grants on subprocess errors; streaming uses session payload",
        ),
        AgentLossinessStep::MeridianOnlyFields,
        AgentLossinessStep::InvocationAxes,
    ],
};

const CURSOR_AGENT_POLICY: AgentLoweringPolicy = AgentLoweringPolicy {
    target_name: "Cursor",
    description: DescriptionPolicy::CursorOneLine,
    mode_note: Some("Cursor may use the same mode concept"),
    fields: &[
        AgentMarkdownField::Identity,
        AgentMarkdownField::Model,
        AgentMarkdownField::Mode,
    ],
    lossiness: &[
        AgentLossinessStep::Approval,
        AgentLossinessStep::Sandbox,
        AgentLossinessStep::ToolsDropped,
        AgentLossinessStep::DisallowedToolsDropped,
        AgentLossinessStep::EffortApproximate("effort maps to --variant on subprocess only"),
        AgentLossinessStep::McpApproximate(
            "MCP grants on subprocess errors; streaming uses session payload",
        ),
        AgentLossinessStep::MeridianOnlyFields,
        AgentLossinessStep::InvocationAxes,
    ],
};

const PI_AGENT_POLICY: AgentLoweringPolicy = AgentLoweringPolicy {
    target_name: "Pi",
    description: DescriptionPolicy::Preserve,
    mode_note: Some("Pi may use the same mode concept"),
    fields: &[
        AgentMarkdownField::Identity,
        AgentMarkdownField::Model,
        AgentMarkdownField::Mode,
    ],
    lossiness: &[
        AgentLossinessStep::Approval,
        AgentLossinessStep::Sandbox,
        AgentLossinessStep::ToolsDropped,
        AgentLossinessStep::DisallowedToolsDropped,
        AgentLossinessStep::EffortApproximate("Pi effort semantics unverified"),
        AgentLossinessStep::McpDropped,
        AgentLossinessStep::MeridianOnlyFields,
        AgentLossinessStep::InvocationAxes,
    ],
};

fn yk(s: &str) -> serde_yaml::Value {
    serde_yaml::Value::String(s.to_string())
}

fn yv(s: &str) -> serde_yaml::Value {
    serde_yaml::Value::String(s.to_string())
}

struct AgentLoweringCtx<'a> {
    harness: HarnessKind,
    policy: &'static AgentLoweringPolicy,
    profile: &'a AgentProfile,
    model_field: &'a NativeModel,
    eff: Effective<'a>,
    body: String,
    native_skills: Vec<String>,
    yaml: serde_yaml::Mapping,
    lossy: Vec<LossyField>,
}

impl<'a> AgentLoweringCtx<'a> {
    fn new(
        harness: HarnessKind,
        policy: &'static AgentLoweringPolicy,
        profile: &'a AgentProfile,
        body: &str,
        model_field: &'a NativeModel,
    ) -> Self {
        let eff = Effective::new(profile, harness);
        let lowered_skills = lower_agent_skills(eff.skills(), agent_skills_policy(harness), body);
        Self {
            harness,
            policy,
            profile,
            model_field,
            eff,
            body: lowered_skills.body,
            native_skills: lowered_skills.native_preload,
            yaml: serde_yaml::Mapping::new(),
            lossy: Vec::new(),
        }
    }

    fn apply_field(&mut self, field: AgentMarkdownField) {
        match field {
            AgentMarkdownField::Identity => self.insert_identity(),
            AgentMarkdownField::Model => self.insert_model(),
            AgentMarkdownField::ClaudeTools => self.insert_claude_tools(),
            AgentMarkdownField::ClaudeEffort => self.insert_claude_effort(),
            AgentMarkdownField::Mode => self.insert_mode(),
        }
    }

    fn insert_identity(&mut self) {
        if let Some(name) = &self.profile.name {
            self.yaml.insert(yk("name"), yv(name));
        }
        if let Some(desc) = &self.profile.description {
            let rendered = match self.policy.description {
                DescriptionPolicy::Preserve => desc.clone(),
                DescriptionPolicy::CursorOneLine => normalize_cursor_description(desc),
            };
            self.yaml.insert(yk("description"), yv(&rendered));
        }
    }

    fn insert_model(&mut self) {
        if let Some(model) = self.model_field.resolve(self.profile) {
            self.yaml.insert(yk("model"), yv(model));
        }
    }

    fn insert_skills(&mut self) {
        if !self.native_skills.is_empty() {
            let seq = serde_yaml::Value::Sequence(
                self.native_skills.iter().map(|skill| yv(skill)).collect(),
            );
            self.yaml.insert(yk("skills"), seq);
        }
    }

    fn insert_claude_tools(&mut self) {
        let target = self.policy.target_name;
        let projection = self.eff.tools.project(self.harness);
        let allowed = native_tools_for_action(
            &projection,
            ToolAction::Allow,
            "tools",
            target,
            &mut self.lossy,
        );
        let disallowed = native_tools_for_action(
            &projection,
            ToolAction::Deny,
            "disallowed-tools",
            target,
            &mut self.lossy,
        );
        let overlaps = projection.overlaps();

        // An agent file's `tools:` restricts the agent to the tools it lists, and leaving it out
        // inherits every tool. So only an allowlist can lower to it: a map-form grant
        // (`tools: {bash: allow}`) says "allow this", not "only this", and emitting it would
        // strip the agent of every tool the map did not name.
        let allowlist = self.eff.tools.is_allowlist();
        if allowlist && !allowed.is_empty() {
            self.yaml.insert(
                yk("tools"),
                serde_yaml::Value::Sequence(allowed.iter().map(|tool| yv(tool)).collect()),
            );
        }
        if !disallowed.is_empty() {
            self.yaml.insert(
                yk("disallowed-tools"),
                serde_yaml::Value::Sequence(disallowed.iter().map(|tool| yv(tool)).collect()),
            );
        }

        // What the file cannot hold: `ask`, and a deny-all allowlist with nothing allowed (an
        // omitted `tools:` inherits every tool). A map-form grant is not lossy: omitting
        // `tools:` already leaves the granted tool available.
        if self.eff.has_ask() {
            self.record_if_present("tools (ask)", true, Lossiness::Dropped);
        }
        if allowlist && allowed.is_empty() {
            self.record_if_present("tools (allowlist)", true, Lossiness::Dropped);
        }
        if !overlaps.is_empty() {
            self.lossy.push(LossyField {
                field: format!("tools ({} both allowed and denied)", overlaps.join(", ")),
                target: target.into(),
                classification: Lossiness::Approximate {
                    note: "Claude applies the deny",
                },
            });
        }
    }

    fn insert_claude_effort(&mut self) {
        if let Some(effort) = self.eff.effort() {
            self.yaml.insert(yk("effort"), yv(effort.claude_str()));
        }
    }

    fn insert_mode(&mut self) {
        if let Some(mode) = &self.profile.mode {
            self.yaml.insert(yk("mode"), yv(mode.as_str()));
            if let Some(note) = self.policy.mode_note {
                self.lossy.push(LossyField {
                    field: "mode".into(),
                    target: self.policy.target_name.into(),
                    classification: Lossiness::Approximate { note },
                });
            }
        }
    }

    fn apply_lossiness(&mut self, step: AgentLossinessStep) {
        match step {
            AgentLossinessStep::Approval => self.record_approval(),
            AgentLossinessStep::Sandbox => self.record_sandbox(),
            AgentLossinessStep::ModeDropped => {
                self.record_if_present("mode", self.profile.mode.is_some(), Lossiness::MeridianOnly)
            }
            AgentLossinessStep::ToolsDropped => {
                self.record_if_present("tools", self.eff.has_tool_allowance(), Lossiness::Dropped)
            }
            AgentLossinessStep::DisallowedToolsDropped => self.record_if_present(
                "disallowed-tools",
                self.eff.has_tool_denial(),
                Lossiness::Dropped,
            ),
            AgentLossinessStep::EffortApproximate(note) => {
                if self.eff.effort().is_some() {
                    self.lossy.push(LossyField {
                        field: "effort".into(),
                        target: self.policy.target_name.into(),
                        classification: Lossiness::Approximate { note },
                    });
                }
            }
            AgentLossinessStep::McpApproximate(note) => record_native_mcp_lossiness(
                &self.eff,
                self.policy.target_name,
                note,
                &mut self.lossy,
            ),
            AgentLossinessStep::McpDropped => {
                if self.eff.has_mcp_policy() {
                    self.lossy.push(LossyField {
                        field: "mcp".into(),
                        target: self.policy.target_name.into(),
                        classification: Lossiness::Dropped,
                    });
                }
            }
            AgentLossinessStep::MeridianOnlyFields => self.record_meridian_only_fields(),
            AgentLossinessStep::InvocationAxes => record_agent_invocation_lossiness(
                self.profile,
                self.policy.target_name,
                &mut self.lossy,
            ),
        }
    }

    fn record_approval(&mut self) {
        let Some(_) = self.eff.approval() else {
            return;
        };
        let classification = if self.harness == HarnessKind::Cursor {
            // Cursor maps approval to CLI flags — approximate, target-enforced gap.
            Lossiness::Approximate {
                note: "auto maps to --force, yolo to --yolo; confirm has no Cursor equivalent and falls back to default",
            }
        } else {
            // Launch-time: Meridian applies approval at spawn via harness projection.
            Lossiness::MeridianOnly
        };
        self.lossy.push(LossyField {
            field: "approval".into(),
            target: self.policy.target_name.into(),
            classification,
        });
    }

    fn record_sandbox(&mut self) {
        let Some(_) = self.eff.sandbox() else {
            return;
        };
        let classification = if self.harness == HarnessKind::Cursor {
            // Cursor sandbox is a coarse enabled/disabled mapping — target-enforced gap.
            Lossiness::Approximate {
                note: "Cursor only supports enabled/disabled; workspace-write and danger-full-access both map to disabled",
            }
        } else {
            // Launch-time: Meridian applies sandbox at spawn via harness projection.
            Lossiness::MeridianOnly
        };
        self.lossy.push(LossyField {
            field: "sandbox".into(),
            target: self.policy.target_name.into(),
            classification,
        });
    }

    fn record_if_present(&mut self, field: &str, present: bool, classification: Lossiness) {
        if present {
            self.lossy.push(LossyField {
                field: field.into(),
                target: self.policy.target_name.into(),
                classification,
            });
        }
    }

    fn record_meridian_only_fields(&mut self) {
        self.record_if_present(
            "network",
            self.eff.network().is_some(),
            Lossiness::MeridianOnly,
        );
        self.record_if_present(
            "network-allow",
            self.eff.has_network_allow_hosts(),
            Lossiness::MeridianOnly,
        );
        self.record_if_present(
            "network-deny",
            self.eff.has_network_deny_hosts(),
            Lossiness::MeridianOnly,
        );
        self.record_if_present(
            "autocompact",
            self.profile.autocompact.is_some(),
            Lossiness::MeridianOnly,
        );
        self.record_if_present(
            "autocompact_pct",
            self.eff.autocompact_pct().is_some(),
            Lossiness::MeridianOnly,
        );
        self.record_if_present(
            "model-policies",
            !self.profile.model_policies.is_empty(),
            Lossiness::MeridianOnly,
        );
        self.record_if_present(
            "fanout",
            !self.profile.fanout.is_empty(),
            Lossiness::MeridianOnly,
        );
        self.record_if_present(
            "native-config",
            self.eff.native_config().is_some(),
            Lossiness::MeridianOnly,
        );
    }

    fn finish(mut self) -> LoweredOutput {
        // Skill emission follows the harness capability policy, independent of the
        // target-specific field list above.
        self.insert_skills();
        LoweredOutput {
            bytes: render_markdown(self.yaml, &self.body),
            lossy_fields: self.lossy,
            siblings: Vec::new(),
        }
    }
}

struct LoweredAgentSkills {
    native_preload: Vec<String>,
    body: String,
}

fn lower_agent_skills(
    skills: &SkillsSpec,
    policy: AgentSkillsPolicy,
    body: &str,
) -> LoweredAgentSkills {
    let native_preload = if policy.native_preload() {
        skills.load.clone()
    } else {
        Vec::new()
    };

    // A skill named in both channels is loaded, matching launch-bundle semantics.
    let available = skills
        .available
        .iter()
        .filter(|skill| !skills.load.contains(skill))
        .collect::<Vec<_>>();
    let body_load = policy
        .load_guidance()
        .then_some(skills.load.as_slice())
        .filter(|skills| !skills.is_empty());

    let mut guidance = Vec::new();
    if let Some(load) = body_load {
        guidance.push(format!(
            "Load these skills before starting, unless they are already in your context: {}.",
            format_skill_references(load.iter().map(String::as_str))
        ));
    }
    if !available.is_empty() {
        guidance.push(format!(
            "Load these skills when the task fits: {}.",
            format_skill_references(available.iter().map(|skill| skill.as_str()))
        ));
    }

    let body = if guidance.is_empty() {
        body.to_string()
    } else {
        let mut lowered = body.to_string();
        if !lowered.ends_with("\n\n") {
            if lowered.ends_with('\n') {
                lowered.push('\n');
            } else if !lowered.is_empty() {
                lowered.push_str("\n\n");
            }
        }
        lowered.push_str(&guidance.join(" "));
        lowered.push('\n');
        lowered
    };

    LoweredAgentSkills {
        native_preload,
        body,
    }
}

fn record_agent_skill_lossiness(
    skills: &SkillsSpec,
    policy: AgentSkillsPolicy,
    target: &str,
    lossy: &mut Vec<LossyField>,
) {
    const BODY_GUIDANCE_NOTE: &str =
        "skill names are preserved as body guidance, but native loading is not guaranteed";
    const DUAL_USE_NOTE: &str = concat!(
        "native preload is exact for subagents, but body guidance used by ",
        "session agents is not guaranteed"
    );

    if policy.load_guidance() && !skills.load.is_empty() {
        lossy.push(LossyField {
            field: "skills.load".into(),
            target: target.into(),
            classification: Lossiness::Approximate {
                note: if policy.native_preload() {
                    DUAL_USE_NOTE
                } else {
                    BODY_GUIDANCE_NOTE
                },
            },
        });
    }

    // A name in both channels is loaded, matching launch-bundle semantics.
    if skills
        .available
        .iter()
        .any(|skill| !skills.load.contains(skill))
    {
        lossy.push(LossyField {
            field: "skills.available".into(),
            target: target.into(),
            classification: Lossiness::Approximate {
                note: BODY_GUIDANCE_NOTE,
            },
        });
    }
}

fn format_skill_references<'a>(skills: impl Iterator<Item = &'a str>) -> String {
    skills
        .map(|skill| format!("/{skill}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_markdown(yaml: serde_yaml::Mapping, body: &str) -> Vec<u8> {
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
    out.into_bytes()
}

fn lower_markdown_agent(
    harness: HarnessKind,
    policy: &'static AgentLoweringPolicy,
    profile: &AgentProfile,
    body: &str,
    model_field: &NativeModel,
) -> LoweredOutput {
    let mut ctx = AgentLoweringCtx::new(harness, policy, profile, body, model_field);
    record_agent_skill_lossiness(
        ctx.eff.skills(),
        agent_skills_policy(harness),
        policy.target_name,
        &mut ctx.lossy,
    );
    for &field in policy.fields {
        ctx.apply_field(field);
    }
    for &step in policy.lossiness {
        ctx.apply_lossiness(step);
    }
    ctx.finish()
}

pub fn lower_to_claude(
    profile: &AgentProfile,
    _fm: &Frontmatter,
    body: &str,
    model_field: &NativeModel,
) -> LoweredOutput {
    lower_markdown_agent(
        HarnessKind::Claude,
        &CLAUDE_AGENT_POLICY,
        profile,
        body,
        model_field,
    )
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
/// - Approximate: skills (body guidance; no native field)
/// - Dropped: tools (no allowlist), disallowed-tools,
///   mcp (approximate), network, mode, autocompact, model-policies, fanout
/// - harness-overrides.codex is launch-bundle passthrough only, not native lowering input
pub fn lower_to_codex(
    profile: &AgentProfile,
    body: &str,
    model_field: &NativeModel,
) -> LoweredOutput {
    let eff = Effective::new(profile, HarnessKind::Codex);
    let target = "Codex";
    let lowered_skills =
        lower_agent_skills(eff.skills(), agent_skills_policy(HarnessKind::Codex), body);
    let mut lossy = Vec::new();
    record_agent_skill_lossiness(
        eff.skills(),
        agent_skills_policy(HarnessKind::Codex),
        target,
        &mut lossy,
    );

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
    if eff.has_tool_allowance() {
        lossy.push(LossyField {
            field: "tools".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    if eff.has_tool_denial() {
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
            classification: Lossiness::MeridianOnly,
        });
    }
    if profile.autocompact.is_some() {
        lossy.push(LossyField {
            field: "autocompact".into(),
            target: target.into(),
            classification: Lossiness::MeridianOnly,
        });
    }
    if eff.network().is_some() {
        lossy.push(LossyField {
            field: "network".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    if eff.has_network_allow_hosts() {
        lossy.push(LossyField {
            field: "network-allow".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
        });
    }
    if eff.has_network_deny_hosts() {
        lossy.push(LossyField {
            field: "network-deny".into(),
            target: target.into(),
            classification: Lossiness::Dropped,
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
        developer_instructions: (!lowered_skills.body.trim().is_empty())
            .then_some(lowered_skills.body.trim_end()),
    };

    let out = toml::to_string_pretty(&doc).unwrap_or_default();

    LoweredOutput {
        bytes: out.into_bytes(),
        lossy_fields: lossy,
        siblings: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Probe-backed markdown native artifacts
// ---------------------------------------------------------------------------

/// Lower an agent profile to OpenCode-native markdown format.
pub fn lower_to_opencode(
    profile: &AgentProfile,
    body: &str,
    model_field: &NativeModel,
) -> LoweredOutput {
    lower_markdown_agent(
        HarnessKind::OpenCode,
        &OPENCODE_AGENT_POLICY,
        profile,
        body,
        model_field,
    )
}

fn normalize_cursor_description(description: &str) -> String {
    description.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn lower_to_cursor_with_model(
    profile: &AgentProfile,
    body: &str,
    model_field: &NativeModel,
) -> LoweredOutput {
    lower_markdown_agent(
        HarnessKind::Cursor,
        &CURSOR_AGENT_POLICY,
        profile,
        body,
        model_field,
    )
}

/// Lower an agent profile to Pi-native markdown format.
pub fn lower_to_pi(profile: &AgentProfile, body: &str, model_field: &NativeModel) -> LoweredOutput {
    lower_markdown_agent(
        HarnessKind::Pi,
        &PI_AGENT_POLICY,
        profile,
        body,
        model_field,
    )
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
    match harness_descriptor::descriptor(*harness).agent_policy {
        AgentLoweringPolicyKind::Claude => lower_to_claude(profile, fm, body, model_field),
        AgentLoweringPolicyKind::Codex => lower_to_codex(profile, body, model_field),
        AgentLoweringPolicyKind::Markdown => match harness {
            HarnessKind::OpenCode => lower_to_opencode(profile, body, model_field),
            HarnessKind::Cursor => lower_to_cursor_with_model(profile, body, model_field),
            HarnessKind::Pi => lower_to_pi(profile, body, model_field),
            HarnessKind::Claude | HarnessKind::Codex => {
                unreachable!("native policies handled above")
            }
        },
    }
}
