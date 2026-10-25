//! Universal skill frontmatter parser and native lowering support.

pub mod lower;

use std::path::Path;

use serde_yaml::Value;

use crate::diagnostic::{DiagnosticCategory, DiagnosticCollector};
use mars_schema::frontmatter::{Frontmatter, FrontmatterError};
use mars_schema::invocability::{parse_invocability_axis, value_label};

/// Reserved lift-internal key; must not appear in user-authored MarsNative packages.
const RESERVED_IMPORT_MARKER: &str = "mars.imported-without-description";

#[derive(Debug, Clone)]
pub struct SkillProfile {
    pub name: Option<String>,
    pub description: Option<String>,
    pub skill_type: Option<String>,
    pub model_invocable: bool,
    pub user_invocable: bool,
    pub license: Option<String>,
    pub metadata: Option<Value>,
    /// true when the source frontmatter explicitly set `model-invocable`
    pub had_model_invocable_field: bool,
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
    pub fn schema_diagnostic(&self) -> mars_schema::diagnostic::SchemaDiagnostic {
        use mars_schema::diagnostic::{SchemaDiagnostic, Severity, key_path};

        let field = match self {
            Self::InvalidFieldValue { field, .. }
            | Self::InvalidFieldType { field, .. }
            | Self::RemovedField { field } => field.as_str(),
            Self::MalformedFrontmatter { .. } => "frontmatter",
        };
        SchemaDiagnostic::new(
            if self.is_error() {
                "skill-schema-error"
            } else {
                "skill-schema-warning"
            },
            if self.is_error() {
                Severity::Error
            } else {
                Severity::Warning
            },
            key_path(field),
            self.message(),
        )
    }

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
            Self::RemovedField { field } => format!("skill field `{field}` has been removed"),
            Self::MalformedFrontmatter { message } => {
                format!("skill frontmatter is malformed; raw fallback used: {message}")
            }
        }
    }

    fn plugin_message(&self) -> String {
        match self {
            Self::MalformedFrontmatter { message } => {
                format!("skill frontmatter is malformed: {message}")
            }
            _ => self.message(),
        }
    }
}

/// Authored MarsNative skill schema: required fields and reserved keys.
///
/// Runs on raw source frontmatter before lift. Lifted foreign imports (e.g. Cursor
/// Manual rules) never hit this path.
pub(crate) fn push_authored_skill_schema_diags_legacy(
    fm: &Frontmatter,
    diags: &mut Vec<SkillDiagnostic>,
) {
    if fm.has_frontmatter() {
        validate_required_string("description", fm.get("description"), diags);
    }
    if fm.get(RESERVED_IMPORT_MARKER).is_some() {
        diags.push(SkillDiagnostic::RemovedField {
            field: RESERVED_IMPORT_MARKER.to_string(),
        });
    }
}

pub(crate) fn push_authored_skill_schema_diags(
    fm: &Frontmatter,
    diags: &mut Vec<mars_schema::diagnostic::SchemaDiagnostic>,
) {
    let mut findings = Vec::new();
    push_authored_skill_schema_diags_legacy(fm, &mut findings);
    diags.extend(findings.iter().map(SkillDiagnostic::schema_diagnostic));
}

/// Emit warnings for foreign tool-field spellings in raw skill frontmatter.
/// Forward parsed skill schema diagnostics into the pipeline collector.
pub(crate) fn emit_skill_schema_diags(
    diag: &mut DiagnosticCollector,
    skill_name: &str,
    skill_diags: &[mars_schema::diagnostic::SchemaDiagnostic],
) {
    for d in skill_diags {
        diag.schema(d, &format!("skill `{skill_name}`"), false);
    }
}

pub(crate) fn emit_skill_schema_diags_from_source(
    source_path: &Path,
    skill_name: &str,
    diag: &mut DiagnosticCollector,
) {
    emit_skill_schema_diags_from_source_with(
        source_path,
        skill_name,
        diag,
        SkillDiagnostic::message,
    );
}

pub(crate) fn emit_plugin_skill_schema_diags_from_source(
    source_path: &Path,
    skill_name: &str,
    diag: &mut DiagnosticCollector,
) {
    emit_skill_schema_diags_from_source_with(
        source_path,
        skill_name,
        diag,
        SkillDiagnostic::plugin_message,
    );
}

fn emit_skill_schema_diags_from_source_with(
    source_path: &Path,
    skill_name: &str,
    diag: &mut DiagnosticCollector,
    message: fn(&SkillDiagnostic) -> String,
) {
    let skill_md = if source_path.is_dir() {
        source_path.join("SKILL.md")
    } else {
        source_path.to_path_buf()
    };
    let Ok(content) = std::fs::read_to_string(skill_md) else {
        return;
    };
    let mut skill_diags = Vec::new();
    let _ = parse_skill_content_legacy(&content, &mut skill_diags);
    for d in &skill_diags {
        let message = format!("skill `{skill_name}`: {}", message(d));
        if d.is_error() {
            diag.error_with_category(
                "skill-schema-error",
                message,
                DiagnosticCategory::Validation,
            );
        } else {
            diag.warn("skill-schema-warning", message);
        }
    }
}

/// Read a skill's `tools:` and `disallowed-tools:` (or its `disallowed_tools` spelling).
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

