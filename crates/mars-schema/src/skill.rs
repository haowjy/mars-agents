//! Skill definition reading.

use crate::frontmatter::{Frontmatter, FrontmatterError};
use crate::invocability::parse_invocability_axis;
use serde_yaml::Value;
/// Reserved lift-internal key; must not appear in user-authored MarsNative packages.
const RESERVED_IMPORT_MARKER: &str = "mars.imported-without-description";

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct SkillProfile {
    pub name: Option<String>,
    pub description: Option<String>,
    /// `metadata.type`, kept apart for callers; the reading carries it under `metadata`.
    #[serde(skip)]
    pub skill_type: Option<String>,
    pub model_invocable: bool,
    pub user_invocable: bool,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    #[serde(serialize_with = "serialize_metadata")]
    pub metadata: Option<Value>,
    /// true when the source frontmatter explicitly set `model-invocable`
    #[serde(skip)]
    pub had_model_invocable_field: bool,
    #[serde(skip)]
    pub has_frontmatter: bool,
    /// Per-harness native keys (`harness-overrides:`), written only to that harness's file.
    #[serde(skip)]
    pub harness_overrides: crate::agent::HarnessOverrides,
}

/// Non-JSON YAML (tags, non-string keys) is left out, as in the reading's `frontmatter`,
/// where it carries a `yaml.non-json-value` diagnostic.
fn serialize_metadata<S: serde::Serializer>(
    value: &Option<Value>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::Serialize;
    value
        .as_ref()
        .and_then(|value| crate::ordered::OrderedValue::from_yaml(value).ok())
        .serialize(serializer)
}

const REMOVED_SKILL_FIELDS: &[(&str, &str)] = &[
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

fn parse_skill_profile_inner(
    fm: &Frontmatter,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
    reject_import_marker: bool,
) -> SkillProfile {
    use crate::diagnostic::{SchemaDiagnostic, Severity, child_path, key_path};

    let mut profile = SkillProfile {
        name: None,
        description: None,
        skill_type: None,
        model_invocable: true,
        user_invocable: true,
        license: None,
        compatibility: None,
        metadata: None,
        had_model_invocable_field: false,
        has_frontmatter: fm.has_frontmatter(),
        harness_overrides: Default::default(),
    };
    let mut classification_diagnostics = Vec::new();
    let mut field_diagnostics = Vec::new();

    for key in fm.keys() {
        let value = fm.get(&key).expect("frontmatter key came from mapping");
        match key.as_str() {
            "name" | "description" | "license" | "compatibility" => {
                let parsed = if let Some(text) = value.as_str() {
                    Some(if key == "name" {
                        text.trim().to_owned()
                    } else {
                        text.to_owned()
                    })
                } else {
                    field_diagnostics.push(SchemaDiagnostic::new(
                        "field.type",
                        Severity::Error,
                        key_path(&key),
                        format!("`{key}` must be a string"),
                    ));
                    None
                };
                match key.as_str() {
                    "name" => profile.name = parsed,
                    "description" => profile.description = parsed,
                    "license" => profile.license = parsed,
                    "compatibility" => profile.compatibility = parsed,
                    _ => unreachable!(),
                }
            }
            "metadata" => {
                profile.metadata = Some(value.clone());
                if let Some(mapping) = value.as_mapping() {
                    if let Some(skill_type) = mapping.get(Value::String("type".to_string())) {
                        if let Some(skill_type) = skill_type.as_str() {
                            profile.skill_type = Some(skill_type.to_owned());
                        } else {
                            field_diagnostics.push(SchemaDiagnostic::new(
                                "field.type",
                                Severity::Error,
                                child_path("metadata", "type"),
                                "`metadata.type` must be a string",
                            ));
                        }
                    }
                } else {
                    field_diagnostics.push(SchemaDiagnostic::new(
                        "field.type",
                        Severity::Error,
                        key_path("metadata"),
                        "`metadata` must be a map",
                    ));
                }
            }
            "model-invocable" => {
                let (value, had_field, invalid) = parse_invocability_axis(Some(value));
                profile.model_invocable = value;
                profile.had_model_invocable_field = had_field;
                if invalid.is_some() {
                    field_diagnostics.push(SchemaDiagnostic::new(
                        "field.type",
                        Severity::Error,
                        key_path("model-invocable"),
                        "`model-invocable` must be true or false",
                    ));
                }
            }
            "user-invocable" => {
                let (value, _, invalid) = parse_invocability_axis(Some(value));
                profile.user_invocable = value;
                if invalid.is_some() {
                    field_diagnostics.push(SchemaDiagnostic::new(
                        "field.type",
                        Severity::Error,
                        key_path("user-invocable"),
                        "`user-invocable` must be true or false",
                    ));
                }
            }
            "harness-overrides" => {
                profile.harness_overrides =
                    crate::agent::parse_harness_overrides("skill", value, &mut field_diagnostics);
            }
            RESERVED_IMPORT_MARKER => {
                if reject_import_marker {
                    classification_diagnostics.push(SchemaDiagnostic::new(
                        "field.removed",
                        Severity::Error,
                        key_path(RESERVED_IMPORT_MARKER),
                        format!("`{RESERVED_IMPORT_MARKER}` is reserved for imported skills"),
                    ));
                }
            }
            _ => {
                if let Some((_, replacement)) = REMOVED_SKILL_FIELDS
                    .iter()
                    .find(|(removed, _)| *removed == key)
                {
                    classification_diagnostics.push(SchemaDiagnostic::new(
                        "field.removed",
                        Severity::Error,
                        key_path(&key),
                        format!("`{key}` was removed; {replacement}"),
                    ));
                } else {
                    classification_diagnostics.push(SchemaDiagnostic::new(
                        "field.unknown",
                        Severity::Warning,
                        key_path(&key),
                        crate::agent::UNKNOWN_FIELD_MESSAGE.replace("{key}", &key),
                    ));
                }
            }
        }
    }

    diags.append(&mut classification_diagnostics);

    if !fm.has_frontmatter() {
        diags.push(SchemaDiagnostic::new(
            "skill.no-frontmatter",
            Severity::Warning,
            Vec::new(),
            "body-only skill has no frontmatter",
        ));
    } else {
        for (key, value) in [
            ("name", profile.name.as_deref()),
            ("description", profile.description.as_deref()),
        ] {
            if value.is_none_or(|value| value.trim().is_empty()) {
                diags.push(SchemaDiagnostic::new(
                    "field.required",
                    Severity::Error,
                    key_path(key),
                    format!("`{key}` must be a non-empty string"),
                ));
            }
        }
    }
    diags.append(&mut field_diagnostics);

    profile
}

pub fn parse_skill_profile(
    fm: &Frontmatter,
    diags: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) -> SkillProfile {
    parse_skill_profile_inner(fm, diags, false)
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
            let fields = parse_skill_profile_inner(&frontmatter, &mut diagnostics, true);
            crate::read::DefinitionRead {
                frontmatter: crate::read::frontmatter_json(frontmatter.mapping(), &mut diagnostics),
                fields: Some(fields),
                body: Some(frontmatter.body().to_string()),
                diagnostics,
            }
        }
        Err(error) => crate::read::DefinitionRead {
            frontmatter: Vec::new(),
            fields: None,
            body: None,
            diagnostics: vec![crate::diagnostic::frontmatter_error(&error)],
        },
    }
}
