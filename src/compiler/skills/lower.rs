//! Per-harness skill lowering: one pipeline that reads the harness's row in `compiler::lowering`.

use serde_yaml::{Mapping, Value};

use crate::compiler::lossiness::{Lossiness, LossyField, Lost, LoweredOutput, LoweredSibling};
use crate::compiler::lowering::{ModelOff, UserOff, lowering};
use mars_schema::HarnessId;
use mars_schema::skill::{self, SkillProfile};

fn yk(s: &str) -> Value {
    Value::String(s.to_string())
}
fn ys(s: &str) -> Value {
    Value::String(s.to_string())
}

// Skill user-invocable lowering keys off the resolved value only, unlike agent lowering.
fn user_invocation_disabled(profile: &SkillProfile) -> bool {
    !profile.user_invocable
}

/// Explicit `model-invocable: false`: Cursor's Manual mode, and the loss elsewhere.
fn model_invocation_explicitly_off(profile: &SkillProfile) -> bool {
    profile.had_model_invocable_field && !profile.model_invocable
}

/// A field the target has no place for, dropped without a warning.
fn dropped(field: &str, target: &str) -> LossyField {
    LossyField {
        field: field.to_string(),
        target: target.to_string(),
        classification: Lossiness::Quiet {
            why: "skills carry no tool or network restriction",
        },
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

fn insert_identity(yaml: &mut Mapping, profile: &SkillProfile) {
    if let Some(name) = &profile.name {
        yaml.insert(yk("name"), ys(name));
    }
    if let Some(description) = &profile.description {
        yaml.insert(yk("description"), ys(description));
    }
}

fn insert_license_metadata(yaml: &mut Mapping, profile: &SkillProfile) {
    if let Some(license) = &profile.license {
        yaml.insert(yk("license"), ys(license));
    }
    if let Some(metadata) = &profile.metadata {
        yaml.insert(yk("metadata"), metadata.clone());
    }
}

/// Write the harness's `harness-overrides` block into the frontmatter. A key Mars lowers
/// itself keeps Mars's value and is reported as dropped; `reserved` also covers keys Mars
/// would lower but had no value for.
fn insert_overrides(
    yaml: &mut Mapping,
    profile: &SkillProfile,
    harness: HarnessId,
    reserved: &[&str],
    lossy_fields: &mut Vec<LossyField>,
) {
    let Some(block) = profile.harness_overrides.get(&harness) else {
        return;
    };
    for (key, value) in block {
        if reserved.contains(&key.as_str()) || yaml.contains_key(yk(key)) {
            lossy_fields.push(dropped(
                &format!("harness-overrides.{harness}.{key}"),
                harness.display_name(),
            ));
        } else {
            yaml.insert(
                yk(key),
                serde_yaml::to_value(value).expect("override value should serialize"),
            );
        }
    }
}

fn insert_disable_model_invocation(yaml: &mut Mapping, profile: &SkillProfile) {
    if !profile.model_invocable {
        yaml.insert(yk("disable-model-invocation"), Value::Bool(true));
    }
}

fn record_user_invocation_loss(
    lossy_fields: &mut Vec<LossyField>,
    profile: &SkillProfile,
    target_name: &str,
) {
    if user_invocation_disabled(profile) {
        lossy_fields.push(dropped("user-invocable", target_name));
    }
}

/// A target with no switch for `model-invocable: false` keeps the skill selectable by the model.
fn record_model_invocation_loss(
    lossy_fields: &mut Vec<LossyField>,
    profile: &SkillProfile,
    target_name: &str,
) {
    if model_invocation_explicitly_off(profile) {
        lossy_fields.push(LossyField {
            field: "model-invocable".to_string(),
            target: target_name.to_string(),
            classification: Lossiness::Dropped(Lost::Invocation),
        });
    }
}

fn apply_cursor_rule_mode(yaml: &mut Mapping, profile: &SkillProfile) {
    if !profile.has_frontmatter {
        return;
    }
    // Cursor's Manual and Intelligent modes both require alwaysApply:false.
    yaml.insert(yk("alwaysApply"), Value::Bool(false));
    if model_invocation_explicitly_off(profile) {
        yaml.remove(yk("description"));
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

pub fn lower_skill_for_harness(
    harness: HarnessId,
    profile: &SkillProfile,
    body: &str,
) -> LoweredOutput {
    let keys = lowering(harness).skill;
    let mut yaml = Mapping::new();
    let mut lossy_fields = Vec::new();
    let mut siblings = Vec::new();
    let target_name = harness.display_name();

    insert_identity(&mut yaml, profile);
    insert_license_metadata(&mut yaml, profile);
    match keys.model_off {
        ModelOff::Key => insert_disable_model_invocation(&mut yaml, profile),
        ModelOff::SiblingFile => {
            if model_invocation_explicitly_off(profile) {
                siblings.push(codex_openai_yaml_sibling());
            }
        }
        ModelOff::Lost => record_model_invocation_loss(&mut lossy_fields, profile, target_name),
        ModelOff::CursorManual => apply_cursor_rule_mode(&mut yaml, profile),
    }
    match keys.user_off {
        UserOff::Key(key, value) => {
            if user_invocation_disabled(profile) {
                yaml.insert(yk(key), Value::Bool(value));
            }
        }
        UserOff::Lost => record_user_invocation_loss(&mut lossy_fields, profile, target_name),
    }

    insert_overrides(
        &mut yaml,
        profile,
        harness,
        skill::LOWERED_KEYS,
        &mut lossy_fields,
    );

    LoweredOutput {
        bytes: render(yaml, body),
        lossy_fields,
        project_losses: Vec::new(),
        siblings,
    }
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
        let lowered =
            lower_skill_for_harness(HarnessId::Claude, &user_invocable_false_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("user-invocable: false"));
        assert!(lowered.lossy_fields.is_empty());
    }

    #[test]
    fn claude_both_false() {
        let lowered = lower_skill_for_harness(HarnessId::Claude, &both_false_profile(), "Body\n");
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
        let lowered = lower_skill_for_harness(HarnessId::Codex, &profile, "Body\n");
        assert!(!is_quiet(&lowered.lossy_fields, "model-invocable", "Codex"));
        assert_eq!(lowered.siblings.len(), 1);
        let sibling = String::from_utf8(lowered.siblings[0].bytes.clone()).unwrap();
        assert!(sibling.contains("policy:"));
        assert!(sibling.contains("allow_implicit_invocation: false"));
    }

    #[test]
    fn codex_drops_user_invocable_false() {
        let lowered =
            lower_skill_for_harness(HarnessId::Codex, &user_invocable_false_profile(), "Body\n");
        assert!(is_quiet(&lowered.lossy_fields, "user-invocable", "Codex"));
    }

    #[test]
    fn opencode_drops_user_invocable_false() {
        let lowered = lower_skill_for_harness(
            HarnessId::OpenCode,
            &user_invocable_false_profile(),
            "Body\n",
        );
        assert!(is_quiet(
            &lowered.lossy_fields,
            "user-invocable",
            "OpenCode"
        ));
    }

    #[test]
    fn pi_model_false_emits_disable_model_invocation() {
        let lowered = lower_skill_for_harness(HarnessId::Pi, &profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("disable-model-invocation: true"));
    }

    #[test]
    fn pi_drops_user_invocable_false() {
        let lowered =
            lower_skill_for_harness(HarnessId::Pi, &user_invocable_false_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("user-invocable"));
        assert!(is_quiet(&lowered.lossy_fields, "user-invocable", "Pi"));
    }

    #[test]
    fn cursor_drops_user_invocable_false() {
        let lowered =
            lower_skill_for_harness(HarnessId::Cursor, &user_invocable_false_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("user-invocable"));
        assert!(is_quiet(&lowered.lossy_fields, "user-invocable", "Cursor"));
    }

    #[test]
    fn agy_hides_the_slash_command_for_user_invocable_false() {
        let lowered =
            lower_skill_for_harness(HarnessId::Agy, &user_invocable_false_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("disable-slash-command: true"), "{out}");
        assert!(!out.contains("user-invocable"), "{out}");
        assert!(lowered.lossy_fields.is_empty());
    }

    #[test]
    fn model_invocable_false_is_a_lost_restriction_where_no_switch_exists() {
        for (harness, target) in [
            (HarnessId::Agy, "Antigravity"),
            (HarnessId::OpenCode, "OpenCode"),
        ] {
            let lowered = lower_skill_for_harness(harness, &profile(), "Body\n");
            let out = String::from_utf8(lowered.bytes).unwrap();
            assert!(
                out.contains("description: desc"),
                "description stays: {out}"
            );
            assert!(
                lowered
                    .lossy_fields
                    .iter()
                    .any(|f| f.field == "model-invocable"
                        && f.target == target
                        && matches!(f.classification, Lossiness::Dropped(_)))
            );
        }
    }
}