pub fn parse_skill_profile_legacy(
    fm: &Frontmatter,
    diags: &mut Vec<SkillDiagnostic>,
) -> SkillProfile {
    // Track which keys we consume so passthrough = all keys minus consumed.
    // This avoids a static list that drifts when new fields are added.
    let mut consumed_keys: Vec<String> = Vec::new();

    consumed_keys.push("name".to_string());
    let name_raw = fm.get("name");
    let name = name_raw.and_then(Value::as_str).map(str::to_owned);
    consumed_keys.push("description".to_string());
    let description_raw = fm.get("description");
    let description = description_raw.and_then(Value::as_str).map(str::to_owned);
    consumed_keys.push(RESERVED_IMPORT_MARKER.to_string());
    if fm.has_frontmatter() {
        validate_required_string("name", name_raw, diags);
    }
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
    for field in [
        "when_to_use",
        "tools",
        "disallowed-tools",
        "disallowed_tools",
        "allowed-tools",
        "allowed_tools",
    ] {
        consumed_keys.push(field.to_string());
    }
    consumed_keys.push("metadata".to_string());
    let metadata = fm.get("metadata").cloned();
    let skill_type = metadata
        .as_ref()
        .and_then(Value::as_mapping)
        .and_then(|mapping| mapping.get(Value::String("type".to_string())))
        .and_then(Value::as_str)
        .map(str::to_owned);

    let (model_invocable, had_model_invocable_field) =
        parse_invocability_bool("model-invocable", fm.get("model-invocable"), diags);
    consumed_keys.push("model-invocable".to_string());
    let (user_invocable, _) =
        parse_invocability_bool("user-invocable", fm.get("user-invocable"), diags);
    consumed_keys.push("user-invocable".to_string());

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
        skill_type,
        model_invocable,
        user_invocable,
        license,
        metadata,
        had_model_invocable_field,
        has_frontmatter: fm.has_frontmatter(),
        passthrough_fields,
    }
}

pub fn parse_skill_content_legacy(
    content: &str,
    diags: &mut Vec<SkillDiagnostic>,
) -> Result<(SkillProfile, Frontmatter), FrontmatterError> {
    let fm = Frontmatter::parse(content).inspect_err(|e| {
        diags.push(SkillDiagnostic::MalformedFrontmatter {
            message: e.to_string(),
        });
    })?;
    let profile = parse_skill_profile_legacy(&fm, diags);
    Ok((profile, fm))
}

pub fn parse_skill_profile(
    fm: &Frontmatter,
    diags: &mut Vec<mars_schema::diagnostic::SchemaDiagnostic>,
) -> SkillProfile {
    let mut findings = Vec::new();
    let profile = parse_skill_profile_legacy(fm, &mut findings);
    diags.extend(findings.iter().map(SkillDiagnostic::schema_diagnostic));
    profile
}

pub fn parse_skill_content(
    content: &str,
    diags: &mut Vec<mars_schema::diagnostic::SchemaDiagnostic>,
) -> Result<(SkillProfile, Frontmatter), FrontmatterError> {
    let fm = Frontmatter::parse(content).inspect_err(|error| {
        diags.push(
            SkillDiagnostic::MalformedFrontmatter {
                message: error.to_string(),
            }
            .schema_diagnostic(),
        );
    })?;
    let profile = parse_skill_profile(&fm, diags);
    Ok((profile, fm))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(content: &str) -> (SkillProfile, Vec<SkillDiagnostic>, Frontmatter) {
        let mut diags = Vec::new();
        let (profile, fm) = parse_skill_content_legacy(content, &mut diags).unwrap();
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
    fn frontmatter_requires_name() {
        let (_, d, _) = parse("---\nname: a\n---\nbody");
        assert!(d.is_empty());
        let (_, d, _) = parse("---\ndescription: b\n---\nbody");
        assert!(d.iter().any(|d| matches!(
            d,
            SkillDiagnostic::InvalidFieldValue { field, value, .. }
                if field == "name" && value == "missing"
        )));
    }

    #[test]
    fn canonical_parse_allows_missing_description() {
        let (profile, d, _) = parse("---\nname: a\nmodel-invocable: false\n---\nbody");
        assert!(d.is_empty());
        assert!(profile.description.is_none());
    }

    #[test]
    fn authored_skill_requires_description() {
        let fm = Frontmatter::parse("---\nname: a\nmodel-invocable: false\n---\nbody").unwrap();
        let mut d = Vec::new();
        push_authored_skill_schema_diags_legacy(&fm, &mut d);
        assert!(d.iter().any(|d| matches!(
            d,
            SkillDiagnostic::InvalidFieldValue { field, value, .. }
                if field == "description" && value == "missing"
        )));
    }

    #[test]
    fn authored_skill_rejects_reserved_import_marker() {
        let fm = Frontmatter::parse(
            "---\nname: a\ndescription: b\nmars.imported-without-description: true\n---\nbody",
        )
        .unwrap();
        let mut d = Vec::new();
        push_authored_skill_schema_diags_legacy(&fm, &mut d);
        assert!(d.iter().any(|d| matches!(
            d,
            SkillDiagnostic::RemovedField { field } if field == RESERVED_IMPORT_MARKER
        )));
    }

    #[test]
    fn warns_for_filtered_non_string_fields() {
        let (_, d, _) = parse(
            "---\nname: a\ndescription: b\ntools:\n  Bash(git *): allow\nlicense: false\n---\nbody",
        );
        assert!(d.iter().any(|d| matches!(
            d,
            SkillDiagnostic::InvalidFieldType { field, .. } if field == "license"
        )));
    }

    #[test]
    fn type_absent_gives_none() {
        let (p, d, _) = parse("---\nname: a\ndescription: b\n---\nbody");
        assert!(d.is_empty());
        assert!(p.skill_type.is_none());
    }

    #[test]
    fn malformed_yaml_raw_fallback_diagnostic() {
        let mut diags = Vec::new();
        let err =
            parse_skill_content_legacy("---\ninvalid: [:\n---\nbody", &mut diags).unwrap_err();
        assert!(matches!(err, FrontmatterError::MalformedYaml(_)));
        assert!(matches!(
            diags[0],
            SkillDiagnostic::MalformedFrontmatter { .. }
        ));
    }
}
