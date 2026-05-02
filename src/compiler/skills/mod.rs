//! Universal skill frontmatter parser and native lowering support.

pub mod lower;

use serde_yaml::Value;

use crate::frontmatter::{Frontmatter, FrontmatterError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillInvocation {
    Explicit,
    Implicit,
}

impl SkillInvocation {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "explicit" => Some(Self::Explicit),
            "implicit" => Some(Self::Implicit),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SkillProfile {
    pub name: Option<String>,
    pub description: Option<String>,
    pub invocation: SkillInvocation,
    pub allowed_tools: Vec<String>,
    pub license: Option<String>,
    pub metadata: Option<Value>,
    pub legacy_fields_present: bool,
    pub had_invocation_field: bool,
    pub has_frontmatter: bool,
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
    DeprecatedLegacyField {
        field: String,
    },
    RedundantLegacyField {
        field: String,
    },
    ConflictingLegacyFields,
    MalformedFrontmatter {
        message: String,
    },
}

impl SkillDiagnostic {
    pub fn is_error(&self) -> bool {
        matches!(
            self,
            Self::InvalidFieldValue { .. }
                | Self::ConflictingLegacyFields
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
            Self::DeprecatedLegacyField { field } => {
                format!("skill uses deprecated `{field}` field; use `invocation` instead")
            }
            Self::RedundantLegacyField { field } => {
                format!("skill field `{field}` ignored because canonical `invocation` is present")
            }
            Self::ConflictingLegacyFields => {
                "skill legacy invocation fields conflict; use canonical `invocation`".to_string()
            }
            Self::MalformedFrontmatter { message } => {
                format!("skill frontmatter is malformed; raw fallback used: {message}")
            }
        }
    }
}

fn value_label(val: &Value) -> String {
    val.as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{val:?}"))
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

fn legacy_invocation(field: &str, val: &Value) -> Option<SkillInvocation> {
    let b = val.as_bool()?;
    match field {
        "disable-model-invocation" => Some(if b {
            SkillInvocation::Explicit
        } else {
            SkillInvocation::Implicit
        }),
        "allow_implicit_invocation" => Some(if b {
            SkillInvocation::Implicit
        } else {
            SkillInvocation::Explicit
        }),
        _ => None,
    }
}

pub fn parse_skill_profile(fm: &Frontmatter, diags: &mut Vec<SkillDiagnostic>) -> SkillProfile {
    let name_raw = fm.get("name");
    let name = name_raw.and_then(Value::as_str).map(str::to_owned);
    let description_raw = fm.get("description");
    let description = description_raw.and_then(Value::as_str).map(str::to_owned);
    if fm.has_frontmatter() {
        validate_required_string("name", name_raw, diags);
        validate_required_string("description", description_raw, diags);
    }
    let allowed_tools = fm
        .get("allowed-tools")
        .map(|v| yaml_str_list("allowed-tools", v, diags))
        .unwrap_or_default();
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
    let metadata = fm.get("metadata").cloned();

    let disable = fm.get("disable-model-invocation");
    let allow = fm.get("allow_implicit_invocation");
    for (field, raw) in [
        ("disable-model-invocation", disable),
        ("allow_implicit_invocation", allow),
    ] {
        if let Some(raw) = raw
            && !raw.is_bool()
        {
            diags.push(SkillDiagnostic::InvalidFieldValue {
                field: field.to_string(),
                value: value_label(raw),
                allowed: "boolean",
            });
        }
    }
    let legacy_fields_present = disable.is_some() || allow.is_some();
    let had_invocation_field = fm.get("invocation").is_some();

    let invocation = if let Some(raw) = fm.get("invocation") {
        for field in ["disable-model-invocation", "allow_implicit_invocation"] {
            if fm.get(field).is_some() {
                diags.push(SkillDiagnostic::RedundantLegacyField {
                    field: field.to_string(),
                });
            }
        }
        match raw.as_str().and_then(SkillInvocation::from_str) {
            Some(inv) => inv,
            None => {
                diags.push(SkillDiagnostic::InvalidFieldValue {
                    field: "invocation".to_string(),
                    value: value_label(raw),
                    allowed: "explicit, implicit",
                });
                SkillInvocation::Implicit
            }
        }
    } else {
        let disable_inv = disable.and_then(|v| legacy_invocation("disable-model-invocation", v));
        let allow_inv = allow.and_then(|v| legacy_invocation("allow_implicit_invocation", v));
        for field in ["disable-model-invocation", "allow_implicit_invocation"] {
            if fm.get(field).is_some() {
                diags.push(SkillDiagnostic::DeprecatedLegacyField {
                    field: field.to_string(),
                });
            }
        }
        match (disable_inv, allow_inv) {
            (Some(a), Some(b)) if a != b => {
                diags.push(SkillDiagnostic::ConflictingLegacyFields);
                SkillInvocation::Implicit
            }
            (Some(a), _) | (_, Some(a)) => a,
            (None, None) => SkillInvocation::Implicit,
        }
    };

    SkillProfile {
        name,
        description,
        invocation,
        allowed_tools,
        license,
        metadata,
        legacy_fields_present,
        had_invocation_field,
        has_frontmatter: fm.has_frontmatter(),
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
    #[test]
    fn no_frontmatter_defaults_implicit_and_preserves_body() {
        let (p, d, fm) = parse("# Body\nbytes");
        assert!(d.is_empty());
        assert_eq!(p.invocation, SkillInvocation::Implicit);
        assert_eq!(fm.body(), "# Body\nbytes");
    }
    #[test]
    fn parses_identity_only() {
        let (p, d, _) = parse("---\nname: a\ndescription: b\n---\nbody");
        assert!(d.is_empty());
        assert_eq!(p.name.as_deref(), Some("a"));
        assert_eq!(p.description.as_deref(), Some("b"));
    }
    #[test]
    fn canonical_invocation_wins_over_legacy() {
        let (p, d, _) = parse(
            "---\nname: a\ndescription: b\ninvocation: implicit\ndisable-model-invocation: true\n---\nbody",
        );
        assert_eq!(p.invocation, SkillInvocation::Implicit);
        assert!(matches!(d[0], SkillDiagnostic::RedundantLegacyField { .. }));
    }
    #[test]
    fn legacy_aliases_map_invocation() {
        let (p, d, _) =
            parse("---\nname: a\ndescription: b\nallow_implicit_invocation: false\n---\nbody");
        assert_eq!(p.invocation, SkillInvocation::Explicit);
        assert!(matches!(
            d[0],
            SkillDiagnostic::DeprecatedLegacyField { .. }
        ));
    }
    #[test]
    fn conflicting_legacy_fields_error() {
        let (p, d, _) = parse(
            "---\nname: a\ndescription: b\ndisable-model-invocation: true\nallow_implicit_invocation: true\n---\nbody",
        );
        assert_eq!(p.invocation, SkillInvocation::Implicit);
        assert!(
            d.iter()
                .any(|d| matches!(d, SkillDiagnostic::ConflictingLegacyFields))
        );
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
    fn legacy_aliases_require_boolean_values() {
        let (_, d, _) =
            parse("---\nname: a\ndescription: b\nallow_implicit_invocation: nope\n---\nbody");
        assert!(d.iter().any(|d| matches!(
            d,
            SkillDiagnostic::InvalidFieldValue { field, allowed, .. }
                if field == "allow_implicit_invocation" && *allowed == "boolean"
        )));
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
