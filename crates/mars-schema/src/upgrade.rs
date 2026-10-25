//! Pure legacy Mars definition and manifest upgrades to schema v1.

use serde_yaml::{Mapping, Value};
use toml_edit::{DocumentMut, Item};

use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity};
use crate::frontmatter::{Frontmatter, FrontmatterError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinitionKind {
    Agent,
    Skill,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradeResult {
    pub text: String,
    pub diagnostics: Vec<SchemaDiagnostic>,
}

impl UpgradeResult {
    pub fn changed_from(&self, input: &str) -> bool {
        self.text != input
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UpgradeError {
    #[error(transparent)]
    Frontmatter(#[from] FrontmatterError),
    #[error("failed to parse mars.toml: {0}")]
    Manifest(#[from] toml_edit::TomlError),
    #[error("mars.toml at a package root must contain [package]")]
    MissingPackage,
}

pub fn upgrade_definition(kind: DefinitionKind, text: &str) -> Result<UpgradeResult, UpgradeError> {
    let mut frontmatter = Frontmatter::parse(text)?;
    let mut diagnostics = Vec::new();
    match kind {
        DefinitionKind::Agent => upgrade_agent(&mut frontmatter, &mut diagnostics),
        DefinitionKind::Skill => upgrade_skill(&mut frontmatter, &mut diagnostics),
    }
    let upgraded = frontmatter.render();
    Ok(UpgradeResult {
        text: if upgraded == text {
            text.to_string()
        } else {
            upgraded
        },
        diagnostics,
    })
}

pub fn upgrade_manifest(text: &str) -> Result<UpgradeResult, UpgradeError> {
    let mut document = text.parse::<DocumentMut>()?;
    let package = document
        .get_mut("package")
        .and_then(Item::as_table_mut)
        .ok_or(UpgradeError::MissingPackage)?;
    if package.get("schema").and_then(Item::as_integer) == Some(1) {
        return Ok(UpgradeResult {
            text: text.to_string(),
            diagnostics: Vec::new(),
        });
    }
    package.insert("schema", toml_edit::value(1));
    let mut diagnostics = Vec::new();
    if package.remove("visibility").is_some() {
        diagnostics.push(removed(&["package", "visibility"], "visibility"));
    }
    Ok(UpgradeResult {
        text: document.to_string(),
        diagnostics,
    })
}

fn upgrade_agent(fm: &mut Frontmatter, diagnostics: &mut Vec<SchemaDiagnostic>) {
    upgrade_tools(fm);
    rename_field(fm, "model_invocable", "model-invocable");
    rename_field(fm, "user_invocable", "user-invocable");

    for key in [
        "network",
        "network-allow",
        "network-deny",
        "fanout",
        "models",
        "mcp-tools",
        "autocompact",
        "autocompact_pct",
    ] {
        if fm.remove(key).is_some() {
            diagnostics.push(removed(&[key], key));
        }
    }

    replace_scalar(fm, "effort", "adaptive", "medium");
    replace_scalar(fm, "effort", "disabled", "none");
    replace_scalar(fm, "approval", "yolo", "never");
    move_permission(fm);
    strip_model_policy_overrides(fm, diagnostics);
}

fn upgrade_skill(fm: &mut Frontmatter, diagnostics: &mut Vec<SchemaDiagnostic>) {
    rename_field(fm, "model_invocable", "model-invocable");
    rename_field(fm, "user_invocable", "user-invocable");

    if let Some(skill_type) = fm.get("type").cloned() {
        let metadata = fm
            .get("metadata")
            .cloned()
            .unwrap_or_else(|| Value::Mapping(Mapping::new()));
        if let Value::Mapping(mut metadata) = metadata {
            let key = Value::String("type".to_string());
            if !metadata.contains_key(&key) {
                metadata.insert(key, skill_type);
                fm.insert("metadata", Value::Mapping(metadata));
                fm.remove("type");
            }
        }
    }

    if let Some(when) = fm
        .get("when_to_use")
        .and_then(Value::as_str)
        .map(str::to_owned)
    {
        let description = match fm.get("description") {
            Some(Value::String(description)) => Some(format!("{description}\n\n{when}")),
            None => Some(when),
            Some(_) => None,
        };
        if let Some(description) = description {
            fm.insert("description", Value::String(description));
            fm.remove("when_to_use");
        }
    }

    for key in [
        "tools",
        "disallowed-tools",
        "allowed-tools",
        "disallowed_tools",
        "allowed_tools",
    ] {
        if fm.remove(key).is_some() {
            diagnostics.push(SchemaDiagnostic::new(
                "skill.tools-dropped",
                Severity::Warning,
                vec![PathSegment::Key(key.to_string())],
                format!("removed skill key `{key}`; skill tool rules have no v1 meaning"),
            ));
        }
    }
}

fn upgrade_tools(fm: &mut Frontmatter) {
    rename_field(fm, "disallowed_tools", "disallowed-tools");
    if let Some(Value::Sequence(entries)) = fm.get("tools").cloned()
        && entries.iter().all(|entry| entry.as_str().is_some())
    {
        let policy = entries
            .into_iter()
            .map(|entry| (entry, Value::String("allow".to_string())))
            .collect();
        fm.insert("tools", Value::Mapping(policy));
    }

    let Some(Value::Sequence(disallowed)) = fm.get("disallowed-tools").cloned() else {
        return;
    };
    if !disallowed.iter().all(|entry| entry.as_str().is_some()) {
        return;
    }
    let policy = fm
        .get("tools")
        .cloned()
        .unwrap_or_else(|| Value::Mapping(Mapping::new()));
    let Value::Mapping(mut policy) = policy else {
        return;
    };
    for entry in disallowed {
        policy.insert(entry, Value::String("deny".to_string()));
    }
    fm.insert("tools", Value::Mapping(policy));
    fm.remove("disallowed-tools");
}

fn move_permission(fm: &mut Frontmatter) {
    let Some(permission @ ("read" | "edit")) = fm.get("permission").and_then(Value::as_str) else {
        return;
    };
    let permission = permission.to_string();
    let overrides = fm
        .get("harness-overrides")
        .cloned()
        .unwrap_or_else(|| Value::Mapping(Mapping::new()));
    let Value::Mapping(mut overrides) = overrides else {
        return;
    };
    let flow_key = Value::String("flow".to_string());
    let flow = overrides
        .get(&flow_key)
        .cloned()
        .unwrap_or_else(|| Value::Mapping(Mapping::new()));
    let Value::Mapping(mut flow) = flow else {
        return;
    };
    let permission_key = Value::String("permission".to_string());
    if flow.contains_key(&permission_key) {
        return;
    }
    flow.insert(permission_key, Value::String(permission));
    overrides.insert(flow_key, Value::Mapping(flow));
    fm.insert("harness-overrides", Value::Mapping(overrides));
    fm.remove("permission");
}

fn strip_model_policy_overrides(fm: &mut Frontmatter, diagnostics: &mut Vec<SchemaDiagnostic>) {
    let Some(Value::Sequence(policies)) = fm.get_mut("model-policies") else {
        return;
    };
    for (index, policy) in policies.iter_mut().enumerate() {
        let Value::Mapping(policy) = policy else {
            continue;
        };
        let Some(Value::Mapping(overrides)) = policy.get_mut(Value::String("override".to_string()))
        else {
            continue;
        };
        for key in ["network", "autocompact", "autocompact_pct", "harness"] {
            if overrides.remove(Value::String(key.to_string())).is_some() {
                diagnostics.push(SchemaDiagnostic::new(
                    "field.removed",
                    Severity::Warning,
                    vec![
                        PathSegment::Key("model-policies".to_string()),
                        PathSegment::Index(index),
                        PathSegment::Key("override".to_string()),
                        PathSegment::Key(key.to_string()),
                    ],
                    format!("removed model policy override key `{key}`"),
                ));
            }
        }
    }
}

fn rename_field(fm: &mut Frontmatter, old: &str, new: &str) {
    let Some(value) = fm.remove(old) else {
        return;
    };
    if fm.get(new).is_none() {
        fm.insert(new, value);
    }
}

fn replace_scalar(fm: &mut Frontmatter, key: &str, old: &str, new: &str) {
    if fm.get(key).and_then(Value::as_str) == Some(old) {
        fm.insert(key, Value::String(new.to_string()));
    }
}

fn removed(path: &[&str], key: &str) -> SchemaDiagnostic {
    SchemaDiagnostic::new(
        "field.removed",
        Severity::Warning,
        path.iter()
            .map(|part| PathSegment::Key((*part).to_string()))
            .collect(),
        format!("removed legacy key `{key}`"),
    )
}
