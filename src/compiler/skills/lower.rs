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
    use mars_schema::skill::parse_skill_content;

    fn parse_profile(content: &str) -> SkillProfile {
        let mut diags = Vec::new();
        parse_skill_content(content, &mut diags).unwrap().0
    }

    fn profile() -> SkillProfile {
        parse_profile(
            "---\nname: skill\ndescription: desc\nmodel-invocable: false\ntools:\n  Bash(git *): allow\nlicense: MIT\nmetadata:\n  owner: team\nextra: stripped\n---\nBody\n",
        )
    }

    fn identity_profile() -> SkillProfile {
        parse_profile("---\nname: skill\ndescription: desc\n---\nBody\n")
    }

    fn user_invocable_false_profile() -> SkillProfile {
        parse_profile("---\nname: skill\ndescription: desc\nuser-invocable: false\n---\nBody\n")
    }

    fn explicit_true_profile() -> SkillProfile {
        parse_profile(
            "---\nname: skill\ndescription: desc\nmodel-invocable: true\nuser-invocable: true\n---\nBody\n",
        )
    }

    fn both_false_profile() -> SkillProfile {
        parse_profile(
            "---\nname: skill\ndescription: desc\nmodel-invocable: false\nuser-invocable: false\n---\nBody\n",
        )
    }

    fn has_dropped(lossy_fields: &[LossyField], field: &str, target: &str) -> bool {
        lossy_fields.iter().any(|f| {
            f.field == field && f.target == target && f.classification == Lossiness::Dropped
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
    fn claude_omits_user_invocable_when_true() {
        let out =
            String::from_utf8(lower_skill_to_claude(&explicit_true_profile(), "Body\n").bytes)
                .unwrap();
        assert!(!out.contains("user-invocable"));
    }

    #[test]
    fn claude_omits_disable_model_invocation_when_true() {
        let out =
            String::from_utf8(lower_skill_to_claude(&explicit_true_profile(), "Body\n").bytes)
                .unwrap();
        assert!(!out.contains("disable-model-invocation"));
        assert!(!out.contains("user-invocable"));
        assert!(!out.contains("allow_implicit_invocation"));
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
        assert!(!has_dropped(
            &lowered.lossy_fields,
            "model-invocable",
            "Codex"
        ));
        assert_eq!(lowered.siblings.len(), 1);
        let sibling = String::from_utf8(lowered.siblings[0].bytes.clone()).unwrap();
        assert!(sibling.contains("policy:"));
        assert!(sibling.contains("allow_implicit_invocation: false"));
    }

    #[test]
    fn codex_default_or_absent_model_invocable_emits_no_sibling() {
        let lowered = lower_skill_to_codex(&identity_profile(), "Body\n");
        assert!(lowered.siblings.is_empty());
        assert!(!has_dropped(
            &lowered.lossy_fields,
            "model-invocable",
            "Codex"
        ));
    }

    #[test]
    fn codex_identity_only_does_not_gain_invocation_field() {
        let out =
            String::from_utf8(lower_skill_to_codex(&identity_profile(), "Body\n").bytes).unwrap();
        assert!(out.contains("name: skill"));
        assert!(out.contains("description: desc"));
        assert!(!out.contains("allow_implicit_invocation"));
    }

    #[test]
    fn codex_explicit_true_is_no_op() {
        let lowered = lower_skill_to_codex(&explicit_true_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("allow_implicit_invocation"));
        assert!(!out.contains("disable-model-invocation"));
        assert!(lowered.siblings.is_empty());
        assert!(!has_dropped(
            &lowered.lossy_fields,
            "model-invocable",
            "Codex"
        ));
        assert!(!has_dropped(
            &lowered.lossy_fields,
            "user-invocable",
            "Codex"
        ));
    }

    #[test]
    fn codex_drops_user_invocable_false() {
        let lowered = lower_skill_to_codex(&user_invocable_false_profile(), "Body\n");
        assert!(has_dropped(
            &lowered.lossy_fields,
            "user-invocable",
            "Codex"
        ));
    }

    #[test]
    fn codex_no_lossiness_user_invocable_true() {
        let lowered = lower_skill_to_codex(&identity_profile(), "Body\n");
        assert!(!has_dropped(
            &lowered.lossy_fields,
            "user-invocable",
            "Codex"
        ));
    }

    #[test]
    fn opencode_drops_user_invocable_false() {
        let lowered = lower_skill_to_opencode(&user_invocable_false_profile(), "Body\n");
        assert!(has_dropped(
            &lowered.lossy_fields,
            "user-invocable",
            "OpenCode"
        ));
    }

    #[test]
    fn opencode_no_invocability_lossiness_when_defaults() {
        let lowered = lower_skill_to_opencode(&identity_profile(), "Body\n");
        assert!(!has_dropped(
            &lowered.lossy_fields,
            "model-invocable",
            "OpenCode"
        ));
        assert!(!has_dropped(
            &lowered.lossy_fields,
            "user-invocable",
            "OpenCode"
        ));
        assert!(lowered.lossy_fields.is_empty());
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
        assert!(has_dropped(&lowered.lossy_fields, "user-invocable", "Pi"));
    }

    #[test]
    fn pi_model_true_omits_disable_model_invocation_and_user_true_no_lossiness() {
        let lowered = lower_skill_to_pi(&explicit_true_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("disable-model-invocation"));
        assert!(!out.contains("user-invocable"));
        assert!(!has_dropped(&lowered.lossy_fields, "user-invocable", "Pi"));
    }

    #[test]
    fn cursor_drops_user_invocable_false() {
        let lowered = lower_skill_to_cursor(&user_invocable_false_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("user-invocable"));
        assert!(has_dropped(
            &lowered.lossy_fields,
            "user-invocable",
            "Cursor"
        ));
    }

    #[test]
    fn cursor_intelligent_emits_always_apply_false_with_description() {
        let lowered = lower_skill_to_cursor(&explicit_true_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("alwaysApply: false"));
        assert!(out.contains("description: desc"));
        assert!(!out.contains("alwaysApply: true"));
        assert!(!out.contains("disable-model-invocation"));
        assert!(!out.contains("user-invocable"));
        assert!(!has_dropped(
            &lowered.lossy_fields,
            "model-invocable",
            "Cursor"
        ));
        assert!(!has_dropped(
            &lowered.lossy_fields,
            "user-invocable",
            "Cursor"
        ));
    }

    #[test]
    fn cursor_intelligent_default_emits_always_apply_false_with_description() {
        let lowered = lower_skill_to_cursor(&identity_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("alwaysApply: false"));
        assert!(out.contains("description: desc"));
        assert!(!has_dropped(
            &lowered.lossy_fields,
            "model-invocable",
            "Cursor"
        ));
    }

    #[test]
    fn cursor_manual_strips_description() {
        let lowered = lower_skill_to_cursor(&profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("alwaysApply: false"));
        assert!(!out.contains("description:"));
        assert!(!has_dropped(
            &lowered.lossy_fields,
            "model-invocable",
            "Cursor"
        ));
    }

    #[test]
    fn no_frontmatter_body_only_all_harnesses() {
        let mut diags = Vec::new();
        let (profile, fm) = parse_skill_content("# Body\nbytes", &mut diags).unwrap();
        let body = fm.body();
        for harness in [
            HarnessId::Claude,
            HarnessId::Codex,
            HarnessId::OpenCode,
            HarnessId::Pi,
            HarnessId::Cursor,
        ] {
            let out =
                String::from_utf8(lower_skill_for_harness(harness, &profile, body).bytes).unwrap();
            assert_eq!(out, "# Body\nbytes");
        }
    }
}
