//! Declarative per-harness agent lowering policies and shared pipeline.
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
/// Launch-time fields (approval, sandbox, mode where not emitted, …) are
/// classified [`Lossiness::MeridianOnly`] — Meridian enforces them at spawn, so omitting
/// them from native artifacts is not a behavioral loss. Target-enforced gaps stay
/// [`Lossiness::Dropped`] or [`Lossiness::Approximate`] and warn loudly.
pub use crate::compiler::lossiness::{Lossiness, LossyField, LoweredOutput};
use crate::compiler::tool_policy::{PolicyProjection, RuleKind, ToolPolicyRuntime};
use mars_schema::HarnessId;
use mars_schema::agent::{AgentProfile, LoweredField};
use mars_schema::frontmatter::SkillsSpec;
pub use mars_schema::tool_policy::EffectiveToolPolicy;
use mars_schema::tool_policy::ToolAction;

// ---------------------------------------------------------------------------
// Effective field access for target lowering
// ---------------------------------------------------------------------------

/// Effective field values read from top-level Mars semantics.
struct Effective<'a> {
    profile: &'a AgentProfile,
    tools: &'a EffectiveToolPolicy,
}

impl<'a> Effective<'a> {
    fn new(profile: &'a AgentProfile) -> Self {
        Self {
            profile,
            tools: &profile.tools,
        }
    }

    fn effort(&self) -> Option<&mars_schema::agent::EffortLevel> {
        self.profile.effort.as_ref()
    }

    fn approval(&self) -> Option<&mars_schema::agent::ApprovalMode> {
        self.profile.approval.as_ref()
    }

    fn sandbox(&self) -> Option<&mars_schema::agent::SandboxMode> {
        self.profile.sandbox.as_ref()
    }

