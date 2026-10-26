//! Declarative per-harness skill lowering policies and shared pipeline.

use serde_yaml::{Mapping, Value};

use crate::compiler::harness_descriptor::{self};
use crate::compiler::lossiness::{Lossiness, LossyField, LoweredOutput, LoweredSibling};
use mars_schema::HarnessId;
use mars_schema::skill::{self, SkillProfile};

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
                harness_descriptor::descriptor(harness).display_name,
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

fn insert_user_invocation(yaml: &mut Mapping, profile: &SkillProfile) {
    if user_invocation_disabled(profile) {
        yaml.insert(yk("user-invocable"), Value::Bool(false));
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

fn apply_cursor_rule_mode(yaml: &mut Mapping, profile: &SkillProfile) {
    if !profile.has_frontmatter {
        return;
    }
    // Cursor's Manual and Intelligent modes both require alwaysApply:false.
    yaml.insert(yk("alwaysApply"), Value::Bool(false));
    if cursor_manual_mode(profile) {
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

pub(super) fn lower_skill_with_policy(
    harness: HarnessId,
    profile: &SkillProfile,
    body: &str,
) -> LoweredOutput {
    let mut yaml = Mapping::new();
    let mut lossy_fields = Vec::new();
    let mut siblings = Vec::new();
    let target_name = harness_descriptor::descriptor(harness).display_name;

    insert_identity(&mut yaml, profile);
    match harness {
        HarnessId::Claude => {
            insert_disable_model_invocation(&mut yaml, profile);
            insert_user_invocation(&mut yaml, profile);
            insert_license_metadata(&mut yaml, profile);
        }
        HarnessId::Codex => {
            insert_license_metadata(&mut yaml, profile);
            if profile.had_model_invocable_field && !profile.model_invocable {
                siblings.push(codex_openai_yaml_sibling());
            }
            record_user_invocation_loss(&mut lossy_fields, profile, target_name);
        }
        HarnessId::OpenCode => {
            insert_license_metadata(&mut yaml, profile);
            record_user_invocation_loss(&mut lossy_fields, profile, target_name);
        }
        HarnessId::Pi => {
            insert_disable_model_invocation(&mut yaml, profile);
            insert_license_metadata(&mut yaml, profile);
            record_user_invocation_loss(&mut lossy_fields, profile, target_name);
        }
        HarnessId::Cursor => {
            insert_license_metadata(&mut yaml, profile);
            apply_cursor_rule_mode(&mut yaml, profile);
            record_user_invocation_loss(&mut lossy_fields, profile, target_name);
        }
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
        siblings,
    }
}
