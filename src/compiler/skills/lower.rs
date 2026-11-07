//! Per-harness lowering for universal skill frontmatter.

#[path = "lower_policy.rs"]
mod lower_policy;

use crate::compiler::harness_descriptor::{
    SkillLoweringPolicyKind, descriptor, descriptor_for_variant_key,
};
use crate::compiler::lossiness::LoweredOutput;
use mars_schema::HarnessId;
use mars_schema::skill::SkillProfile;

pub type SkillHarness = HarnessId;

pub fn skill_harness_from_variant_key(key: &str) -> Option<SkillHarness> {
    descriptor_for_variant_key(key).map(|descriptor| descriptor.kind)
}
pub fn lower_skill_for_harness(
    harness: SkillHarness,
    profile: &SkillProfile,
    body: &str,
) -> LoweredOutput {
    match descriptor(harness).skill_policy {
        SkillLoweringPolicyKind::Claude => lower_skill_to_claude(profile, body),
        SkillLoweringPolicyKind::Codex => lower_skill_to_codex(profile, body),
        SkillLoweringPolicyKind::OpenCode => lower_skill_to_opencode(profile, body),
        SkillLoweringPolicyKind::Pi => lower_skill_to_pi(profile, body),
        SkillLoweringPolicyKind::Cursor => lower_skill_to_cursor(profile, body),
    }
}

pub fn lower_skill_to_claude(profile: &SkillProfile, body: &str) -> LoweredOutput {
    lower_policy::lower_skill_with_policy(HarnessId::Claude, profile, body)
}

pub fn lower_skill_to_codex(profile: &SkillProfile, body: &str) -> LoweredOutput {
    lower_policy::lower_skill_with_policy(HarnessId::Codex, profile, body)
}

pub fn lower_skill_to_opencode(profile: &SkillProfile, body: &str) -> LoweredOutput {
    lower_policy::lower_skill_with_policy(HarnessId::OpenCode, profile, body)
}

pub fn lower_skill_to_pi(profile: &SkillProfile, body: &str) -> LoweredOutput {
    lower_policy::lower_skill_with_policy(HarnessId::Pi, profile, body)
}

pub fn lower_skill_to_cursor(profile: &SkillProfile, body: &str) -> LoweredOutput {
    lower_policy::lower_skill_with_policy(HarnessId::Cursor, profile, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::lossiness::{Lossiness, LossyField};

    fn parse_profile(content: &str) -> SkillProfile {
        mars_schema::skill::read(content).fields.unwrap()
    }

    fn profile() -> SkillProfile {
        parse_profile(
            "---\nname: skill\ndescription: desc\nmodel-invocable: false\ntools:\n  Bash(git *): allow\nlicense: MIT\nmetadata:\n  owner: team\nextra: stripped\n---\nBody\n",
        )
    }

    fn user_invocable_false_profile() -> SkillProfile {
        parse_profile("---\nname: skill\ndescription: desc\nuser-invocable: false\n---\nBody\n")
    }

    fn both_false_profile() -> SkillProfile {
        parse_profile(
            "---\nname: skill\ndescription: desc\nmodel-invocable: false\nuser-invocable: false\n---\nBody\n",
        )
    }

    fn is_quiet(lossy_fields: &[LossyField], field: &str, target: &str) -> bool {
        lossy_fields.iter().any(|f| {
            f.field == field
                && f.target == target
                && matches!(f.classification, Lossiness::Quiet { .. })
        })
    }

    #[test]
    fn claude_emits_user_invocable_false() {
        let lowered = lower_skill_to_claude(&user_invocable_false_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("user-invocable: false"));
        assert!(lowered.lossy_fields.is_empty());
    }

    #[test]
    fn claude_both_false() {
        let lowered = lower_skill_to_claude(&both_false_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("disable-model-invocation: true"));
        assert!(out.contains("user-invocable: false"));
        assert!(lowered.lossy_fields.is_empty());
    }

    #[test]
    fn codex_explicit_false_emits_openai_yaml_no_lossiness() {
        let profile = parse_profile(
            "---\nname: skill\ndescription: desc\nmodel-invocable: false\n---\nBody\n",
        );
        let lowered = lower_skill_to_codex(&profile, "Body\n");
        assert!(!is_quiet(&lowered.lossy_fields, "model-invocable", "Codex"));
        assert_eq!(lowered.siblings.len(), 1);
        let sibling = String::from_utf8(lowered.siblings[0].bytes.clone()).unwrap();
        assert!(sibling.contains("policy:"));
        assert!(sibling.contains("allow_implicit_invocation: false"));
    }

    #[test]
    fn codex_drops_user_invocable_false() {
        let lowered = lower_skill_to_codex(&user_invocable_false_profile(), "Body\n");
        assert!(is_quiet(&lowered.lossy_fields, "user-invocable", "Codex"));
    }

    #[test]
    fn opencode_drops_user_invocable_false() {
        let lowered = lower_skill_to_opencode(&user_invocable_false_profile(), "Body\n");
        assert!(is_quiet(
            &lowered.lossy_fields,
            "user-invocable",
            "OpenCode"
        ));
    }

    #[test]
    fn pi_model_false_emits_disable_model_invocation() {
        let lowered = lower_skill_to_pi(&profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("disable-model-invocation: true"));
    }

    #[test]
    fn pi_drops_user_invocable_false() {
        let lowered = lower_skill_to_pi(&user_invocable_false_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("user-invocable"));
        assert!(is_quiet(&lowered.lossy_fields, "user-invocable", "Pi"));
    }

    #[test]
    fn cursor_drops_user_invocable_false() {
        let lowered = lower_skill_to_cursor(&user_invocable_false_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("user-invocable"));
        assert!(is_quiet(&lowered.lossy_fields, "user-invocable", "Cursor"));
    }
}
