//! Declarative per-harness skill lowering policies and shared pipeline.

use serde_yaml::{Mapping, Value};

use crate::compiler::agents::HarnessKind;
use crate::compiler::harness_descriptor::{self, SkillLoweringPolicyKind};
use crate::compiler::lossiness::{Lossiness, LossyField, LoweredOutput, LoweredSibling};
use crate::compiler::mcp_ref::project_mcp_refs_for_emission;
use crate::compiler::skills::SkillProfile;
use crate::compiler::tool_names::{ToolProjectionStatus, project_tool_for_harness};

// ---------------------------------------------------------------------------
// Policy axes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum ModelInvocablePolicy {
    /// Emit `disable-model-invocation: true` when `model_invocable` is false.
    EmitDisableWhenFalse,
    /// Drop silently when `model_invocable` is false — no lossiness warning.
    ///
    /// Used for OpenCode, whose skill frontmatter exposes no model-invocation
    /// gate: the canonical `model-invocable` axis is structurally unrepresentable
    /// there. The drop is uniform across every non-model-invocable skill and not
    /// actionable by the author, so surfacing it per-skill is pure noise — it is
    /// treated as a known, accepted target limitation rather than per-item loss.
    DropSilentlyWhenFalse,
    /// Emit sibling `openai.yaml` when source explicitly set `model-invocable: false`.
    /// Explicit `true` and absent are no-ops: Codex defaults `allow_implicit_invocation` to true.
    EmitCodexOpenaiYamlWhenExplicitFalse,
    /// Cursor: handled by [`LoweringStep::CursorRuleMode`], not this step.
    CursorRuleMode,
}

#[derive(Debug, Clone, Copy)]
enum UserInvocablePolicy {
    /// Emit `user-invocable: false` when user invocation is disabled.
    EmitFalseWhenDisabled,
    /// Warn-drop when user invocation is disabled.
    DropWhenDisabled,
}

#[derive(Debug, Clone, Copy)]
enum AllowedToolsPolicy {
    /// Lower allowlist to harness-native `allowed-tools`.
    Emit { track_unknown_tool_lossiness: bool },
    /// Warn-drop when allowlist is non-empty.
    DropWhenNonEmpty,
}

#[derive(Debug, Clone, Copy)]
enum DisallowedToolsPolicy {
    /// Lower denylist to harness-native `disallowed-tools`.
    Emit,
    /// Warn-drop when denylist is non-empty.
    Drop,
}

