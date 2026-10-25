//! Skill definition reading.

use crate::frontmatter::{Frontmatter, FrontmatterError};
use crate::invocability::parse_invocability_axis;
use serde_yaml::Value;
/// Reserved lift-internal key; must not appear in user-authored MarsNative packages.
const RESERVED_IMPORT_MARKER: &str = "mars.imported-without-description";

#[derive(Debug, Clone, serde::Serialize)]
pub struct SkillProfile {
    pub name: Option<String>,
    pub description: Option<String>,
    pub skill_type: Option<String>,
    pub model_invocable: bool,
    pub user_invocable: bool,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub metadata: Option<Value>,
    /// true when the source frontmatter explicitly set `model-invocable`
    pub had_model_invocable_field: bool,
    pub has_frontmatter: bool,
    /// Frontmatter fields not recognized by mars — passed through to all targets.
    pub passthrough_fields: Vec<(String, Value)>,
}

/// Add authored-skill checks that are not part of foreign lifting.
pub fn push_authored_skill_schema_diags(
    fm: &Frontmatter,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) {
    if fm.get(RESERVED_IMPORT_MARKER).is_some() {
        diags.push(crate::diagnostic::SchemaDiagnostic::new(
            "field.removed",
            crate::diagnostic::Severity::Error,
            crate::diagnostic::key_path(RESERVED_IMPORT_MARKER),
            format!("`{RESERVED_IMPORT_MARKER}` is reserved for imported skills"),
        ));
    }
}

