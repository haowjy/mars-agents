//! Per-harness lowering for universal skill frontmatter.

use serde_yaml::{Mapping, Value};

use crate::compiler::lossiness::{Lossiness, LossyField, LoweredOutput};
use crate::compiler::skills::SkillProfile;
use crate::compiler::tool_names::{ToolProjectionStatus, project_tool_for_harness};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillHarness {
    Claude,
    Codex,
    OpenCode,
    Pi,
    Cursor,
}

impl SkillHarness {
    pub fn from_variant_key(key: &str) -> Option<Self> {
        match key {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "opencode" => Some(Self::OpenCode),
            "pi" => Some(Self::Pi),
            "cursor" => Some(Self::Cursor),
            _ => None,
        }
    }
    fn target_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
            Self::Pi => "Pi",
            Self::Cursor => "Cursor",
        }
    }
}

fn yk(s: &str) -> Value {
    Value::String(s.to_string())
}
fn ys(s: &str) -> Value {
    Value::String(s.to_string())
}
fn insert_identity(yaml: &mut Mapping, profile: &SkillProfile) {
    if let Some(name) = &profile.name {
        yaml.insert(yk("name"), ys(name));
    }
    if let Some(description) = &profile.description {
        yaml.insert(yk("description"), ys(description));
    }
}
fn insert_allowed_tools(
    yaml: &mut Mapping,
    profile: &SkillProfile,
    harness: &str,
    lossy_fields: Option<&mut Vec<LossyField>>,
) {
    let allowed = profile.effective_tool_policy().allowed;
    if !allowed.is_empty() {
        let mut lossy_fields = lossy_fields;
        let mut tools = Vec::new();
        for tool in &allowed {
            let projected = project_tool_for_harness(tool, harness);
            if projected.status == ToolProjectionStatus::Unknown
                && let Some(lossy_fields) = lossy_fields.as_deref_mut()
            {
                lossy_fields.push(LossyField {
                    field: "tools".into(),
                    target: harness.into(),
                    classification: Lossiness::Approximate {
                        note: "unknown tool name passed through verbatim",
                    },
                });
            }
            tools.push(projected.name);
        }
        yaml.insert(
            yk("allowed-tools"),
            Value::Sequence(tools.iter().map(|s| ys(s)).collect()),
        );
    }
}
fn insert_disallowed_tools(
    yaml: &mut Mapping,
    profile: &SkillProfile,
    harness: SkillHarness,
    harness_str: &str,
    lossy_fields: &mut Vec<LossyField>,
) {
    let disallowed = profile.effective_tool_policy().disallowed;
    if disallowed.is_empty() {
        return;
    }
    match harness {
        SkillHarness::Claude | SkillHarness::Pi => {
            let mut tools = Vec::new();
            for tool in &disallowed {
                let projected = project_tool_for_harness(tool, harness_str);
                if projected.status == ToolProjectionStatus::Unknown {
                    lossy_fields.push(LossyField {
                        field: "disallowed-tools".into(),
                        target: harness.target_name().into(),
                        classification: Lossiness::Approximate {
                            note: "unknown tool name passed through verbatim",
                        },
                    });
                }
                tools.push(projected.name);
            }
            yaml.insert(
                yk("disallowed-tools"),
                Value::Sequence(tools.iter().map(|s| ys(s)).collect()),
            );
        }
        SkillHarness::Codex | SkillHarness::OpenCode | SkillHarness::Cursor => {
            lossy_fields.push(dropped("disallowed-tools", harness));
        }
    }
}
fn insert_when_to_use(yaml: &mut Mapping, profile: &SkillProfile) {
    if let Some(when_to_use) = &profile.when_to_use {
        yaml.insert(yk("when_to_use"), ys(when_to_use));
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

fn insert_passthrough(yaml: &mut Mapping, profile: &SkillProfile) {
    for (key, value) in &profile.passthrough_fields {
        yaml.insert(yk(key), value.clone());
    }
}

fn user_invocation_disabled(profile: &SkillProfile) -> bool {
    let _was_explicitly_set = profile.had_user_invocable_field;
    !profile.user_invocable
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

pub fn lower_skill_for_harness(
    harness: SkillHarness,
    profile: &SkillProfile,
    body: &str,
) -> LoweredOutput {
    match harness {
        SkillHarness::Claude => lower_skill_to_claude(profile, body),
        SkillHarness::Codex => lower_skill_to_codex(profile, body),
        SkillHarness::OpenCode => lower_skill_to_opencode(profile, body),
        SkillHarness::Pi => lower_skill_to_pi(profile, body),
        SkillHarness::Cursor => lower_skill_to_cursor(profile, body),
    }
}

pub fn lower_skill_to_claude(profile: &SkillProfile, body: &str) -> LoweredOutput {
    let mut yaml = Mapping::new();
    insert_identity(&mut yaml, profile);
    if !profile.model_invocable {
        yaml.insert(yk("disable-model-invocation"), Value::Bool(true));
    }
    if user_invocation_disabled(profile) {
        yaml.insert(yk("user-invocable"), Value::Bool(false));
    }
    let mut lossy_fields = Vec::new();
    insert_allowed_tools(&mut yaml, profile, "claude", Some(&mut lossy_fields));
    insert_disallowed_tools(
        &mut yaml,
        profile,
        SkillHarness::Claude,
        "claude",
        &mut lossy_fields,
    );
    insert_when_to_use(&mut yaml, profile);
    insert_license_metadata(&mut yaml, profile);
    insert_passthrough(&mut yaml, profile);
    LoweredOutput {
        bytes: render(yaml, body),
        lossy_fields,
    }
}

pub fn lower_skill_to_codex(profile: &SkillProfile, body: &str) -> LoweredOutput {
    let mut yaml = Mapping::new();
    insert_identity(&mut yaml, profile);
    insert_license_metadata(&mut yaml, profile);
    insert_passthrough(&mut yaml, profile);
    let mut lossy_fields = Vec::new();
    if profile.had_model_invocable_field {
        // TODO(#116): emit Codex sibling `policy` file for faithful
        // invocation/tool gating — see https://github.com/haowjy/mars-agents/issues/116
        lossy_fields.push(dropped("model-invocable", SkillHarness::Codex));
    }
    let tool_policy = profile.effective_tool_policy();
    if !tool_policy.allowed.is_empty() {
        lossy_fields.push(dropped("tools", SkillHarness::Codex));
    }
    insert_disallowed_tools(
        &mut yaml,
        profile,
        SkillHarness::Codex,
        "codex",
        &mut lossy_fields,
    );
    if user_invocation_disabled(profile) {
        lossy_fields.push(dropped("user-invocable", SkillHarness::Codex));
    }
    if profile.when_to_use.is_some() {
        lossy_fields.push(dropped("when_to_use", SkillHarness::Codex));
    }
    LoweredOutput {
        bytes: render(yaml, body),
        lossy_fields,
    }
}

pub fn lower_skill_to_opencode(profile: &SkillProfile, body: &str) -> LoweredOutput {
    let mut yaml = Mapping::new();
    insert_identity(&mut yaml, profile);
    insert_license_metadata(&mut yaml, profile);
    insert_passthrough(&mut yaml, profile);
    let mut lossy_fields = Vec::new();
    if !profile.model_invocable {
        lossy_fields.push(dropped("model-invocable", SkillHarness::OpenCode));
    }
    if user_invocation_disabled(profile) {
        lossy_fields.push(dropped("user-invocable", SkillHarness::OpenCode));
    }
    let tool_policy = profile.effective_tool_policy();
    if !tool_policy.allowed.is_empty() {
        lossy_fields.push(dropped("tools", SkillHarness::OpenCode));
    }
    insert_disallowed_tools(
        &mut yaml,
        profile,
        SkillHarness::OpenCode,
        "opencode",
        &mut lossy_fields,
    );
    if profile.when_to_use.is_some() {
        lossy_fields.push(dropped("when_to_use", SkillHarness::OpenCode));
    }
    LoweredOutput {
        bytes: render(yaml, body),
        lossy_fields,
    }
}

pub fn lower_skill_to_pi(profile: &SkillProfile, body: &str) -> LoweredOutput {
    let mut yaml = Mapping::new();
    insert_identity(&mut yaml, profile);
    if !profile.model_invocable {
        yaml.insert(yk("disable-model-invocation"), Value::Bool(true));
    }
    insert_allowed_tools(&mut yaml, profile, "pi", None);
    let mut lossy_fields = Vec::new();
    insert_disallowed_tools(&mut yaml, profile, SkillHarness::Pi, "pi", &mut lossy_fields);
    insert_when_to_use(&mut yaml, profile);
    insert_license_metadata(&mut yaml, profile);
    insert_passthrough(&mut yaml, profile);
    if user_invocation_disabled(profile) {
        lossy_fields.push(dropped("user-invocable", SkillHarness::Pi));
    }
    LoweredOutput {
        bytes: render(yaml, body),
        lossy_fields,
    }
}

pub fn lower_skill_to_cursor(profile: &SkillProfile, body: &str) -> LoweredOutput {
    let mut yaml = Mapping::new();
    insert_identity(&mut yaml, profile);
    insert_license_metadata(&mut yaml, profile);
    insert_passthrough(&mut yaml, profile);
    let mut lossy_fields = Vec::new();
    if profile.had_model_invocable_field {
        if profile.model_invocable {
            yaml.insert(yk("alwaysApply"), Value::Bool(true));
        } else {
            lossy_fields.push(dropped("model-invocable", SkillHarness::Cursor));
        }
    }
    let tool_policy = profile.effective_tool_policy();
    if !tool_policy.allowed.is_empty() {
        lossy_fields.push(dropped("tools", SkillHarness::Cursor));
    }
    insert_disallowed_tools(
        &mut yaml,
        profile,
        SkillHarness::Cursor,
        "cursor",
        &mut lossy_fields,
    );
    if user_invocation_disabled(profile) {
        lossy_fields.push(dropped("user-invocable", SkillHarness::Cursor));
    }
    if profile.when_to_use.is_some() {
        lossy_fields.push(dropped("when_to_use", SkillHarness::Cursor));
    }
    LoweredOutput {
        bytes: render(yaml, body),
        lossy_fields,
    }
}

fn dropped(field: &str, harness: SkillHarness) -> LossyField {
    LossyField {
        field: field.to_string(),
        target: harness.target_name().to_string(),
        classification: Lossiness::Dropped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::skills::parse_skill_content;

    fn parse_profile(content: &str) -> SkillProfile {
        let mut diags = Vec::new();
        parse_skill_content(content, &mut diags).unwrap().0
    }

    fn profile() -> SkillProfile {
        parse_profile(
            "---\nname: skill\ndescription: desc\nmodel-invocable: false\ntools: [Bash(git *)]\nlicense: MIT\nmetadata:\n  owner: team\nextra: stripped\n---\nBody\n",
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

    fn disallowed_tools_profile() -> SkillProfile {
        parse_profile(
            "---\nname: skill\ndescription: desc\ndisallowed-tools: [Agent, Bash(git *)]\n---\nBody\n",
        )
    }

    #[test]
    fn claude_emits_disallowed_tools_projected() {
        let lowered = lower_skill_to_claude(&disallowed_tools_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("disallowed-tools:"));
        assert!(out.contains("- Agent"));
        assert!(
            out.contains("- Bash(git *)"),
            "scoped payload preserved: {out}"
        );
        assert!(lowered.lossy_fields.is_empty());
    }

    #[test]
    fn pi_emits_disallowed_tools() {
        let lowered = lower_skill_to_pi(&disallowed_tools_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("disallowed-tools:"));
        assert!(out.contains("- agent"));
        assert!(!has_dropped(&lowered.lossy_fields, "disallowed-tools", "Pi"));
    }

    #[test]
    fn codex_warn_drops_disallowed_tools() {
        let lowered = lower_skill_to_codex(&disallowed_tools_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("disallowed-tools"));
        assert!(has_dropped(
            &lowered.lossy_fields,
            "disallowed-tools",
            "Codex"
        ));
    }

    #[test]
    fn opencode_warn_drops_disallowed_tools() {
        let lowered = lower_skill_to_opencode(&disallowed_tools_profile(), "Body\n");
        assert!(has_dropped(
            &lowered.lossy_fields,
            "disallowed-tools",
            "OpenCode"
        ));
    }

    #[test]
    fn cursor_warn_drops_disallowed_tools() {
        let lowered = lower_skill_to_cursor(&disallowed_tools_profile(), "Body\n");
        assert!(has_dropped(
            &lowered.lossy_fields,
            "disallowed-tools",
            "Cursor"
        ));
    }

    #[test]
    fn claude_maps_model_invocation_and_tools() {
        let lowered = lower_skill_to_claude(&profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("disable-model-invocation: true"));
        assert!(out.contains("allowed-tools:"));
        assert!(!out.contains("allow_implicit_invocation"));
        assert!(out.contains("license: MIT"));
        assert!(out.contains("owner: team"));
        // Unknown fields pass through to all targets
        assert!(out.contains("extra: stripped"));
        assert!(lowered.lossy_fields.is_empty());
    }

    #[test]
    fn claude_projects_canonical_allowed_tools() {
        let profile = parse_profile(
            "---\nname: skill\ndescription: desc\ntools: [AskUser, Bash(git *)]\n---\nBody\n",
        );
        let lowered = lower_skill_to_claude(&profile, "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();

        assert!(out.contains("- AskUser"), "AskUser not projected: {out}");
        assert!(
            out.contains("- Bash(git *)"),
            "scoped Bash not projected while preserving payload: {out}"
        );
    }

    #[test]
    fn claude_lowers_tools_map_allow_and_deny() {
        let profile = parse_profile(
            "---\nname: skill\ndescription: desc\ntools:\n  ask_user: allow\n  \"bash(git *)\": deny\n---\nBody\n",
        );
        let lowered = lower_skill_to_claude(&profile, "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("allowed-tools:"));
        assert!(out.contains("- AskUser"));
        assert!(!out.contains("bash(git *)"), "denied tools must not appear in allowlist: {out}");
        assert!(lowered.lossy_fields.is_empty());
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
    fn codex_warn_drops_model_invocable_and_tools() {
        let lowered = lower_skill_to_codex(&profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("allow_implicit_invocation"));
        assert!(!out.contains("disable-model-invocation"));
        assert!(!out.contains("allowed-tools"));
        assert!(has_dropped(
            &lowered.lossy_fields,
            "model-invocable",
            "Codex"
        ));
        assert!(has_dropped(&lowered.lossy_fields, "tools", "Codex"));
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
    fn codex_explicit_true_warn_drops_model_invocable() {
        let lowered = lower_skill_to_codex(&explicit_true_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("allow_implicit_invocation"));
        assert!(!out.contains("disable-model-invocation"));
        assert!(has_dropped(
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
    fn opencode_drops_model_invocable_and_tools() {
        let lowered = lower_skill_to_opencode(&profile(), "Body\n");
        assert!(has_dropped(
            &lowered.lossy_fields,
            "model-invocable",
            "OpenCode"
        ));
        assert!(has_dropped(
            &lowered.lossy_fields,
            "tools",
            "OpenCode"
        ));
        assert_eq!(lowered.lossy_fields.len(), 2);
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
    fn codex_warn_drops_when_to_use_without_emitting() {
        let profile = parse_profile(
            "---\nname: skill\ndescription: desc\nwhen_to_use: Use for git\n---\nBody\n",
        );
        let lowered = lower_skill_to_codex(&profile, "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("when_to_use"));
        assert!(has_dropped(&lowered.lossy_fields, "when_to_use", "Codex"));
    }

    #[test]
    fn cursor_drops_tools() {
        let lowered = lower_skill_to_cursor(&profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("disable-model-invocation"));
        assert!(!out.contains("allowed-tools"));
        assert!(has_dropped(
            &lowered.lossy_fields,
            "model-invocable",
            "Cursor"
        ));
        assert_eq!(lowered.lossy_fields.len(), 2);
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
    fn cursor_model_true_emits_always_apply_not_claude_keys() {
        let lowered = lower_skill_to_cursor(&explicit_true_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("alwaysApply: true"));
        assert!(!out.contains("disable-model-invocation"));
        assert!(!out.contains("user-invocable"));
        assert!(!has_dropped(
            &lowered.lossy_fields,
            "user-invocable",
            "Cursor"
        ));
    }

    #[test]
    fn snake_case_model_invocable_not_leaked_to_any_target() {
        let profile = parse_profile(
            "---\nname: skill\ndescription: desc\nmodel_invocable: false\n---\nBody\n",
        );
        for harness in [
            SkillHarness::Claude,
            SkillHarness::Codex,
            SkillHarness::OpenCode,
            SkillHarness::Pi,
            SkillHarness::Cursor,
        ] {
            let lowered = lower_skill_for_harness(harness, &profile, "Body\n");
            let out = String::from_utf8(lowered.bytes).unwrap();
            assert!(
                !out.contains("model_invocable"),
                "leaked snake key for {harness:?}: {out}"
            );
        }
        let claude = String::from_utf8(lower_skill_to_claude(&profile, "Body\n").bytes).unwrap();
        assert!(claude.contains("disable-model-invocation: true"));
    }

    #[test]
    fn no_frontmatter_body_only_all_harnesses() {
        let mut diags = Vec::new();
        let (profile, fm) = parse_skill_content("# Body\nbytes", &mut diags).unwrap();
        let body = fm.body();
        for harness in [
            SkillHarness::Claude,
            SkillHarness::Codex,
            SkillHarness::OpenCode,
            SkillHarness::Pi,
            SkillHarness::Cursor,
        ] {
            let out =
                String::from_utf8(lower_skill_for_harness(harness, &profile, body).bytes).unwrap();
            assert_eq!(out, "# Body\nbytes");
        }
    }

    #[test]
    fn claude_lowers_tools_map_deny_to_disallowed_tools() {
        let profile = parse_profile(
            "---\nname: skill\ndescription: desc\ntools:\n  Agent: deny\n---\nBody\n",
        );
        let lowered = lower_skill_to_claude(&profile, "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("disallowed-tools:"), "missing denylist: {out}");
        assert!(out.contains("- Agent"), "Agent not projected: {out}");
        assert!(lowered.lossy_fields.is_empty());
    }

    #[test]
    fn codex_warns_tools_map_deny_via_disallowed_tools() {
        let profile = parse_profile(
            "---\nname: skill\ndescription: desc\ntools:\n  Agent: deny\n---\nBody\n",
        );
        let lowered = lower_skill_to_codex(&profile, "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("disallowed-tools"));
        assert!(has_dropped(
            &lowered.lossy_fields,
            "disallowed-tools",
            "Codex"
        ));
    }
}
