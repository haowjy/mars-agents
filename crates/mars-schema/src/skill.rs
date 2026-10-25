//! Skill definition reading.

use crate::frontmatter::{Frontmatter, FrontmatterError};
use crate::invocability::{parse_invocability_axis, value_label};
use serde_yaml::Value;
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
    pub fn schema_diagnostic(&self) -> crate::diagnostic::SchemaDiagnostic {
        use crate::diagnostic::{SchemaDiagnostic, Severity, key_path};

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

    pub fn plugin_message(&self) -> String {
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
pub fn push_authored_skill_schema_diags_legacy(fm: &Frontmatter, diags: &mut Vec<SkillDiagnostic>) {
    if fm.has_frontmatter() {
        validate_required_string("description", fm.get("description"), diags);
    }
    if fm.get(RESERVED_IMPORT_MARKER).is_some() {
        diags.push(SkillDiagnostic::RemovedField {
            field: RESERVED_IMPORT_MARKER.to_string(),
        });
    }
}

pub fn push_authored_skill_schema_diags(
    fm: &Frontmatter,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) {
    let mut findings = Vec::new();
    push_authored_skill_schema_diags_legacy(fm, &mut findings);
    diags.extend(findings.iter().map(SkillDiagnostic::schema_diagnostic));
}

/// Emit warnings for foreign tool-field spellings in raw skill frontmatter.
/// Forward parsed skill schema diagnostics into the pipeline collector.
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
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) -> SkillProfile {
    let mut findings = Vec::new();
    let profile = parse_skill_profile_legacy(fm, &mut findings);
    diags.extend(findings.iter().map(SkillDiagnostic::schema_diagnostic));
    profile
}

pub fn parse_skill_content(
    content: &str,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
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