#[derive(Debug, Clone, Copy)]
enum McpToolsPolicy {
    Emit,
    Approximate(&'static str),
    Drop,
}

#[derive(Debug, Clone, Copy)]
enum WhenToUsePolicy {
    /// Emit native `when_to_use` (Claude, Pi).
    Emit,
    /// Fold into native `description` (Cursor Apply Intelligently, Codex, OpenCode).
    FoldIntoDescription,
}

/// Ordered lowering phases — sequence differs per harness (lossiness order matters).
#[derive(Debug, Clone, Copy)]
enum LoweringStep {
    Identity,
    LicenseMetadata,
    Passthrough,
    ModelInvocable,
    UserInvocable,
    AllowedTools,
    DisallowedTools,
    McpTools,
    WhenToUse,
    /// Pi records user-invocable lossiness after passthrough is inserted.
    UserInvocableLossinessLate,
    /// Cursor-only rule mode: `alwaysApply` + reconcile `description` for Manual vs Intelligent.
    CursorRuleMode,
}

#[derive(Debug, Clone, Copy)]
struct SkillLoweringPolicy {
    target_name: &'static str,
    steps: &'static [LoweringStep],
    model_invocable: ModelInvocablePolicy,
    user_invocable: UserInvocablePolicy,
    allowed_tools: AllowedToolsPolicy,
    disallowed_tools: DisallowedToolsPolicy,
    mcp: McpToolsPolicy,
    when_to_use: WhenToUsePolicy,
}

const CLAUDE_POLICY: SkillLoweringPolicy = SkillLoweringPolicy {
    target_name: "Claude",
    steps: &[
        LoweringStep::Identity,
        LoweringStep::ModelInvocable,
        LoweringStep::UserInvocable,
        LoweringStep::AllowedTools,
        LoweringStep::DisallowedTools,
        LoweringStep::McpTools,
        LoweringStep::WhenToUse,
        LoweringStep::LicenseMetadata,
        LoweringStep::Passthrough,
    ],
    model_invocable: ModelInvocablePolicy::EmitDisableWhenFalse,
    user_invocable: UserInvocablePolicy::EmitFalseWhenDisabled,
    allowed_tools: AllowedToolsPolicy::Emit {
        track_unknown_tool_lossiness: true,
    },
    disallowed_tools: DisallowedToolsPolicy::Emit,
    mcp: McpToolsPolicy::Emit,
    when_to_use: WhenToUsePolicy::Emit,
};

const CODEX_POLICY: SkillLoweringPolicy = SkillLoweringPolicy {
    target_name: "Codex",
    steps: &[
        LoweringStep::Identity,
        LoweringStep::LicenseMetadata,
        LoweringStep::Passthrough,
        LoweringStep::ModelInvocable,
        LoweringStep::AllowedTools,
        LoweringStep::DisallowedTools,
        LoweringStep::McpTools,
        LoweringStep::UserInvocable,
        LoweringStep::WhenToUse,
    ],
    model_invocable: ModelInvocablePolicy::EmitCodexOpenaiYamlWhenExplicitFalse,
    user_invocable: UserInvocablePolicy::DropWhenDisabled,
    allowed_tools: AllowedToolsPolicy::DropWhenNonEmpty,
    disallowed_tools: DisallowedToolsPolicy::Drop,
    mcp: McpToolsPolicy::Approximate("Codex uses -c mcp.servers.<name>.command"),
    when_to_use: WhenToUsePolicy::FoldIntoDescription,
};

const OPENCODE_POLICY: SkillLoweringPolicy = SkillLoweringPolicy {
    target_name: "OpenCode",
    steps: &[
        LoweringStep::Identity,
        LoweringStep::LicenseMetadata,
        LoweringStep::Passthrough,
        LoweringStep::ModelInvocable,
        LoweringStep::UserInvocable,
        LoweringStep::AllowedTools,
        LoweringStep::DisallowedTools,
        LoweringStep::McpTools,
        LoweringStep::WhenToUse,
    ],
    model_invocable: ModelInvocablePolicy::DropSilentlyWhenFalse,
    user_invocable: UserInvocablePolicy::DropWhenDisabled,
    allowed_tools: AllowedToolsPolicy::DropWhenNonEmpty,
    disallowed_tools: DisallowedToolsPolicy::Drop,
    mcp: McpToolsPolicy::Approximate(
        "MCP grants on subprocess errors; streaming uses session payload",
    ),
    when_to_use: WhenToUsePolicy::FoldIntoDescription,
};

const PI_POLICY: SkillLoweringPolicy = SkillLoweringPolicy {
    target_name: "Pi",
    steps: &[
        LoweringStep::Identity,
        LoweringStep::ModelInvocable,
        LoweringStep::AllowedTools,
        LoweringStep::DisallowedTools,
        LoweringStep::McpTools,
        LoweringStep::WhenToUse,
        LoweringStep::LicenseMetadata,
        LoweringStep::Passthrough,
        LoweringStep::UserInvocableLossinessLate,
    ],
    model_invocable: ModelInvocablePolicy::EmitDisableWhenFalse,
    user_invocable: UserInvocablePolicy::DropWhenDisabled,
    allowed_tools: AllowedToolsPolicy::Emit {
        track_unknown_tool_lossiness: false,
    },
    disallowed_tools: DisallowedToolsPolicy::Emit,
    mcp: McpToolsPolicy::Drop,
    when_to_use: WhenToUsePolicy::Emit,
};

const CURSOR_POLICY: SkillLoweringPolicy = SkillLoweringPolicy {
    target_name: "Cursor",
    steps: &[
        LoweringStep::Identity,
        LoweringStep::LicenseMetadata,
        LoweringStep::Passthrough,
        LoweringStep::CursorRuleMode,
        LoweringStep::AllowedTools,
        LoweringStep::DisallowedTools,
        LoweringStep::McpTools,
        LoweringStep::UserInvocable,
        LoweringStep::WhenToUse,
    ],
    model_invocable: ModelInvocablePolicy::CursorRuleMode,
    user_invocable: UserInvocablePolicy::DropWhenDisabled,
    allowed_tools: AllowedToolsPolicy::DropWhenNonEmpty,
    disallowed_tools: DisallowedToolsPolicy::Drop,
    mcp: McpToolsPolicy::Approximate(
        "MCP grants on subprocess errors; streaming uses session payload",
    ),
    when_to_use: WhenToUsePolicy::FoldIntoDescription,
};

fn policy_for(harness: HarnessKind) -> &'static SkillLoweringPolicy {
    match harness_descriptor::descriptor(harness).skill_policy {
        SkillLoweringPolicyKind::Claude => &CLAUDE_POLICY,
        SkillLoweringPolicyKind::Codex => &CODEX_POLICY,
        SkillLoweringPolicyKind::OpenCode => &OPENCODE_POLICY,
        SkillLoweringPolicyKind::Pi => &PI_POLICY,
        SkillLoweringPolicyKind::Cursor => &CURSOR_POLICY,
    }
}