fn parse_skill_profile_inner(fm: &Frontmatter) -> SkillProfile {
    let mut consumed_keys: Vec<String> = vec![
        "name",
        "description",
        RESERVED_IMPORT_MARKER,
        "license",
        "type",
        "when_to_use",
        "tools",
        "disallowed-tools",
        "disallowed_tools",
        "allowed-tools",
        "allowed_tools",
        "compatibility",
        "metadata",
        "model-invocable",
        "user-invocable",
        "invocation",
        "disable-model-invocation",
        "allow_implicit_invocation",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    consumed_keys.sort();

    let name = fm
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .map(str::to_owned);
    let description = fm
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let license = fm.get("license").and_then(Value::as_str).map(str::to_owned);
    let compatibility = fm
        .get("compatibility")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let metadata = fm.get("metadata").cloned();
    let skill_type = metadata
        .as_ref()
        .and_then(Value::as_mapping)
        .and_then(|mapping| mapping.get(Value::String("type".to_string())))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let (model_invocable, had_model_invocable_field, _) =
        parse_invocability_axis(fm.get("model-invocable"));
    let (user_invocable, _, _) = parse_invocability_axis(fm.get("user-invocable"));
    let passthrough_fields = fm
        .keys()
        .into_iter()
        .filter(|key| consumed_keys.binary_search(key).is_err())
        .filter_map(|key| fm.get(&key).cloned().map(|value| (key, value)))
        .collect();

    SkillProfile {
        name,
        description,
        skill_type,
        model_invocable,
        user_invocable,
        license,
        compatibility,
        metadata,
        had_model_invocable_field,
        has_frontmatter: fm.has_frontmatter(),
        passthrough_fields,
    }
}

pub fn parse_skill_profile(
    fm: &Frontmatter,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) -> SkillProfile {
    strict_skill_diagnostics(fm, diags);
    parse_skill_profile_inner(fm)
}

fn strict_skill_diagnostics(
    fm: &Frontmatter,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) {
    use crate::diagnostic::{SchemaDiagnostic, Severity, key_path};
    const KEPT: &[&str] = &[
        "name",
        "description",
        "license",
        "compatibility",
        "metadata",
        "model-invocable",
        "user-invocable",
    ];
    const REMOVED: &[(&str, &str)] = &[
        ("type", "use `metadata.type`"),
        ("when_to_use", "fold it into `description`"),
        ("tools", "remove this unused skill policy"),
        ("disallowed-tools", "remove this unused skill policy"),
        ("disallowed_tools", "remove this unused skill policy"),
        ("allowed-tools", "remove this unused skill policy"),
        ("allowed_tools", "remove this unused skill policy"),
        ("model_invocable", "use `model-invocable`"),
        ("user_invocable", "use `user-invocable`"),
        ("invocation", "use `model-invocable` and `user-invocable`"),
        ("disable-model-invocation", "use `model-invocable`"),
        ("allow_implicit_invocation", "use `model-invocable`"),
    ];
    for key in fm.keys() {
        if let Some((_, replacement)) = REMOVED.iter().find(|(removed, _)| *removed == key) {
            diags.push(SchemaDiagnostic::new(
                "field.removed",
                Severity::Error,
                key_path(&key),
                format!("`{key}` was removed; {replacement}"),
            ));
        } else if !KEPT.contains(&key.as_str()) && key != RESERVED_IMPORT_MARKER {
            diags.push(SchemaDiagnostic::new(
                "field.unknown",
                Severity::Warning,
                key_path(&key),
                format!("unknown field `{key}` is preserved"),
            ));
        }
    }
    if !fm.has_frontmatter() {
        diags.push(SchemaDiagnostic::new(
            "skill.no-frontmatter",
            Severity::Warning,
            Vec::new(),
            "body-only skill has no frontmatter",
        ));
    } else {
        for key in ["name", "description"] {
            if !matches!(fm.get(key), Some(Value::String(value)) if !value.trim().is_empty()) {
                diags.push(SchemaDiagnostic::new(
                    "field.required",
                    Severity::Error,
                    key_path(key),
                    format!("`{key}` must be a non-empty string"),
                ));
            }
        }
    }
    for key in ["name", "description", "license", "compatibility"] {
        if let Some(value) = fm.get(key)
            && !value.is_string()
        {
            diags.push(SchemaDiagnostic::new(
                "field.type",
                Severity::Error,
                key_path(key),
                format!("`{key}` must be a string"),
            ));
        }
    }
    if let Some(value) = fm.get("metadata")
        && !value.is_mapping()
    {
        diags.push(SchemaDiagnostic::new(
            "field.type",
            Severity::Error,
            key_path("metadata"),
            "`metadata` must be a map",
        ));
    }
    if let Some(value) = fm
        .get("metadata")
        .and_then(Value::as_mapping)
        .and_then(|metadata| metadata.get(Value::String("type".to_string())))
        && !value.is_string()
    {
        diags.push(SchemaDiagnostic::new(
            "field.type",
            Severity::Error,
            crate::diagnostic::child_path("metadata", "type"),
            "`metadata.type` must be a string",
        ));
    }
    for key in ["model-invocable", "user-invocable"] {
        if let Some(value) = fm.get(key)
            && !value.is_bool()
        {
            diags.push(SchemaDiagnostic::new(
                "field.type",
                Severity::Error,
                key_path(key),
                format!("`{key}` must be true or false"),
            ));
        }
    }
}

pub fn parse_skill_content(
    content: &str,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) -> Result<(SkillProfile, Frontmatter), FrontmatterError> {
    let fm = Frontmatter::parse(content).inspect_err(|error| {
        diags.push(crate::diagnostic::frontmatter_error(error));
    })?;
    let profile = parse_skill_profile(&fm, diags);
    Ok((profile, fm))
}

/// Read a skill definition through the versioned schema contract.
pub fn read(content: &str) -> crate::read::DefinitionRead<SkillProfile> {
    let mut diagnostics = Vec::new();
    match Frontmatter::parse(content) {
        Ok(frontmatter) => {
            let fields = parse_skill_profile(&frontmatter, &mut diagnostics);
            push_authored_skill_schema_diags(&frontmatter, &mut diagnostics);
            const KNOWN: &[&str] = &[
                "name",
                "description",
                "license",
                "compatibility",
                "metadata",
                "model-invocable",
                "user-invocable",
            ];
            let unknown = frontmatter
                .keys()
                .into_iter()
                .filter(|key| !KNOWN.contains(&key.as_str()))
                .filter_map(|key| frontmatter.get(&key).cloned().map(|value| (key, value)))
                .collect();
            crate::read::DefinitionRead {
                fields: Some(fields),
                body: Some(frontmatter.body().to_string()),
                unknown,
                diagnostics,
            }
        }
        Err(error) => crate::read::DefinitionRead {
            fields: None,
            body: None,
            unknown: Vec::new(),
            diagnostics: vec![crate::diagnostic::frontmatter_error(&error)],
        },
    }
}
