//! Universal skill frontmatter parser and native lowering support.

pub mod lower;

use serde_yaml::Value;

use crate::compiler::invocability::{find_invocability_field, parse_invocability_axis, value_label};
use crate::compiler::tool_names::{ParsedToolName, parse_mars_tool_name};
use crate::frontmatter::{Frontmatter, FrontmatterError};

#[derive(Debug, Clone)]
pub struct SkillProfile {
    pub name: Option<String>,
    pub description: Option<String>,
    pub when_to_use: Option<String>,
    pub skill_type: Option<String>,
    pub model_invocable: bool,
    pub user_invocable: bool,
    pub allowed_tools: Vec<String>,
    /// Canonical tool denylist — lowered to harness-native denylist fields where supported.
    pub disallowed_tools: Vec<String>,
    pub license: Option<String>,
    pub metadata: Option<Value>,
    /// true when the source frontmatter explicitly set `model-invocable`
    pub had_model_invocable_field: bool,
    /// true when the source frontmatter explicitly set `user-invocable`
    pub had_user_invocable_field: bool,
    pub has_frontmatter: bool,
    /// Frontmatter fields not recognized by mars — passed through to all targets.
    pub passthrough_fields: Vec<(String, Value)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillDiagnostic {
    InvalidFieldValue {
        field: String,
        value: String,
        allowed: &'static str,
    },
    InvalidFieldType {
        field: String,
        value: String,
        allowed: &'static str,
    },
    RemovedField {
        field: String,
    },
    MalformedFrontmatter {
        message: String,
    },
}

impl SkillDiagnostic {
    pub fn is_error(&self) -> bool {
        matches!(
            self,
            Self::InvalidFieldValue { .. }
                | Self::RemovedField { .. }
                | Self::MalformedFrontmatter { .. }
        )
    }