// ---------------------------------------------------------------------------
// Shared pipeline
// ---------------------------------------------------------------------------

fn yk(s: &str) -> Value {
    Value::String(s.to_string())
}
fn ys(s: &str) -> Value {
    Value::String(s.to_string())
}

// Skill user-invocable lowering keys off the resolved value only — unlike agent
// lowering, it does not need to remember whether the source field was explicit.
fn user_invocation_disabled(profile: &SkillProfile) -> bool {
    !profile.user_invocable
}

/// Cursor Manual: explicit `model-invocable: false` — @-mention only, no auto-selection hook.
fn cursor_manual_mode(profile: &SkillProfile) -> bool {
    profile.had_model_invocable_field && !profile.model_invocable
}

/// Selection guidance for harnesses that fold `when_to_use` into native `description`.
fn selection_description(profile: &SkillProfile) -> Option<String> {
    match (&profile.description, &profile.when_to_use) {
        (Some(desc), Some(wtu)) => Some(format!("{desc}\n\n{wtu}")),
        (Some(desc), None) => Some(desc.clone()),
        (None, Some(wtu)) => Some(wtu.clone()),
        (None, None) => None,
    }
}

fn dropped(field: &str, target: &str) -> LossyField {
    LossyField {
        field: field.to_string(),
        target: target.to_string(),
        classification: Lossiness::Dropped,
    }
}

fn render(yaml: Mapping, body: &str) -> Vec<u8> {
    if yaml.is_empty() {
        return body.as_bytes().to_vec();
    }
    let mut yaml_str = serde_yaml::to_string(&yaml).expect("skill frontmatter should serialize");
    if let Some(stripped) = yaml_str.strip_prefix("---\n") {
        yaml_str = stripped.to_string();
    }
    let mut out = String::from("---\n");
    out.push_str(&yaml_str);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("---\n");
    out.push_str(body);
    out.into_bytes()
}

struct LoweringCtx<'a> {
    harness_kind: HarnessKind,
    policy: &'static SkillLoweringPolicy,
    profile: &'a SkillProfile,
    yaml: Mapping,
    lossy_fields: Vec<LossyField>,
    siblings: Vec<LoweredSibling>,
}

impl<'a> LoweringCtx<'a> {
    fn new(
        harness_kind: HarnessKind,
        policy: &'static SkillLoweringPolicy,
        profile: &'a SkillProfile,
    ) -> Self {
        Self {
            harness_kind,
            policy,
            profile,
            yaml: Mapping::new(),
            lossy_fields: Vec::new(),
            siblings: Vec::new(),
        }
    }

    fn run_step(&mut self, step: LoweringStep) {
        match step {
            LoweringStep::Identity => self.insert_identity(),
            LoweringStep::LicenseMetadata => self.insert_license_metadata(),
            LoweringStep::Passthrough => self.insert_passthrough(),
            LoweringStep::ModelInvocable => self.apply_model_invocable(),
            LoweringStep::UserInvocable => self.apply_user_invocable(),
            LoweringStep::AllowedTools => self.apply_allowed_tools(),
            LoweringStep::DisallowedTools => self.apply_disallowed_tools(),
            LoweringStep::McpTools => self.apply_mcp(),
            LoweringStep::WhenToUse => self.apply_when_to_use(),
            LoweringStep::UserInvocableLossinessLate => self.apply_user_invocable_lossiness(),
            LoweringStep::CursorRuleMode => self.apply_cursor_rule_mode(),
        }
    }

    fn insert_identity(&mut self) {
        let profile = self.profile;
        if let Some(name) = &profile.name {
            self.yaml.insert(yk("name"), ys(name));
        }
        if let Some(description) = &profile.description {
            self.yaml.insert(yk("description"), ys(description));
        }
    }

    fn insert_license_metadata(&mut self) {
        let profile = self.profile;
        if let Some(license) = &profile.license {
            self.yaml.insert(yk("license"), ys(license));
        }
        if let Some(metadata) = &profile.metadata {
            self.yaml.insert(yk("metadata"), metadata.clone());
        }
    }

