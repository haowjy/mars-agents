//! Declarative per-harness skill lowering policies and shared pipeline.

use serde_yaml::{Mapping, Value};

use crate::compiler::harness_descriptor::{self, SkillLoweringPolicyKind};
use crate::compiler::lossiness::{Lossiness, LossyField, LoweredOutput, LoweredSibling};
use mars_schema::HarnessId;
use mars_schema::skill::SkillProfile;

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

/// Ordered lowering phases — sequence differs per harness (lossiness order matters).
#[derive(Debug, Clone, Copy)]
enum LoweringStep {
    Identity,
    LicenseMetadata,
    Passthrough,
    ModelInvocable,
    UserInvocable,
    /// Pi records user-invocable lossiness after passthrough is inserted.
    UserInvocableLossinessLate,
    /// Cursor-only rule mode: `alwaysApply` + reconcile `description` for Manual vs Intelligent.
    CursorRuleMode,
}

#[derive(Debug, Clone, Copy)]
struct SkillLoweringPolicy {
    steps: &'static [LoweringStep],
    model_invocable: ModelInvocablePolicy,
    user_invocable: UserInvocablePolicy,
}

const CLAUDE_POLICY: SkillLoweringPolicy = SkillLoweringPolicy {
    steps: &[
        LoweringStep::Identity,
        LoweringStep::ModelInvocable,
        LoweringStep::UserInvocable,
        LoweringStep::LicenseMetadata,
        LoweringStep::Passthrough,
    ],
    model_invocable: ModelInvocablePolicy::EmitDisableWhenFalse,
    user_invocable: UserInvocablePolicy::EmitFalseWhenDisabled,
};

const CODEX_POLICY: SkillLoweringPolicy = SkillLoweringPolicy {
    steps: &[
        LoweringStep::Identity,
        LoweringStep::LicenseMetadata,
        LoweringStep::Passthrough,
        LoweringStep::ModelInvocable,
        LoweringStep::UserInvocable,
    ],
    model_invocable: ModelInvocablePolicy::EmitCodexOpenaiYamlWhenExplicitFalse,
    user_invocable: UserInvocablePolicy::DropWhenDisabled,
};

const OPENCODE_POLICY: SkillLoweringPolicy = SkillLoweringPolicy {
    steps: &[
        LoweringStep::Identity,
        LoweringStep::LicenseMetadata,
        LoweringStep::Passthrough,
        LoweringStep::ModelInvocable,
        LoweringStep::UserInvocable,
    ],
    model_invocable: ModelInvocablePolicy::DropSilentlyWhenFalse,
    user_invocable: UserInvocablePolicy::DropWhenDisabled,
};

const PI_POLICY: SkillLoweringPolicy = SkillLoweringPolicy {
    steps: &[
        LoweringStep::Identity,
        LoweringStep::ModelInvocable,
        LoweringStep::LicenseMetadata,
        LoweringStep::Passthrough,
        LoweringStep::UserInvocableLossinessLate,
    ],
    model_invocable: ModelInvocablePolicy::EmitDisableWhenFalse,
    user_invocable: UserInvocablePolicy::DropWhenDisabled,
};

const CURSOR_POLICY: SkillLoweringPolicy = SkillLoweringPolicy {
    steps: &[
        LoweringStep::Identity,
        LoweringStep::LicenseMetadata,
        LoweringStep::Passthrough,
        LoweringStep::CursorRuleMode,
        LoweringStep::UserInvocable,
    ],
    model_invocable: ModelInvocablePolicy::CursorRuleMode,
    user_invocable: UserInvocablePolicy::DropWhenDisabled,
};

fn policy_for(harness: HarnessId) -> &'static SkillLoweringPolicy {
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
    policy: &'static SkillLoweringPolicy,
    target_name: &'static str,
    profile: &'a SkillProfile,
    yaml: Mapping,
    lossy_fields: Vec<LossyField>,
    siblings: Vec<LoweredSibling>,
}

impl<'a> LoweringCtx<'a> {
    fn new(
        harness_kind: HarnessId,
        policy: &'static SkillLoweringPolicy,
        profile: &'a SkillProfile,
    ) -> Self {
        Self {
            policy,
            target_name: harness_descriptor::descriptor(harness_kind).display_name,
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
                .push(dropped("user-invocable", self.target_name));
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
    harness: HarnessId,
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
