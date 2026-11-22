//! Per-harness agent lowering policies and the shared pipeline.
//!
//! Translates a parsed [`AgentProfile`] into the harness's native
//! file, under the policy on its row in `compiler::lowering`.
//!
//! # Lossiness classification (per agent-compilation-mapping.md §6)
//!
//! Every field lowering is classified as:
//! - **exact** — field maps 1:1 to a native equivalent with identical semantics
//! - **approximate** — semantic equivalent exists but gap is noted
//! - **dropped** — no native equivalent; value is discarded in native artifact
//! - **meridian-only** — consumed exclusively by Meridian; never lowered
//!
//! Dropped fields with non-default values emit [`LossyField`] diagnostics.
//!
//! Launch-time fields (approval, sandbox, mode where not emitted, model-policies, …) are
//! classified [`Lossiness::MeridianOnly`]: Meridian applies them at spawn. A restriction the
//! native file cannot carry (a tool deny, `ask`, an allow list, an
//! MCP deny, a network limit, an overridden policy key) is [`Lossiness::Dropped`] with the kind it
//! costs, and prints by default: the target runs the agent with what the author withheld. Quiet
//! needs a reason ([`Lossiness::Quiet`]): a `deny` of file writes on Codex when the agent's
//! `sandbox: read-only` is written to the same file, a plain grant, a network grant. An
//! unclassified site is loud. Codex lowers `network` as dropped, because its native artifact
//! carries `sandbox_mode`, so a natively run Codex agent keeps Codex's own network default.

