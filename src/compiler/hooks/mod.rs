//! Native hook discovery, validation, and deterministic ordering.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{ConfigError, MarsError};

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind")]
pub enum HookAction {
    #[serde(rename = "script")]
    Script { path: String },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookTarget {
    pub events: Vec<String>,
    #[serde(default)]
    pub matcher: Option<String>,
    #[serde(default)]
    pub unchecked: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHookDef {
    name: String,
    #[serde(default = "default_visibility")]
    visibility: String,
    targets: BTreeMap<String, HookTarget>,
    action: HookAction,
    #[serde(default)]
    order: i32,
}

fn default_visibility() -> String {
    "local".to_string()
}

#[derive(Debug, Clone)]
pub struct HookDef {
    pub name: String,
    pub visibility: String,
    pub targets: BTreeMap<String, HookTarget>,
    pub action: HookAction,
    pub order: i32,
}

#[derive(Debug, Clone)]
pub struct ParsedHookItem {
    pub def: HookDef,
    pub source_name: String,
    pub package_depth: usize,
    pub decl_order: usize,
    pub package_root: PathBuf,
}

fn validate_path_component(name: &str) -> Result<(), &'static str> {
    if name.contains('\0') {
        return Err("contains null byte");
    }
    for component in Path::new(name).components() {
        use std::path::Component;
        match component {
            Component::ParentDir => return Err("contains `..` component"),
            Component::RootDir | Component::Prefix(_) => {
                return Err("must not be an absolute path");
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_hook_script_path(path: &str) -> Result<(), &'static str> {
    if path.contains('\0') {
        return Err("contains null byte");
    }
    use std::path::Component;
    for component in Path::new(path).components() {
        match component {
            Component::ParentDir => return Err("contains `..` component"),
            Component::RootDir | Component::Prefix(_) => {
                return Err("must not be an absolute path");
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn discover_hook_items(
    package_root: &Path,
    source_name: &str,
    package_depth: usize,
    decl_order: usize,
) -> Result<Vec<ParsedHookItem>, MarsError> {
    let hooks_dir = package_root.join("hooks");
    if !hooks_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut items = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(&hooks_dir)
        .map_err(MarsError::from)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .collect();
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let toml_path = entry.path().join("hook.toml");
        if !toml_path.is_file() {
            continue;
        }
        let raw = std::fs::read_to_string(&toml_path).map_err(MarsError::from)?;
        let value: toml::Value =
            toml::from_str(&raw).map_err(|error| invalid_parse(&toml_path, error))?;
        if value.get("event").is_some() || value.get("targets").is_some_and(toml::Value::is_array) {
            return Err(MarsError::Config(ConfigError::Invalid {
                message: format!(
                    "{} uses the removed universal hook schema; migrate by replacing `event =` \
                     and `targets = [...]` with `[targets.\".claude\"]` (or another target) and \
                     `events = [\"<native event>\"]`",
                    toml_path.display()
                ),
            }));
        }
        let raw_def: RawHookDef =
            toml::from_str(&raw).map_err(|error| invalid_parse(&toml_path, error))?;

        if raw_def.targets.is_empty() {
            return Err(invalid(&toml_path, "at least one target table is required"));
        }
        for (target, spec) in &raw_def.targets {
            if spec.events.is_empty() {
                return Err(invalid(
                    &toml_path,
                    &format!("target `{target}` must declare at least one event"),
                ));
            }
            if spec.events.iter().any(|event| event.is_empty()) {
                return Err(invalid(&toml_path, "hook event names must not be empty"));
            }
        }
        if let Err(message) = validate_path_component(&raw_def.name) {
            return Err(invalid(
                &toml_path,
                &format!("invalid name `{}`: {message}", raw_def.name),
            ));
        }
        let HookAction::Script { path } = &raw_def.action;
        if let Err(message) = validate_hook_script_path(path) {
            return Err(invalid(
                &toml_path,
                &format!(
                    "hook `{}` has invalid script path `{path}`: {message}",
                    raw_def.name
                ),
            ));
        }

        items.push(ParsedHookItem {
            def: HookDef {
                name: raw_def.name,
                visibility: raw_def.visibility,
                targets: raw_def.targets,
                action: raw_def.action,
                order: raw_def.order,
            },
            source_name: source_name.to_string(),
            package_depth,
            decl_order,
            package_root: package_root.to_path_buf(),
        });
    }
    Ok(items)
}

fn invalid_parse(path: &Path, error: toml::de::Error) -> MarsError {
    invalid(path, &format!("failed to parse: {error}"))
}

fn invalid(path: &Path, message: &str) -> MarsError {
    MarsError::Config(ConfigError::Invalid {
        message: format!("{}: {message}", path.display()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_hook(root: &Path, name: &str, body: &str) {
        let dir = root.join("hooks").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hook.toml"), body).unwrap();
    }

    #[test]
    fn parses_multi_target_multi_event_and_matchers() {
        let temp = TempDir::new().unwrap();
        write_hook(
            temp.path(),
            "audit",
            r#"
name = "audit"
[targets.".claude"]
events = ["PreToolUse", "PostToolUse"]
matcher = "Bash"
[targets.".codex"]
events = ["SessionStart"]
[action]
kind = "script"
path = "run.sh"
"#,
        );
        let items = discover_hook_items(temp.path(), "base", 0, 0).unwrap();
        assert_eq!(items[0].def.targets[".claude"].events.len(), 2);
        assert_eq!(
            items[0].def.targets[".claude"].matcher.as_deref(),
            Some("Bash")
        );
        assert!(items[0].def.targets[".codex"].matcher.is_none());
    }

    #[test]
    fn old_schema_error_names_file_and_gives_migration_hint() {
        let temp = TempDir::new().unwrap();
        write_hook(
            temp.path(),
            "old",
            r#"name = "old"
event = "tool.pre"
targets = [".claude"]
[action]
kind = "script"
path = "run.sh"
"#,
        );
        let error = discover_hook_items(temp.path(), "base", 0, 0)
            .unwrap_err()
            .to_string();
        assert!(error.contains("hook.toml"));
        assert!(error.contains("removed universal hook schema"));
        assert!(error.contains("[targets.\".claude\"]"));
    }

    #[test]
    fn rejects_empty_events() {
        let temp = TempDir::new().unwrap();
        write_hook(
            temp.path(),
            "bad",
            r#"name = "bad"
[targets.".claude"]
events = []
[action]
kind = "script"
path = "run.sh"
"#,
        );
        assert!(
            discover_hook_items(temp.path(), "base", 0, 0)
                .unwrap_err()
                .to_string()
                .contains("at least one event")
        );
    }
}