    pub fn message(&self) -> String {
        match self {
            Self::InvalidFieldValue {
                field,
                value,
                allowed,
            } => format!("skill field `{field}` has invalid value `{value}`; allowed: {allowed}"),
            Self::InvalidFieldType {
                field,
                value,
                allowed,
            } => format!(
                "skill field `{field}` has unsupported value `{value}`; expected: {allowed}"
            ),
            Self::RemovedField { field } => format!(
                "skill field `{field}` has been removed; use `model-invocable` / `user-invocable` instead"
            ),
            Self::MalformedFrontmatter { message } => {
                format!("skill frontmatter is malformed; raw fallback used: {message}")
            }
        }
    }
}

fn yaml_str_list(field: &str, val: &Value, diags: &mut Vec<SkillDiagnostic>) -> Vec<String> {
    match val {
        Value::Sequence(seq) => seq
            .iter()
            .enumerate()
            .filter_map(|(idx, item)| match item.as_str() {
                Some(s) => Some(s.to_owned()),
                None => {
                    diags.push(SkillDiagnostic::InvalidFieldType {
                        field: format!("{field}[{idx}]"),
                        value: value_label(item),
                        allowed: "string",
                    });
                    None
                }
            })
            .collect(),
        Value::String(s) => vec![s.clone()],
        _ => {
            diags.push(SkillDiagnostic::InvalidFieldType {
                field: field.to_string(),
                value: value_label(val),
                allowed: "string or list of strings",
            });
            vec![]
        }
    }
}

fn yaml_tool_list(field: &str, val: &Value, diags: &mut Vec<SkillDiagnostic>) -> Vec<String> {
    yaml_str_list(field, val, diags)
        .into_iter()
        .enumerate()
        .filter_map(|(idx, tool)| match parse_mars_tool_name(&tool) {
            Ok(ParsedToolName { name, .. }) => Some(name),
            Err(err) => {
                diags.push(SkillDiagnostic::InvalidFieldValue {
                    field: format!("{field}[{idx}]"),
                    value: tool,
                    allowed: err.allowed(),
                });
                None
            }
        })
        .collect()
}

fn validate_required_string(field: &str, val: Option<&Value>, diags: &mut Vec<SkillDiagnostic>) {
    match val {
        Some(raw) if raw.is_string() => {}
        Some(raw) => diags.push(SkillDiagnostic::InvalidFieldValue {
            field: field.to_string(),
            value: value_label(raw),
            allowed: "string",
        }),
        None => diags.push(SkillDiagnostic::InvalidFieldValue {
            field: field.to_string(),
            value: "missing".to_string(),
            allowed: "string",
        }),
    }
}

fn parse_invocability_bool(
    field: &str,
    raw: Option<&Value>,
    diags: &mut Vec<SkillDiagnostic>,
) -> (bool, bool) {
    let (value, had_field, invalid) = parse_invocability_axis(raw);
    if let Some(invalid) = invalid {
        diags.push(SkillDiagnostic::InvalidFieldType {
            field: field.to_string(),
            value: invalid,
            allowed: "boolean",
        });
    }
    (value, had_field)
}

pub fn parse_skill_profile(fm: &Frontmatter, diags: &mut Vec<SkillDiagnostic>) -> SkillProfile {
    // Track which keys we consume so passthrough = all keys minus consumed.
    // This avoids a static list that drifts when new fields are added.
    let mut consumed_keys: Vec<String> = Vec::new();

    consumed_keys.push("name".to_string());
    let name_raw = fm.get("name");
    let name = name_raw.and_then(Value::as_str).map(str::to_owned);
    consumed_keys.push("description".to_string());
    let description_raw = fm.get("description");
    let description = description_raw.and_then(Value::as_str).map(str::to_owned);
    consumed_keys.push("when_to_use".to_string());
    let when_to_use_raw = fm.get("when_to_use");
    let when_to_use = when_to_use_raw.and_then(Value::as_str).map(str::to_owned);
    if let Some(raw) = when_to_use_raw
        && !raw.is_string()
    {
        diags.push(SkillDiagnostic::InvalidFieldType {
            field: "when_to_use".to_string(),
            value: value_label(raw),
            allowed: "string",
        });
    }
    if fm.has_frontmatter() {
        validate_required_string("name", name_raw, diags);
        validate_required_string("description", description_raw, diags);
    }
    consumed_keys.push("allowed-tools".to_string());
    consumed_keys.push("allowed_tools".to_string());
    let allowed_tools = fm
        .get("allowed-tools")
        .or_else(|| fm.get("allowed_tools"))
        .map(|v| yaml_tool_list("allowed-tools", v, diags))
        .unwrap_or_default();
    consumed_keys.push("disallowed-tools".to_string());
    consumed_keys.push("disallowed_tools".to_string());
    let disallowed_tools = fm
        .get("disallowed-tools")
        .or_else(|| fm.get("disallowed_tools"))
        .map(|v| yaml_tool_list("disallowed-tools", v, diags))
        .unwrap_or_default();
    consumed_keys.push("license".to_string());
    let license_raw = fm.get("license");
    let license = license_raw.and_then(Value::as_str).map(str::to_owned);
    if let Some(raw) = license_raw
        && !raw.is_string()
    {
        diags.push(SkillDiagnostic::InvalidFieldType {
            field: "license".to_string(),
            value: value_label(raw),
            allowed: "string",
        });
    }
    consumed_keys.push("type".to_string());
    let skill_type_raw = fm.get("type");
    let skill_type = skill_type_raw.and_then(Value::as_str).map(str::to_owned);
    if let Some(raw) = skill_type_raw
        && !raw.is_string()
    {
        diags.push(SkillDiagnostic::InvalidFieldType {
            field: "type".to_string(),
            value: value_label(raw),
            allowed: "string",
        });
    }
    consumed_keys.push("metadata".to_string());
    let metadata = fm.get("metadata").cloned();

    let model_invocability = find_invocability_field(fm, "model-invocable");
    let (model_invocable, had_model_invocable_field) = parse_invocability_bool(
        "model-invocable",
        model_invocability.as_ref().map(|f| &f.value),
        diags,
    );
    if let Some(field) = model_invocability {
        consumed_keys.push(field.consumed_key);
    }
    let user_invocability = find_invocability_field(fm, "user-invocable");
    let (user_invocable, had_user_invocable_field) = parse_invocability_bool(
        "user-invocable",
        user_invocability.as_ref().map(|f| &f.value),
        diags,
    );
    if let Some(field) = user_invocability {
        consumed_keys.push(field.consumed_key);
    }

    for field in [
        "invocation",
        "disable-model-invocation",
        "allow_implicit_invocation",
    ] {
        consumed_keys.push(field.to_string());
        if fm.get(field).is_some() {
            diags.push(SkillDiagnostic::RemovedField {
                field: field.to_string(),
            });
        }
    }

    // Passthrough = all frontmatter keys we didn't consume above.
    let passthrough_fields = fm
        .keys()
        .into_iter()
        .filter(|k| !consumed_keys.iter().any(|c| c == k))
        .filter_map(|k| fm.get(&k).map(|v| (k.clone(), v.clone())))
        .collect::<Vec<_>>();

    SkillProfile {
        name,
        description,
        when_to_use,
        skill_type,
        model_invocable,
        user_invocable,
        allowed_tools,
        disallowed_tools,
        license,
        metadata,
        had_model_invocable_field,
        had_user_invocable_field,
        has_frontmatter: fm.has_frontmatter(),
        passthrough_fields,
    }
}

pub fn parse_skill_content(
    content: &str,
    diags: &mut Vec<SkillDiagnostic>,
) -> Result<(SkillProfile, Frontmatter), FrontmatterError> {
    let fm = Frontmatter::parse(content).inspect_err(|e| {
        diags.push(SkillDiagnostic::MalformedFrontmatter {
            message: e.to_string(),
        });
    })?;
    let profile = parse_skill_profile(&fm, diags);
    Ok((profile, fm))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(content: &str) -> (SkillProfile, Vec<SkillDiagnostic>, Frontmatter) {
        let mut diags = Vec::new();
        let (profile, fm) = parse_skill_content(content, &mut diags).unwrap();
        (profile, diags, fm)
    }

    fn removed_field_named(diags: &[SkillDiagnostic], expected: &str) -> bool {
        diags.iter().any(|d| {
            matches!(
                d,
                SkillDiagnostic::RemovedField { field } if field == expected
            )
        })
    }

    #[test]
    fn snake_case_model_invocable_not_in_passthrough() {
        let (p, d, _) = parse("---\nname: a\ndescription: b\nmodel_invocable: false\n---\nbody");
        assert!(d.is_empty());
        assert!(!p.model_invocable);
        assert!(p.had_model_invocable_field);
        assert!(
            !p.passthrough_fields
                .iter()
                .any(|(k, _)| k == "model_invocable")
        );
    }

    #[test]
    fn no_frontmatter_defaults_invocable_and_preserves_body() {
        let (p, d, fm) = parse("# Body\nbytes");
        assert!(d.is_empty());
        assert!(p.model_invocable);
        assert!(p.user_invocable);
        assert!(!p.has_frontmatter);
        assert_eq!(fm.body(), "# Body\nbytes");
    }

    #[test]
    fn parses_identity_only() {
        let (p, d, _) = parse("---\nname: a\ndescription: b\n---\nbody");
        assert!(d.is_empty());
        assert!(p.has_frontmatter);
        assert_eq!(p.name.as_deref(), Some("a"));
        assert_eq!(p.description.as_deref(), Some("b"));
    }

    #[test]
    fn model_invocable_false_parses() {
        let (p, d, _) = parse("---\nname: a\ndescription: b\nmodel-invocable: false\n---\nbody");
        assert!(d.is_empty());
        assert!(!p.model_invocable);
        assert!(p.had_model_invocable_field);
        assert!(p.user_invocable);
        assert!(!p.had_user_invocable_field);
    }

    #[test]
    fn user_invocable_false_parses() {
        let (p, d, _) = parse(
            "---
name: a
description: b
user-invocable: false
---
body",
        );
        assert!(d.is_empty());
        assert!(p.model_invocable);
        assert!(!p.had_model_invocable_field);
        assert!(!p.user_invocable);
        assert!(p.had_user_invocable_field);
    }

    #[test]
    fn both_booleans_false_accepted() {
        let (p, d, _) = parse(
            "---\nname: a\ndescription: b\nmodel-invocable: false\nuser-invocable: false\n---\nbody",
        );
        assert!(d.is_empty());
        assert!(!p.model_invocable);
        assert!(!p.user_invocable);
        assert!(p.had_model_invocable_field);
        assert!(p.had_user_invocable_field);
    }

    #[test]
    fn explicit_true_invocability_sets_presence_flags() {
        let (p, d, _) = parse(
            "---\nname: a\ndescription: b\nmodel-invocable: true\nuser-invocable: true\n---\nbody",
        );
        assert!(d.is_empty());
        assert!(p.model_invocable);
        assert!(p.user_invocable);
        assert!(p.had_model_invocable_field);
        assert!(p.had_user_invocable_field);
    }

    #[test]
    fn non_boolean_model_invocable_defaults_true() {
        let (p, d, _) = parse("---\nname: a\ndescription: b\nmodel-invocable: \"yes\"\n---\nbody");
        assert!(p.model_invocable);
        assert!(!p.had_model_invocable_field);
        assert!(d.iter().any(|d| matches!(
            d,
            SkillDiagnostic::InvalidFieldType { field, allowed, .. }
                if field == "model-invocable" && *allowed == "boolean"
        )));
    }

    #[test]
    fn non_boolean_user_invocable_defaults_true() {
        let (p, d, _) = parse(
            "---
name: a
description: b
user-invocable: 7
---
body",
        );
        assert!(p.user_invocable);
        assert!(!p.had_user_invocable_field);
        assert!(d.iter().any(|d| matches!(
            d,
            SkillDiagnostic::InvalidFieldType { field, allowed, .. }
                if field == "user-invocable" && *allowed == "boolean"
        )));
    }

    #[test]
    fn removed_field_invocation() {
        let (p, d, _) = parse("---\nname: a\ndescription: b\ninvocation: explicit\n---\nbody");
        assert!(p.model_invocable);
        assert!(p.user_invocable);
        assert!(removed_field_named(&d, "invocation"));
        assert!(d.iter().any(SkillDiagnostic::is_error));
    }

    #[test]
    fn removed_field_disable_model_invocation() {
        let (p, d, _) = parse(
            "---
name: a
description: b
disable-model-invocation: true
---
body",
        );
        assert!(p.model_invocable);
        assert!(!p.had_model_invocable_field);
        assert!(p.user_invocable);
        assert!(removed_field_named(&d, "disable-model-invocation"));
        assert!(d.iter().any(SkillDiagnostic::is_error));
    }

    #[test]
    fn removed_field_allow_implicit_invocation() {
        let (p, d, _) = parse(
            "---
name: a
description: b
allow_implicit_invocation: false
---
body",
        );
        assert!(p.model_invocable);
        assert!(!p.had_model_invocable_field);
        assert!(p.user_invocable);
        assert!(removed_field_named(&d, "allow_implicit_invocation"));
        assert!(d.iter().any(SkillDiagnostic::is_error));
    }

    #[test]
    fn all_removed_fields_emit_removed_field() {
        let (_, d, _) = parse(
            "---\nname: a\ndescription: b\ninvocation: explicit\ndisable-model-invocation: true\nallow_implicit_invocation: false\n---\nbody",
        );
        assert!(removed_field_named(&d, "invocation"));
        assert!(removed_field_named(&d, "disable-model-invocation"));
        assert!(removed_field_named(&d, "allow_implicit_invocation"));
    }

    #[test]
    fn frontmatter_requires_name_and_description() {
        let (_, d, _) = parse("---\nname: a\n---\nbody");
        assert!(d.iter().any(|d| matches!(
            d,
            SkillDiagnostic::InvalidFieldValue { field, value, .. }
                if field == "description" && value == "missing"
        )));
    }

    #[test]
    fn warns_for_filtered_non_string_fields() {
        let (_, d, _) = parse(
            "---\nname: a\ndescription: b\nallowed-tools: [Bash(git *), 7]\nlicense: false\n---\nbody",
        );
        assert!(d.iter().any(|d| matches!(
            d,
            SkillDiagnostic::InvalidFieldType { field, .. } if field == "allowed-tools[1]"
        )));
        assert!(d.iter().any(|d| matches!(
            d,
            SkillDiagnostic::InvalidFieldType { field, .. } if field == "license"
        )));
    }

    #[test]
    fn separator_tool_aliases_canonicalize() {
        let (p, d, _) = parse(
            "---\nname: a\ndescription: b\nallowed-tools: [ask_user, bash(git *)]\n---\nbody",
        );

        assert_eq!(p.allowed_tools, vec!["ask_user", "bash(git *)"]);
        assert!(d.is_empty());
    }

    #[test]
    fn unknown_pascal_case_tool_names_convert_to_snake_case() {
        let (p, d, _) = parse(
            "---
name: a
description: b
allowed-tools: [customtool, CustomTool]
---
body",
        );

        assert_eq!(p.allowed_tools, vec!["customtool", "custom_tool"]);
        assert!(d.is_empty());
    }

    #[test]
    fn type_parses_from_frontmatter() {
        let (p, d, _) = parse("---\nname: a\ndescription: b\ntype: guardrail\n---\nbody");
        assert!(d.is_empty());
        assert_eq!(p.skill_type.as_deref(), Some("guardrail"));
    }

    #[test]
    fn type_absent_gives_none() {
        let (p, d, _) = parse("---\nname: a\ndescription: b\n---\nbody");
        assert!(d.is_empty());
        assert!(p.skill_type.is_none());
    }

    #[test]
    fn non_string_type_emits_diagnostic() {
        let (p, d, _) = parse("---\nname: a\ndescription: b\ntype: [a, b]\n---\nbody");
        assert!(p.skill_type.is_none());
        assert!(d.iter().any(|diag| matches!(
            diag,
            SkillDiagnostic::InvalidFieldType { field, allowed, .. }
                if field == "type" && *allowed == "string"
        )));
    }

    #[test]
    fn disallowed_tools_defaults_empty() {
        let (p, d, _) = parse("---\nname: a\ndescription: b\n---\nbody");
        assert!(d.is_empty());
        assert!(p.disallowed_tools.is_empty());
    }

    #[test]
    fn disallowed_tools_parses_and_canonicalizes() {
        let (p, d, _) = parse(
            "---\nname: a\ndescription: b\ndisallowed-tools: [Agent, web_search]\n---\nbody",
        );
        assert!(d.is_empty());
        assert_eq!(p.disallowed_tools, vec!["agent", "web_search"]);
    }

    #[test]
    fn disallowed_tools_snake_key_parses() {
        let (p, d, _) = parse(
            "---\nname: a\ndescription: b\ndisallowed_tools: [Write]\n---\nbody",
        );
        assert!(d.is_empty());
        assert_eq!(p.disallowed_tools, vec!["write"]);
    }

    #[test]
    fn malformed_yaml_raw_fallback_diagnostic() {
        let mut diags = Vec::new();
        let err = parse_skill_content("---\ninvalid: [:\n---\nbody", &mut diags).unwrap_err();
        assert!(matches!(err, FrontmatterError::MalformedYaml(_)));
        assert!(matches!(
            diags[0],
            SkillDiagnostic::MalformedFrontmatter { .. }
        ));
    }
}