use crate::compiler::agents::opencode_permission;
use crate::compiler::agents::tool_loss::{self, Allowlist, Carries, DenyCarry};
pub use crate::compiler::lossiness::{Lossiness, LossyField, Lost, LoweredOutput, ProjectLoss};
use crate::compiler::lowering::{AgentFormat, lowering};
use mars_schema::HarnessId;
use mars_schema::agent::{AgentProfile, LoweredField};
use mars_schema::frontmatter::SkillsSpec;
use mars_schema::ordered::OrderedValue;
pub use mars_schema::tool_policy::EffectiveToolPolicy;
use mars_schema::tool_policy::ToolAction;
use mars_schema::tool_project::{RuleKind, project};

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

    /// The level to write. `auto` and no effort both write nothing.
    fn effort(&self) -> Option<mars_schema::agent::EffortLevel> {
        match self.profile.effort {
            Some(mars_schema::agent::EffortRequest::Level(level)) => Some(level),
            Some(mars_schema::agent::EffortRequest::Default) | None => None,
        }
    }

    fn approval(&self) -> Option<&mars_schema::agent::ApprovalMode> {
        self.profile.approval.as_ref()
    }

    fn sandbox(&self) -> Option<&mars_schema::agent::SandboxMode> {
        self.profile.sandbox.as_ref()
    }

    fn network(&self) -> Option<&mars_schema::network::NetworkPolicy> {
        self.profile.network.as_ref()
    }

    fn has_network_allow_hosts(&self) -> bool {
        !self.profile.network_allow.entries().is_empty()
    }

    fn has_network_deny_hosts(&self) -> bool {
        !self.profile.network_deny.entries().is_empty()
    }

    fn skills(&self) -> &SkillsSpec {
        &self.profile.skills
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
            classification: Lossiness::Quiet {
                why: "no target file carries it, so another model can still pick the agent",
            },
        });
    }
    if profile.had_user_invocable_field && !profile.user_invocable {
        lossy.push(LossyField {
            field: "user-invocable".into(),
            target: target.into(),
            classification: Lossiness::Quiet {
                why: "the user can still invoke it here; no tool or network access changes",
            },
        });
    }
}

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
    /// Emit the author's token as written: Mars could not check it, so the harness judges it.
    Unchecked(String),
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
            NativeModel::Set(model) | NativeModel::Unchecked(model) => Some(model),
            NativeModel::Clear => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum AgentLossinessStep {
    ModeDropped,
    /// The tool rules the target's file cannot carry, per [`tool_loss::losses`].
    ToolRules(Carries),
    /// OpenCode: the tool rules become the `permission` map, the one tool control its agent
    /// file has (`tools` booleans are deprecated).
    PermissionMap,
    EffortApproximate(&'static str),
    /// Meridian sets the effort for each session; the native file has no effort.
    EffortMeridianOnly,
    MeridianOnlyFields,
    InvocationAxes,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AgentLoweringPolicy {
    mode_note: Option<&'static str>,
    skills: AgentSkillsPolicy,
    fields: &'static [LoweredField],
    lossiness: &'static [AgentLossinessStep],
}

#[derive(Debug, Clone, Copy)]
enum AgentSkillsPolicy {
    NativePreloadWithReminder,
    BodyGuidance,
}

impl AgentSkillsPolicy {
    fn native_preload(self) -> bool {
        !matches!(self, Self::BodyGuidance)
    }
}

impl AgentLoweringPolicy {
    pub(crate) fn writes_model(&self) -> bool {
        self.fields.iter().any(|field| matches!(field, LoweredField::Model))
    }
}

pub(crate) const CLAUDE_AGENT_POLICY: AgentLoweringPolicy = AgentLoweringPolicy {
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
        AgentLossinessStep::ModeDropped,
        AgentLossinessStep::ToolRules(Carries {
            deny: DenyCarry::Unscoped,
            ask: false,
            allowlist: Allowlist::NonEmpty,
            writes_covered: false,
            mcp_remainder: None,
        }),
        AgentLossinessStep::MeridianOnlyFields,
        AgentLossinessStep::InvocationAxes,
    ],
};

pub(crate) const OPENCODE_AGENT_POLICY: AgentLoweringPolicy = AgentLoweringPolicy {
    mode_note: None,
    skills: AgentSkillsPolicy::BodyGuidance,
    fields: &[
        LoweredField::Identity,
        LoweredField::Model,
        LoweredField::Mode,
    ],
    lossiness: &[
        AgentLossinessStep::ToolRules(Carries {
            deny: DenyCarry::All,
            ask: true,
            allowlist: Allowlist::Any,
            writes_covered: false,
            mcp_remainder: Some(Lossiness::Approximate {
                note: "MCP grants on subprocess errors; streaming uses session payload",
            }),
        }),
        AgentLossinessStep::PermissionMap,
        AgentLossinessStep::EffortApproximate("effort maps to --variant on subprocess only"),
        AgentLossinessStep::MeridianOnlyFields,
        AgentLossinessStep::InvocationAxes,
    ],
};

pub(crate) const PI_AGENT_POLICY: AgentLoweringPolicy = AgentLoweringPolicy {
    mode_note: Some("Pi may use the same mode concept"),
    skills: AgentSkillsPolicy::BodyGuidance,
    fields: &[
        LoweredField::Identity,
        LoweredField::Model,
        LoweredField::Mode,
    ],
    lossiness: &[
        AgentLossinessStep::ToolRules(Carries {
            deny: DenyCarry::Nothing,
            ask: false,
            allowlist: Allowlist::Nothing,
            writes_covered: false,
            mcp_remainder: Some(Lossiness::Quiet {
                why: "an MCP grant restricts nothing",
            }),
        }),
        AgentLossinessStep::EffortApproximate("Pi effort semantics unverified"),
        AgentLossinessStep::MeridianOnlyFields,
        AgentLossinessStep::InvocationAxes,
    ],
};

pub(crate) const AGY_AGENT_POLICY: AgentLoweringPolicy = AgentLoweringPolicy {
    mode_note: None,
    skills: AgentSkillsPolicy::BodyGuidance,
    fields: &[LoweredField::Identity, LoweredField::AllowlistOrAll],
    lossiness: &[
        AgentLossinessStep::ModeDropped,
        AgentLossinessStep::ToolRules(Carries {
            deny: DenyCarry::Omission,
            ask: false,
            allowlist: Allowlist::NonEmpty,
            writes_covered: false,
            mcp_remainder: Some(Lossiness::Quiet {
                why: "agy gives every agent every MCP server, so an MCP grant restricts nothing",
            }),
        }),
        AgentLossinessStep::EffortMeridianOnly,
        AgentLossinessStep::MeridianOnlyFields,
        AgentLossinessStep::InvocationAxes,
    ],
};

/// Override keys that carry policy: dropping one leaves the target running the agent with
/// permissions the author's override would have limited. Any other key is not known to restrict.
const POLICY_OVERRIDE_KEYS: &[&str] = &[
    "tools",
    "disallowed-tools",
    "disallowedTools",
    "permission",
    "permissions",
    "permission-mode",
    "sandbox_mode",
    "approval_policy",
];

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
) -> Vec<(&'a str, &'a OrderedValue)> {
    let Some(block) = profile.harness_overrides.get(&harness) else {
        return Vec::new();
    };
    let mut allowed = Vec::new();
    for (key, value) in block {
        if reserved.contains(&key.as_str()) || present(key) {
            lossy.push(LossyField {
                field: format!("harness-overrides.{harness}.{key}"),
                target: target.into(),
                classification: if POLICY_OVERRIDE_KEYS.contains(&key.as_str()) {
                    Lossiness::Dropped(Lost::Policy)
                } else {
                    Lossiness::Quiet {
                        why: "not a known restriction; Mars keeps its own value for this key",
                    }
                },
            });
        } else {
            allowed.push((key.as_str(), value));
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
    project_losses: Vec<ProjectLoss>,
    /// Whether the target writes a `permission` map built from the tool rules and the override.
    writes_permission_map: bool,
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
            project_losses: Vec::new(),
            writes_permission_map: false,
        }
    }

    fn apply_field(&mut self, field: LoweredField) {
        match field {
            LoweredField::Identity => self.insert_identity(),
            LoweredField::Model => self.insert_model(),
            LoweredField::Skills => self.insert_skills(),
            LoweredField::Tools => self.insert_tools(),
            LoweredField::Effort => self.insert_claude_effort(),
            LoweredField::Mode => self.insert_mode(),
            LoweredField::AllowlistOrAll => self.insert_allowlist_or_all(),
        }
    }

    fn target_name(&self) -> &'static str {
        self.harness.display_name()
    }

    fn insert_identity(&mut self) {
        if let Some(name) = &self.profile.name {
            self.yaml.insert(yk("name"), yv(name));
        }
        if let Some(desc) = &self.profile.description {
            self.yaml.insert(yk("description"), yv(desc));
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

    fn insert_tools(&mut self) {
        let projection = project(self.eff.tools, self.harness);
        let allowed = projection.available();
        let disallowed: Vec<_> = projection
            .natives(ToolAction::Deny, RuleKind::All)
            .into_iter()
            .filter(|tool| !tool.contains('('))
            .collect();

        // An agent file's `tools:` restricts the agent to the tools it lists, and leaving it out
        // inherits every tool. So only an allowlist can lower to it: a map-form grant
        // (`tools: {bash: allow}`) says "allow this", not "only this", and emitting it would
        // strip the agent of every tool the map did not name.
        if self.eff.tools.is_allowlist() && !allowed.is_empty() {
            self.yaml.insert(
                yk("tools"),
                serde_yaml::Value::Sequence(allowed.iter().map(|tool| yv(tool)).collect()),
            );
        }
        // A scoped entry in `disallowedTools` removes the whole tool in Claude, so only whole-tool
        // denies are written; `tool_loss` reports the scoped ones as lost.
        // The key Claude reads in an agent file is camelCase; `disallowed-tools` is a skill's.
        if !disallowed.is_empty() {
            self.yaml.insert(
                yk("disallowedTools"),
                serde_yaml::Value::Sequence(disallowed.iter().map(|tool| yv(tool)).collect()),
            );
        }
    }

    /// `agy` gives an agent with no `tools` key no write or shell, has no deny list and no
    /// wildcard: the key is the allowlist, or every tool name Mars knows when the policy does
    /// not restrict, less the tools the policy denies whole (a scoped deny has no native name).
    /// An empty list writes no key.
    fn insert_allowlist_or_all(&mut self) {
        let projection = project(self.eff.tools, self.harness);
        let mut tools: Vec<String> = if self.eff.tools.is_allowlist() {
            projection.available()
        } else {
            // `agy` has no wildcard (`*` and `all` fail the agent at its first prompt) and gives
            // an agent with no `tools` key no write or shell, so an agent that restricts
            // nothing lists every tool.
            mars_schema::tool_natives::every_tool(self.harness)
                .into_iter()
                .map(str::to_owned)
                .collect()
        };
        let denied = projection.natives(ToolAction::Deny, RuleKind::All);
        tools.retain(|tool| !denied.contains(tool));
        if !tools.is_empty() {
            self.yaml.insert(
                yk("tools"),
                serde_yaml::Value::Sequence(tools.iter().map(|tool| yv(tool)).collect()),
            );
        }
    }

    fn insert_claude_effort(&mut self) {
        if let Some(effort) = self.eff.effort() {
            self.yaml.insert(yk("effort"), yv(effort.as_str()));
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
            AgentLossinessStep::ModeDropped => {
                self.record_if_present("mode", self.profile.mode.is_some(), Lossiness::MeridianOnly)
            }
            AgentLossinessStep::ToolRules(carries) => {
                let target = self.target_name();
                let losses = tool_loss::losses(self.eff.tools, self.harness, target, carries);
                self.lossy.extend(losses.agent);
                self.project_losses.extend(losses.project);
            }
            AgentLossinessStep::PermissionMap => self.writes_permission_map = true,
            AgentLossinessStep::EffortApproximate(note) => {
                if self.eff.effort().is_some() {
                    self.lossy.push(LossyField {
                        field: "effort".into(),
                        target: self.target_name().into(),
                        classification: Lossiness::Approximate { note },
                    });
                }
            }
            AgentLossinessStep::EffortMeridianOnly => self.record_if_present(
                "effort",
                self.eff.effort().is_some(),
                Lossiness::MeridianOnly,
            ),
            AgentLossinessStep::MeridianOnlyFields => self.record_meridian_only_fields(),
            AgentLossinessStep::InvocationAxes => {
                record_agent_invocation_lossiness(self.profile, self.target_name(), &mut self.lossy)
            }
        }
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

    /// Launch-time fields no native file carries; Meridian applies them at spawn.
    fn record_meridian_only_fields(&mut self) {
        self.record_if_present(
            "approval",
            self.eff.approval().is_some(),
            Lossiness::MeridianOnly,
        );
        self.record_if_present(
            "sandbox",
            self.eff.sandbox().is_some(),
            Lossiness::MeridianOnly,
        );
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
        let mut overrides = native_overrides(
            self.profile,
            self.harness,
            &reserved,
            |key| self.yaml.contains_key(yk(key)),
            self.target_name(),
            &mut self.lossy,
        );
        if self.writes_permission_map {
            overrides.retain(|(key, _)| *key != "permission");
            let built = opencode_permission::build(
                self.eff.tools,
                opencode_permission::author_override(self.profile),
                self.target_name(),
                &mut self.lossy,
            );
            if !built.map.is_empty() {
                self.yaml
                    .insert(yk("permission"), serde_yaml::Value::Mapping(built.map));
            }
            if built.non_deny {
                self.project_losses.push(ProjectLoss {
                    key: opencode_permission::OUTSIDE_RULES_KEY.into(),
                    field: LossyField {
                        field: opencode_permission::OUTSIDE_RULES_NOTE.into(),
                        target: self.target_name().into(),
                        classification: Lossiness::Dropped(Lost::Policy),
                    },
                });
            }
        }
        for (key, value) in overrides {
            self.yaml.insert(
                yk(key),
                serde_yaml::to_value(value.to_json()).expect("override value should serialize"),
            );
        }
        LoweredOutput {
            bytes: render_markdown(self.yaml, &self.body),
            lossy_fields: self.lossy,
            project_losses: self.project_losses,
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
    let body_load = Some(load.as_slice()).filter(|skills| !skills.is_empty());

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

/// Lower an agent profile to Codex-native TOML format.
///
/// Per agent-compilation-mapping.md V0 §5.4 and §10:
/// - Preserved: name, description, model, effort (as model_reasoning_effort),
///   sandbox (as sandbox_mode), approval (as approval_policy), body
///   (as developer_instructions)
/// - Skills: designed body guidance (Codex has no native field)
/// - Dropped: tools (no allowlist), disallowed-tools,
///   mcp (approximate), network, mode, model-policies, fanout
/// - harness-overrides.codex entries are added as TOML keys unless Mars lowers that key itself
fn lower_to_codex(profile: &AgentProfile, body: &str, model_field: &NativeModel) -> LoweredOutput {
    let eff = Effective::new(profile);
    let target = "Codex";
    let lowered_skills = lower_agent_skills(eff.skills(), AgentSkillsPolicy::BodyGuidance, body);
    let mut lossy = Vec::new();

    // Effort — exact (lowered to model_reasoning_effort); Codex takes every level.
    let effort_str = eff.effort().map(|effort| effort.as_str());

    // Sandbox — exact
    let sandbox_str = eff.sandbox().map(|s| s.as_str());

    // Approval — exact (lowered to approval_policy)
    let approval_policy = eff.approval().and_then(|a| {
        use mars_schema::agent::ApprovalMode;
        match a {
            ApprovalMode::Default => None,
            ApprovalMode::Auto => Some("on-request"),
            ApprovalMode::Confirm => Some("on-request"),
            ApprovalMode::Never => Some("never"),
        }
    });

    // The Codex file writes `sandbox_mode`, so a read-only sandbox there covers write denies.
    let tool_losses = tool_loss::losses(
        eff.tools,
        HarnessId::Codex,
        target,
        Carries {
            deny: DenyCarry::Nothing,
            ask: false,
            allowlist: Allowlist::Nothing,
            writes_covered: eff.sandbox() == Some(&mars_schema::agent::SandboxMode::ReadOnly),
            mcp_remainder: Some(Lossiness::Approximate {
                note: "Codex per-tool MCP gating lives in server config, not the tool list",
            }),
        },
    );
    lossy.extend(tool_losses.agent);
    // The native file carries no network setting, so Codex keeps its own default: anything but
    // `network: allow` is a restriction that does not apply.
    use mars_schema::network::NetworkPolicy;
    for (field, present, restriction) in [
        (
            "network",
            eff.network().is_some(),
            eff.network() != Some(&NetworkPolicy::Allow),
        ),
        ("network-allow", eff.has_network_allow_hosts(), true),
        ("network-deny", eff.has_network_deny_hosts(), true),
    ] {
        if present {
            lossy.push(LossyField {
                field: field.into(),
                target: target.into(),
                classification: if restriction {
                    Lossiness::Dropped(Lost::Network)
                } else {
                    Lossiness::Quiet {
                        why: "a grant, not a restriction; Codex keeps its own network setting",
                    }
                },
            });
        }
    }
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
                let value =
                    toml::Value::try_from(value.to_json()).expect("override value should be TOML");
                (key.to_owned(), value)
            })
            .collect::<toml::Table>();
        out.push_str(&toml::to_string_pretty(&table).unwrap_or_default());
    }

    LoweredOutput {
        bytes: out.into_bytes(),
        lossy_fields: lossy,
        project_losses: tool_losses.project,
        siblings: Vec::new(),
    }
}

/// Lower an agent to the native format of `harness`.
pub fn lower_for_harness_with_model(
    harness: HarnessId,
    profile: &AgentProfile,
    body: &str,
    model_field: &NativeModel,
) -> LoweredOutput {
    match lowering(harness).agent {
        AgentFormat::Markdown(policy) => {
            lower_markdown_agent(harness, policy, profile, body, model_field)
        }
        AgentFormat::CodexToml => lower_to_codex(profile, body, model_field),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mars_schema::frontmatter::Frontmatter;
    use mars_schema::{agent::AgentProfile, diagnostic::SchemaDiagnostic};

    fn profile_from(content: &str) -> (AgentProfile, Frontmatter, Vec<SchemaDiagnostic>) {
        let reading = mars_schema::agent::read(content);
        let fm = Frontmatter::parse(content).unwrap();
        (reading.fields.unwrap(), fm, reading.diagnostics)
    }

    // --- 3.3: Claude lowering ---

    #[test]
    fn claude_lowering_omits_unrecognized_tools_and_preserves_mcp_wire_identifiers() {
        let content = "---\nname: coder\nharness: claude\ntools:\n  '*': deny\n  my_custom_tool: allow\n  mcp__server__Tool: allow\n---\n# Body";
        let (profile, fm, diags) = profile_from(content);
        assert!(diags.is_empty());
        let out = lower_for_harness_with_model(
            HarnessId::Claude,
            &profile,
            fm.body(),
            &NativeModel::Inherit,
        );
        let text = String::from_utf8(out.bytes).unwrap();

        assert!(
            !text.to_lowercase().contains("custom"),
            "an unrecognized tool must not be guessed into a native name: {text}"
        );
        assert!(
            text.contains("- mcp__server__Tool"),
            "mcp__ wire identifier should be preserved verbatim: {text}"
        );
        assert!(out.lossy_fields.iter().any(|field| {
            field.field.starts_with("tools (allow my_custom_tool")
                && field.target == "Claude"
                && matches!(field.classification, Lossiness::Dropped(_))
        }));
    }

    #[test]
    fn claude_lowering_drops_approval_sandbox_network_mode() {
        let content = "---\nname: coder\nharness: claude\napproval: auto\nsandbox: read-only\nnetwork: allowlist\nnetwork-allow: [example.com]\nnetwork-deny: [blocked.example]\nmode: subagent\n---\n# Body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_for_harness_with_model(
            HarnessId::Claude,
            &profile,
            fm.body(),
            &NativeModel::Inherit,
        );
        let text = String::from_utf8(out.bytes).unwrap();
        for leaked in [
            "approval:",
            "sandbox:",
            "network:",
            "network-allow:",
            "network-deny:",
        ] {
            assert!(!text.contains(leaked), "{leaked} leaked: {text}");
        }
        // Launch-time fields are meridian-only, not target-enforced drops.
        let meridian_only: Vec<_> = out
            .lossy_fields
            .iter()
            .filter(|f| matches!(f.classification, Lossiness::MeridianOnly))
            .map(|f| f.field.as_str())
            .collect();
        for field in [
            "approval",
            "sandbox",
            "network",
            "network-allow",
            "network-deny",
            "mode",
        ] {
            assert!(
                meridian_only.contains(&field),
                "{field} not in lossy: {meridian_only:?}"
            );
        }
    }

    #[test]
    fn claude_harness_override_does_not_replace_mcp() {
        let content = "---\nname: r\nharness: claude\ntools:\n  '*': deny\n  mcp(plugin:base): allow\nharness-overrides:\n  claude:\n    tools: [plugin:claude]\n    color: red\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_for_harness_with_model(
            HarnessId::Claude,
            &profile,
            fm.body(),
            &NativeModel::Inherit,
        );
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            text.contains("color: red"),
            "non-conflicting override should be written: {text}"
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
    fn pi_agent_records_mcp_lossiness_when_mcp_refs_present() {
        let content = "---\nname: r\nharness: pi\ntools:\n  mcp(plugin:demo): allow\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out =
            lower_for_harness_with_model(HarnessId::Pi, &profile, fm.body(), &NativeModel::Inherit);
        assert!(out.lossy_fields.iter().any(|field| {
            field.field.starts_with("mcp")
                && field.target == "Pi"
                && matches!(field.classification, Lossiness::Quiet { .. })
        }));
    }
    #[test]
    fn claude_agent_model_invocable_false_warn_drops_without_skill_key() {
        let content = "---\nname: coder\nharness: claude\nmodel-invocable: false\n---\n# Body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_for_harness_with_model(
            HarnessId::Claude,
            &profile,
            fm.body(),
            &NativeModel::Inherit,
        );
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("disable-model-invocation"),
            "subagents have no invocation frontmatter; must not emit skill-only key: {text}"
        );
        assert!(out.lossy_fields.iter().any(|f| {
            f.field == "model-invocable"
                && f.target == "Claude"
                && matches!(f.classification, Lossiness::Quiet { .. })
        }));
    }

    #[test]
    fn claude_agent_user_invocable_false_warn_drops() {
        let content = "---\nname: coder\nharness: claude\nuser-invocable: false\n---\n# Body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_for_harness_with_model(
            HarnessId::Claude,
            &profile,
            fm.body(),
            &NativeModel::Inherit,
        );
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("user-invocable"),
            "no native agent key: {text}"
        );
        assert!(out.lossy_fields.iter().any(|f| {
            f.field == "user-invocable"
                && f.target == "Claude"
                && matches!(f.classification, Lossiness::Quiet { .. })
        }));
    }

    // --- 3.3: Codex lowering ---

    #[test]
    fn codex_lowering_drops_network_and_keeps_sandbox() {
        let content = "---\nname: coder\nharness: codex\nsandbox: workspace-write\nnetwork: allowlist\nnetwork-allow: [example.com]\nnetwork-deny: [blocked.example]\n---\n# Coder";
        let (profile, fm, _) = profile_from(content);
        let out = lower_for_harness_with_model(
            HarnessId::Codex,
            &profile,
            fm.body(),
            &NativeModel::Inherit,
        );
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            text.contains("sandbox_mode = \"workspace-write\""),
            "sandbox missing: {text}"
        );
        assert!(!text.contains("network"), "network leaked: {text}");
        for field in ["network", "network-allow", "network-deny"] {
            assert!(
                out.lossy_fields.iter().any(|lossy| lossy.field == field
                    && matches!(lossy.classification, Lossiness::Dropped(_))),
                "{field} not reported as a lost restriction: {:?}",
                out.lossy_fields
            );
        }
    }

    #[test]
    fn codex_mcp_lossiness_uses_top_level_policy() {
        let content = "---\nname: r\nharness: codex\ntools:\n  mcp(plugin:base): allow\nharness-overrides:\n  codex:\n    mcp-tools: []\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_for_harness_with_model(
            HarnessId::Codex,
            &profile,
            fm.body(),
            &NativeModel::Inherit,
        );
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
    fn codex_override_adds_keys_and_never_replaces_lowered_ones() {
        let content = "---\nname: r\neffort: low\nharness-overrides:\n  codex:\n    custom_key: 1\n    model_reasoning_effort: high\n  claude:\n    color: red\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_for_harness_with_model(
            HarnessId::Codex,
            &profile,
            fm.body(),
            &NativeModel::Inherit,
        );
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("custom_key = 1"), "{text}");
        assert!(text.contains("model_reasoning_effort = \"low\""), "{text}");
        assert!(
            !text.contains("color"),
            "claude overrides must not reach Codex: {text}"
        );
        assert!(out.lossy_fields.iter().any(|field| {
            field.field == "harness-overrides.codex.model_reasoning_effort"
                && matches!(field.classification, Lossiness::Quiet { .. })
        }));
    }

    // --- 3.3: OpenCode lowering ---

    #[test]
    fn override_passthrough_keeps_mcp_lossiness_and_stays_with_its_harness() {
        let content = "---\nname: r\nharness: opencode\ntools:\n  \"*\": deny\n  Read: allow\n  mcp(plugin:base): allow\nharness-overrides:\n  opencode:\n    tools: []\n    mcp-tools: []\n    native-config:\n      opencode.only: true\n  cursor:\n    tools: [Bash]\n    mcp-tools: [plugin:cursor]\n    native-config:\n      cursor.only: true\n---\n# body";
        let (profile, fm, _) = profile_from(content);

        let opencode = lower_for_harness_with_model(
            HarnessId::OpenCode,
            &profile,
            fm.body(),
            &NativeModel::Inherit,
        );
        let text = String::from_utf8(opencode.bytes.clone()).unwrap();
        assert!(
            text.contains("permission:\n  '*': deny\n  read: allow\n")
                && !opencode
                    .lossy_fields
                    .iter()
                    .any(|field| field.field.starts_with("tools")),
            "OpenCode's file carries the allowlist, and the override cannot replace it: {text}",
        );
        assert!(
            opencode
                .lossy_fields
                .iter()
                .any(|field| field.field == "mcp"),
            "harness-overrides passthrough should not clear mcp lossiness",
        );
    }

    // --- Antigravity lowering ---

    fn agy(content: &str) -> (String, Vec<crate::compiler::lossiness::LossyField>) {
        let (profile, fm, _) = profile_from(content);
        let out = lower_for_harness_with_model(
            HarnessId::Agy,
            &profile,
            fm.body(),
            &NativeModel::Inherit,
        );
        (String::from_utf8(out.bytes).unwrap(), out.lossy_fields)
    }

    #[test]
    fn agy_agent_without_rules_gets_every_tool_and_no_role_keys() {
        let (text, lossy) =
            agy("---\nname: r\ndescription: d\nmodel: opus\neffort: high\n---\n# body");
        assert!(
            !text.contains("'*'") && !text.contains("mainAgent"),
            "{text}"
        );
        assert!(
            text.contains("- run_command\n") && !text.contains("call_mcp_tool"),
            "{text}"
        );
        assert!(
            !text.contains("model:") && !text.contains("effort:"),
            "{text}"
        );
        assert!(
            lossy
                .iter()
                .any(|f| f.field == "effort" && f.classification == Lossiness::MeridianOnly)
        );
    }

    #[test]
    fn agy_allowlist_lists_native_names_and_no_mcp_name() {
        let (text, lossy) = agy(
            "---\nname: r\nmode: subagent\ntools:\n  \"*\": deny\n  read: allow\n  bash: allow\n  mcp(srv/ping): allow\n---\n# body",
        );
        assert!(
            text.contains("tools:\n- read_file\n- run_command\n"),
            "{text}"
        );
        assert!(
            !text.contains("mainAgent") && !text.contains("subagent:"),
            "{text}"
        );
        assert!(
            lossy
                .iter()
                .any(|f| f.field == "mode" && f.classification == Lossiness::MeridianOnly)
        );
        assert!(
            !lossy
                .iter()
                .any(|f| matches!(f.classification, Lossiness::Dropped(_))),
            "an allowlist the file carries loses nothing: {lossy:?}"
        );
    }

    #[test]
    fn agy_leaves_a_whole_tool_deny_out_of_the_list_and_reports_a_scoped_one() {
        let (text, lossy) = agy(
            "---\nname: r\nmode: primary\ntools:\n  edit: deny\n  bash(rm *): deny\n---\n# body",
        );
        assert!(
            text.contains("- run_command\n") && !text.contains("'*'"),
            "{text}"
        );
        assert!(
            !text.contains("write_to_file") && !text.contains("replace_file_content"),
            "a denied tool is not listed: {text}"
        );
        assert!(
            !lossy.iter().any(|f| f.field.contains("deny edit")),
            "the list carries the deny: {lossy:?}"
        );
        assert!(lossy.iter().any(|f| f.field.contains("deny bash(rm *)")
            && matches!(f.classification, Lossiness::Dropped(_))));
    }

    #[test]
    fn agy_reports_a_scoped_allow_it_leaves_out_of_an_allowlist() {
        let (text, lossy) = agy(
            "---\nname: r\ntools:\n  '*': deny\n  read: allow\n  bash(npx test *): allow\n---\n# body",
        );
        assert!(!text.contains("run_command"), "{text}");
        assert!(
            lossy
                .iter()
                .any(|f| f.field.starts_with("shell (allow bash(npx test *)")
                    && matches!(f.classification, Lossiness::Dropped(_))),
            "{lossy:?}"
        );
    }

    #[test]
    fn agy_never_lists_view_file_beside_read_file() {
        let (text, _) = agy(
            "---\nname: r\ntools:\n  \"*\": deny\n  read: allow\n  edit: allow\n  ls: allow\n---\n# body",
        );
        assert!(
            text.contains("read_file") && !text.contains("view_file"),
            "{text}"
        );
    }

    // --- 3.3: Dispatch ---
}