    fn insert_passthrough(&mut self) {
        for (key, value) in &self.profile.passthrough_fields {
            self.yaml.insert(yk(key), value.clone());
        }
    }

    fn apply_model_invocable(&mut self) {
        let profile = self.profile;
        let policy = self.policy;
        match policy.model_invocable {
            ModelInvocablePolicy::EmitDisableWhenFalse => {
                if !profile.model_invocable {
                    self.yaml
                        .insert(yk("disable-model-invocation"), Value::Bool(true));
                }
            }
            ModelInvocablePolicy::DropSilentlyWhenFalse => {
                // OpenCode has no model-invocation gate; the axis is structurally
                // unrepresentable. Drop without lossiness — see the variant doc.
            }
            ModelInvocablePolicy::EmitCodexOpenaiYamlWhenExplicitFalse => {
                // Codex skills are model-invocable by default; only explicit false needs
                // the openai.yaml sibling to set allow_implicit_invocation: false.
                if profile.had_model_invocable_field && !profile.model_invocable {
                    self.siblings.push(codex_openai_yaml_sibling());
                }
            }
            ModelInvocablePolicy::CursorRuleMode => {}
        }
    }

    fn apply_user_invocable(&mut self) {
        match self.policy.user_invocable {
            UserInvocablePolicy::EmitFalseWhenDisabled => {
                if user_invocation_disabled(self.profile) {
                    self.yaml.insert(yk("user-invocable"), Value::Bool(false));
                }
            }
            UserInvocablePolicy::DropWhenDisabled => {
                self.apply_user_invocable_lossiness();
            }
        }
    }

    fn apply_user_invocable_lossiness(&mut self) {
        if user_invocation_disabled(self.profile) {
            self.lossy_fields
                .push(dropped("user-invocable", self.policy.target_name));
        }
    }

    fn apply_allowed_tools(&mut self) {
        let tool_policy = self.profile.effective_tool_policy();
        let policy = self.policy;
        match policy.allowed_tools {
            AllowedToolsPolicy::Emit {
                track_unknown_tool_lossiness,
            } => {
                if tool_policy.allowed.is_empty() {
                    return;
                }
                let mut tools = Vec::new();
                for tool in &tool_policy.allowed {
                    let projected = project_tool_for_harness(tool, self.harness_kind);
                    if track_unknown_tool_lossiness
                        && projected.status == ToolProjectionStatus::UnknownProjected
                    {
                        self.lossy_fields.push(LossyField {
                            field: "tools".into(),
                            target: policy.target_name.into(),
                            classification: Lossiness::Approximate {
                                note: "unknown tool projected via harness naming convention",
                            },
                        });
                    }
                    tools.push(projected.name);
                }
                self.yaml.insert(
                    yk("allowed-tools"),
                    Value::Sequence(tools.iter().map(|s| ys(s)).collect()),
                );
            }
            AllowedToolsPolicy::DropWhenNonEmpty => {
                if !tool_policy.allowed.is_empty() {
                    self.lossy_fields.push(dropped("tools", policy.target_name));
                }
            }
        }
    }

    fn apply_disallowed_tools(&mut self) {
        let tool_policy = self.profile.effective_tool_policy();
        if tool_policy.disallowed.is_empty() && tool_policy.mcp_disallowed.is_empty() {
            return;
        }
        let policy = self.policy;
        match policy.disallowed_tools {
            DisallowedToolsPolicy::Emit => {
                let mut tools = Vec::new();
                for tool in &tool_policy.disallowed {
                    let projected = project_tool_for_harness(tool, self.harness_kind);
                    if projected.status == ToolProjectionStatus::UnknownProjected {
                        self.lossy_fields.push(LossyField {
                            field: "disallowed-tools".into(),
                            target: policy.target_name.into(),
                            classification: Lossiness::Approximate {
                                note: "unknown tool projected via harness naming convention",
                            },
                        });
                    }
                    tools.push(projected.name);
                }
                let mcp_tokens = project_mcp_refs_for_emission(
                    &tool_policy.mcp_disallowed,
                    self.harness_kind,
                    |_, reason| {
                        self.lossy_fields.push(LossyField {
                            field: "disallowed-tools".into(),
                            target: policy.target_name.into(),
                            classification: Lossiness::Approximate {
                                note: reason.message(),
                            },
                        });
                    },
                );
                tools.extend(mcp_tokens);
                self.yaml.insert(
                    yk("disallowed-tools"),
                    Value::Sequence(tools.iter().map(|s| ys(s)).collect()),
                );
            }
            DisallowedToolsPolicy::Drop => {
                self.lossy_fields
                    .push(dropped("disallowed-tools", policy.target_name));
            }
        }
    }