    fn skills(&self) -> &SkillsSpec {
        &self.profile.skills
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
/// - Skills: load is native-preloaded where supported; other channels use designed body guidance
/// - Meridian-only (launch-time): approval, sandbox, mode,, model-policies, fanout
/// - Dropped: harness, legacy-models. `harness-overrides.<harness>` keys are added unless Mars lowers that key
///
/// What model field a lowered native agent should carry. The native compiler is the
/// sole caller; this replaces a bare `Option<&str>` so "emit no model" is expressible
/// authoritatively, without mutating the profile to strip its model.
#[derive(Debug, Clone)]
pub enum NativeModel {
    /// Emit the profile's own model verbatim (unpinned alias / fanout token).
    #[cfg(test)]
    Inherit,
    /// Emit this resolved model id.
    Set(String),
    /// Emit no model field (agent emitted to a harness its model does not resolve to).
    Clear,
}

impl NativeModel {
    /// The concrete model string to render for `profile`, or `None` for "no model".
    fn resolve<'a>(&'a self, profile: &'a AgentProfile) -> Option<&'a str> {
        #[cfg(not(test))]
        let _ = profile;
        match self {
            #[cfg(test)]
            NativeModel::Inherit => profile.model.as_deref(),
            NativeModel::Set(model) => Some(model),
            NativeModel::Clear => None,
        }
    }
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
    description: DescriptionPolicy,
    mode_note: Option<&'static str>,
    skills: AgentSkillsPolicy,
    fields: &'static [LoweredField],
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

#[derive(Debug, Clone, Copy)]
enum DescriptionPolicy {
    Preserve,
    CursorOneLine,
}

const CLAUDE_AGENT_POLICY: AgentLoweringPolicy = AgentLoweringPolicy {
    description: DescriptionPolicy::Preserve,
    mode_note: None,
    // Claude ignores `skills:` when this file is selected with `claude --agent`.
    skills: AgentSkillsPolicy::NativePreloadWithReminder,
    fields: &[
        LoweredField::Identity,
        LoweredField::Model,
        LoweredField::Skills,
        LoweredField::Tools,
        LoweredField::Effort,
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
    description: DescriptionPolicy::Preserve,
    mode_note: None,
    skills: AgentSkillsPolicy::BodyGuidance,
    fields: &[
        LoweredField::Identity,
        LoweredField::Model,
        LoweredField::Mode,
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
    description: DescriptionPolicy::CursorOneLine,
    mode_note: Some("Cursor may use the same mode concept"),
    skills: AgentSkillsPolicy::NativePreload,
    fields: &[
        LoweredField::Identity,
        LoweredField::Model,
        LoweredField::Skills,
        LoweredField::Mode,
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
    description: DescriptionPolicy::Preserve,
    mode_note: Some("Pi may use the same mode concept"),
    skills: AgentSkillsPolicy::BodyGuidance,
    fields: &[
        LoweredField::Identity,
        LoweredField::Model,
        LoweredField::Mode,
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

/// Entries of the profile's `harness-overrides.<harness>` block that may be written to the
/// harness's native file: a key Mars lowers itself keeps Mars's value (`reserved`, or already
/// present), and each such override is reported as dropped.
fn native_overrides<'a>(
    profile: &'a AgentProfile,
    harness: HarnessId,
    reserved: &[&str],
    present: impl Fn(&str) -> bool,
    target: &str,
    lossy: &mut Vec<LossyField>,
) -> Vec<(&'a str, serde_json::Value)> {
    let Some(block) = profile.harness_overrides.get(&harness) else {
        return Vec::new();
    };
    let mut allowed = Vec::new();
    for (key, value) in block {
        if reserved.contains(&key.as_str()) || present(key) {
            lossy.push(LossyField {
                field: format!("harness-overrides.{harness}.{key}"),
                target: target.into(),
                classification: Lossiness::Dropped,
            });
        } else {
            allowed.push((key.as_str(), value.to_json()));
        }
    }
    allowed
}

fn yk(s: &str) -> serde_yaml::Value {
    serde_yaml::Value::String(s.to_string())
}

fn yv(s: &str) -> serde_yaml::Value {
    serde_yaml::Value::String(s.to_string())
}

struct AgentLoweringCtx<'a> {
    harness: HarnessId,
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
        harness: HarnessId,
        policy: &'static AgentLoweringPolicy,
        profile: &'a AgentProfile,
        body: &str,
        model_field: &'a NativeModel,
    ) -> Self {
        let eff = Effective::new(profile);
        let lowered_skills = lower_agent_skills(eff.skills(), policy.skills, body);
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

    fn apply_field(&mut self, field: LoweredField) {
        match field {
            LoweredField::Identity => self.insert_identity(),
            LoweredField::Model => self.insert_model(),
            LoweredField::Skills => self.insert_skills(),
            LoweredField::Tools => self.insert_claude_tools(),
            LoweredField::Effort => self.insert_claude_effort(),
            LoweredField::Mode => self.insert_mode(),
        }
    }

    fn target_name(&self) -> &'static str {
        harness_descriptor::descriptor(self.harness).display_name
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
        let target = self.target_name();
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
        match self.eff.effort() {
            // Claude Code accepts low|medium|high|xhigh|max and has no off value.
            Some(mars_schema::agent::EffortLevel::None) => self.lossy.push(LossyField {
                field: "effort".into(),
                target: self.target_name().into(),
                classification: Lossiness::Approximate {
                    note: "Claude Code has no reasoning-off effort; omitted",
                },
            }),
            Some(effort) => {
                self.yaml.insert(yk("effort"), yv(effort.claude_str()));
            }
            None => {}
        }
    }

    fn insert_mode(&mut self) {
        if let Some(mode) = &self.profile.mode {
            self.yaml.insert(yk("mode"), yv(mode.as_str()));
            if let Some(note) = self.policy.mode_note {
                self.lossy.push(LossyField {
                    field: "mode".into(),
                    target: self.target_name().into(),
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
                        target: self.target_name().into(),
                        classification: Lossiness::Approximate { note },
                    });
                }
            }
            AgentLossinessStep::McpApproximate(note) => {
                record_native_mcp_lossiness(&self.eff, self.target_name(), note, &mut self.lossy)
            }
            AgentLossinessStep::McpDropped => {
                if self.eff.has_mcp_policy() {
                    self.lossy.push(LossyField {
                        field: "mcp".into(),
                        target: self.target_name().into(),
                        classification: Lossiness::Dropped,
                    });
                }
            }
            AgentLossinessStep::MeridianOnlyFields => self.record_meridian_only_fields(),
            AgentLossinessStep::InvocationAxes => {
                record_agent_invocation_lossiness(self.profile, self.target_name(), &mut self.lossy)
            }
        }
    }

    fn record_approval(&mut self) {
        let Some(_) = self.eff.approval() else {
            return;
        };
        let classification = if self.harness == HarnessId::Cursor {
            // Cursor maps approval to CLI flags — approximate, target-enforced gap.
            Lossiness::Approximate {
                note: "auto maps to --force, never to --yolo; confirm has no Cursor equivalent and falls back to default",
            }
        } else {
            // Launch-time: Meridian applies approval at spawn via harness projection.
            Lossiness::MeridianOnly
        };
        self.lossy.push(LossyField {
            field: "approval".into(),
            target: self.target_name().into(),
            classification,
        });
    }

    fn record_sandbox(&mut self) {
        let Some(_) = self.eff.sandbox() else {
            return;
        };
        let classification = if self.harness == HarnessId::Cursor {
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
            target: self.target_name().into(),
            classification,
        });
    }

    fn record_if_present(&mut self, field: &str, present: bool, classification: Lossiness) {
        if present {
            self.lossy.push(LossyField {
                field: field.into(),
                target: self.target_name().into(),
                classification,
            });
        }
    }

    fn record_meridian_only_fields(&mut self) {
        self.record_if_present(
            "model-policies",
            !self.profile.model_policies.is_empty(),
            Lossiness::MeridianOnly,
        );
    }

    fn finish(mut self) -> LoweredOutput {
        let reserved = self
            .policy
            .fields
            .iter()
            .flat_map(|field| field.keys().iter().copied())
            .collect::<Vec<_>>();
        let overrides = native_overrides(
            self.profile,
            self.harness,
            &reserved,
            |key| self.yaml.contains_key(yk(key)),
            self.target_name(),
            &mut self.lossy,
        );
        for (key, value) in overrides {
            self.yaml.insert(
                yk(key),
                serde_yaml::to_value(value).expect("override value should serialize"),
            );
        }
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
    let load = dedupe_skill_names(&skills.load);
    let native_preload = if policy.native_preload() {
        load.clone()
    } else {
        Vec::new()
    };

    // A skill named in both channels is loaded, matching launch-bundle semantics.
    let available = dedupe_skill_names(&skills.available)
        .into_iter()
        .filter(|skill| !load.contains(skill))
        .collect::<Vec<_>>();
    let body_load = policy
        .load_guidance()
        .then_some(load.as_slice())
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
            format_skill_references(available.iter().map(String::as_str))
        ));
    }

    let body = if guidance.is_empty() {
        body.to_string()
    } else {
        append_guidance(body, &guidance.join(" "))
    };

    LoweredAgentSkills {
        native_preload,
        body,
    }
}

fn dedupe_skill_names(skills: &[String]) -> Vec<String> {
    let mut deduped = Vec::with_capacity(skills.len());
    for skill in skills {
        if !deduped.contains(skill) {
            deduped.push(skill.clone());
        }
    }
    deduped
}

fn append_guidance(body: &str, guidance: &str) -> String {
    let line_ending = body_line_ending(body);
    let mut lowered = body.to_string();

    let mut fences = mars_schema::markdown::FenceTracker::default();
    for line in body.split_inclusive('\n') {
        fences.visit(line);
    }

    // An unclosed authored fence would otherwise turn the generated instruction into code.
    if let Some(fence) = fences.open() {
        if !lowered.ends_with(line_ending) {
            lowered.push_str(line_ending);
        }
        lowered.extend(std::iter::repeat_n(' ', fence.indentation));
        lowered.extend(std::iter::repeat_n(fence.marker, fence.length));
        lowered.push_str(line_ending);
    }

    let blank_line = format!("{line_ending}{line_ending}");
    if !lowered.ends_with(&blank_line) {
        if lowered.ends_with(line_ending) {
            lowered.push_str(line_ending);
        } else if !lowered.is_empty() {
            lowered.push_str(&blank_line);
        }
    }
    lowered.push_str(guidance);
    lowered.push_str(line_ending);
    lowered
}

fn body_line_ending(body: &str) -> &'static str {
    body.find('\n')
        .map(|newline| {
            if body[..newline].ends_with('\r') {
                "\r\n"
            } else {
                "\n"
            }
        })
        .unwrap_or("\n")
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
    harness: HarnessId,
    policy: &'static AgentLoweringPolicy,
    profile: &AgentProfile,
    body: &str,
    model_field: &NativeModel,
) -> LoweredOutput {
    let mut ctx = AgentLoweringCtx::new(harness, policy, profile, body, model_field);
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
    body: &str,
    model_field: &NativeModel,
) -> LoweredOutput {
    lower_markdown_agent(
        HarnessId::Claude,
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
/// - Skills: designed body guidance (Codex has no native field)
/// - Dropped: tools (no allowlist), disallowed-tools,
///   mcp (approximate), mode, model-policies, fanout
/// - harness-overrides.codex entries are added as TOML keys unless Mars lowers that key itself
pub fn lower_to_codex(
    profile: &AgentProfile,
    body: &str,
    model_field: &NativeModel,
) -> LoweredOutput {
    let eff = Effective::new(profile);
    let target = "Codex";
    let lowered_skills = lower_agent_skills(eff.skills(), AgentSkillsPolicy::BodyGuidance, body);
    let mut lossy = Vec::new();

    // Effort — exact (lowered to model_reasoning_effort)
    let effort_str = eff.effort().map(|e| e.as_str());

    // Sandbox — exact
    let sandbox_str = eff.sandbox().map(|s| s.as_str());

    // Approval — exact (lowered to approval_policy)
    let approval_policy = eff.approval().and_then(|a| {
        use mars_schema::agent::ApprovalMode;
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
    if !profile.model_policies.is_empty() {
        lossy.push(LossyField {
            field: "model-policies".into(),
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

    let mut out = toml::to_string_pretty(&doc).unwrap_or_default();
    // The fields above are all top-level scalars, so the override tables can follow them.
    let overrides = native_overrides(
        profile,
        HarnessId::Codex,
        &[
            "name",
            "description",
            "model",
            "model_reasoning_effort",
            "sandbox_mode",
            "approval_policy",
            "developer_instructions",
        ],
        |_| false,
        target,
        &mut lossy,
    );
    if !overrides.is_empty() {
        let table = overrides
            .into_iter()
            .map(|(key, value)| {
                let value = toml::Value::try_from(value).expect("override value should be TOML");
                (key.to_owned(), value)
            })
            .collect::<toml::Table>();
        out.push_str(&toml::to_string_pretty(&table).unwrap_or_default());
    }

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
        HarnessId::OpenCode,
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
        HarnessId::Cursor,
        &CURSOR_AGENT_POLICY,
        profile,
        body,
        model_field,
    )
}

/// Lower an agent profile to Pi-native markdown format.
pub fn lower_to_pi(profile: &AgentProfile, body: &str, model_field: &NativeModel) -> LoweredOutput {
    lower_markdown_agent(HarnessId::Pi, &PI_AGENT_POLICY, profile, body, model_field)
}

// ---------------------------------------------------------------------------
// Dispatch: lower for a given harness
// ---------------------------------------------------------------------------

/// Lower an agent to the native format for the given harness.
///
/// Returns `None` for unknown harnesses (should not happen if the profile was
/// validated, but guards against future harness additions).
pub fn lower_for_harness_with_model(
    harness: &HarnessId,
    profile: &AgentProfile,
    body: &str,
    model_field: &NativeModel,
) -> LoweredOutput {
    match harness_descriptor::descriptor(*harness).agent_policy {
        AgentLoweringPolicyKind::Claude => lower_to_claude(profile, body, model_field),
        AgentLoweringPolicyKind::Codex => lower_to_codex(profile, body, model_field),
        AgentLoweringPolicyKind::Markdown => match harness {
            HarnessId::OpenCode => lower_to_opencode(profile, body, model_field),
            HarnessId::Cursor => lower_to_cursor_with_model(profile, body, model_field),
            HarnessId::Pi => lower_to_pi(profile, body, model_field),
            HarnessId::Claude | HarnessId::Codex => {
                unreachable!("native policies handled above")
            }
        },
    }
}