    fn apply_mcp(&mut self) {
        let tool_policy = self.profile.effective_tool_policy();
        if tool_policy.mcp_allowed.is_empty() {
            return;
        }
        let policy = self.policy;
        match policy.mcp {
            McpToolsPolicy::Emit => {
                let mcp_tokens = project_mcp_refs_for_emission(
                    &tool_policy.mcp_allowed,
                    self.harness_kind,
                    |_, reason| {
                        self.lossy_fields.push(LossyField {
                            field: "mcp".into(),
                            target: policy.target_name.into(),
                            classification: Lossiness::Approximate {
                                note: reason.message(),
                            },
                        });
                    },
                );
                if mcp_tokens.is_empty() {
                    return;
                }
                let mut tools = match self.yaml.get(yk("allowed-tools")) {
                    Some(Value::Sequence(seq)) => seq
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect::<Vec<_>>(),
                    _ => Vec::new(),
                };
                tools.extend(mcp_tokens);
                self.yaml.insert(
                    yk("allowed-tools"),
                    Value::Sequence(tools.iter().map(|s| ys(s)).collect()),
                );
                self.lossy_fields.push(LossyField {
                    field: "mcp".into(),
                    target: policy.target_name.into(),
                    classification: Lossiness::Approximate {
                        note: "Claude skill allowed-tools grants MCP access; it does not restrict invocation",
                    },
                });
            }
            McpToolsPolicy::Approximate(note) => {
                self.lossy_fields.push(LossyField {
                    field: "mcp".into(),
                    target: policy.target_name.into(),
                    classification: Lossiness::Approximate { note },
                });
            }
            McpToolsPolicy::Drop => {
                self.lossy_fields.push(dropped("mcp", policy.target_name));
            }
        }
    }

    fn apply_when_to_use(&mut self) {
        let profile = self.profile;
        match self.policy.when_to_use {
            WhenToUsePolicy::Emit => {
                if let Some(when_to_use) = &profile.when_to_use {
                    self.yaml.insert(yk("when_to_use"), ys(when_to_use));
                }
            }
            WhenToUsePolicy::FoldIntoDescription => {
                // Manual Cursor skills must not carry a description selection hook.
                if self.policy.model_invocable == ModelInvocablePolicy::CursorRuleMode
                    && cursor_manual_mode(profile)
                {
                    return;
                }
                if let Some(description) = selection_description(profile) {
                    self.yaml.insert(yk("description"), ys(&description));
                }
            }
        }
    }

    fn apply_cursor_rule_mode(&mut self) {
        if !self.profile.has_frontmatter {
            return;
        }
        // Cursor rule modes (2026): alwaysApply + description/globs control activation.
        // Never emit alwaysApply: true — that is Always-on, not model-invocable.
        // Intelligent (default or explicit true): alwaysApply:false + description hook.
        // Manual (explicit false): alwaysApply:false, strip description so Cursor won't auto-select.
        self.yaml.insert(yk("alwaysApply"), Value::Bool(false));
        if cursor_manual_mode(self.profile) {
            self.yaml.remove(yk("description"));
        }
    }

    fn finish(self, body: &str) -> LoweredOutput {
        LoweredOutput {
            bytes: render(self.yaml, body),
            lossy_fields: self.lossy_fields,
            siblings: self.siblings,
        }
    }
}

fn codex_openai_yaml_sibling() -> LoweredSibling {
    #[derive(serde::Serialize)]
    struct Policy {
        allow_implicit_invocation: bool,
    }
    #[derive(serde::Serialize)]
    struct OpenaiYaml {
        policy: Policy,
    }
    let doc = OpenaiYaml {
        policy: Policy {
            allow_implicit_invocation: false,
        },
    };
    LoweredSibling {
        rel_path: "openai.yaml".into(),
        bytes: serde_yaml::to_string(&doc)
            .expect("Codex openai.yaml policy should serialize")
            .into_bytes(),
    }
}

pub(super) fn lower_skill_with_policy(
    harness: HarnessKind,
    profile: &SkillProfile,
    body: &str,
) -> LoweredOutput {
    let policy = policy_for(harness);
    let mut ctx = LoweringCtx::new(harness, policy, profile);
    for &step in policy.steps {
        ctx.run_step(step);
    }
    ctx.finish(body)
}
