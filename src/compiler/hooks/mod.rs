//! Native hook discovery and fragment validation.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::diagnostic::DiagnosticCollector;
use crate::error::{ConfigError, MarsError};

pub(crate) const REMOVED_HOOK_SCHEMA_MESSAGE: &str = "uses the removed v0.11.0 hook schema (`events`/`matcher`/`[action]`/`path`); \
     migrate to per-target native fragment files with `fragment = \"<target>.json\"`";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookTarget {
    #[serde(default)]
    pub fragment: Option<String>,
    #[serde(default)]
    pub unchecked: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHookDef {
    #[serde(default)]
    name: Option<String>,
    #[serde(default = "default_visibility")]
    visibility: String,
    targets: BTreeMap<String, HookTarget>,
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
    pub order: i32,
}

#[derive(Debug, Clone)]
pub struct ParsedHookItem {
    pub def: HookDef,
    pub source_name: String,
    pub package_depth: usize,
    pub decl_order: usize,
    pub hook_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct HookFragment {
    pub events: BTreeMap<String, Vec<Value>>,
}

pub(crate) fn uses_removed_schema(value: &toml::Value) -> bool {
    let removed = ["events", "matcher", "action", "path"];
    let target_has_removed = value
        .get("targets")
        .and_then(toml::Value::as_table)
        .is_some_and(|targets| {
            targets.values().any(|value| {
                value
                    .as_table()
                    .is_some_and(|target| removed.iter().any(|key| target.contains_key(*key)))
            })
        });
    removed.iter().any(|key| value.get(*key).is_some())
        || target_has_removed
        || value.get("targets").is_some_and(toml::Value::is_array)
}

fn validate_path_component(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("must not be empty");
    }
    if name.contains(['/', '\\', '\0']) {
        return Err("must be a single path component");
    }
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(()),
        _ => Err("must be a single path component"),
    }
}

fn validate_fragment_path(path: &str) -> Result<(), &'static str> {
    if path.is_empty() || path.contains('\0') {
        return Err("must be a non-empty relative path");
    }
    if path.contains('\\') {
        return Err("must use a relative portable path");
    }
    if Path::new(path)
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("must be a relative path without `.` or `..`");
    }
    Ok(())
}

pub fn default_fragment_name(target: &str) -> String {
    format!("{}.json", target.trim_start_matches('.'))
}

pub fn discover_hook_items(
    package_root: &Path,
    source_name: &str,
    package_depth: usize,
    decl_order: usize,
) -> Result<Vec<ParsedHookItem>, MarsError> {
    let mut items = Vec::new();
    for hook_dir in crate::discover::discover_hook_directories(package_root)? {
        let dir_name = hook_dir
            .file_name()
            .expect("discovered hook directory has a name")
            .to_string_lossy()
            .into_owned();
        let toml_path = hook_dir.join("hook.toml");
        let raw = std::fs::read_to_string(&toml_path)?;
        let value: toml::Value = toml::from_str(&raw).map_err(|e| invalid_parse(&toml_path, e))?;
        if uses_removed_schema(&value) {
            return Err(ConfigError::RemovedHookSchema {
                path: toml_path,
                message: REMOVED_HOOK_SCHEMA_MESSAGE,
            }
            .into());
        }
        let raw_def: RawHookDef = toml::from_str(&raw).map_err(|e| invalid_parse(&toml_path, e))?;
        if raw_def.targets.is_empty() {
            return Err(invalid(&toml_path, "at least one target table is required"));
        }
        let name = raw_def.name.unwrap_or(dir_name);
        if let Err(message) = validate_path_component(&name) {
            return Err(invalid(
                &toml_path,
                &format!("invalid name `{name}`: {message}"),
            ));
        }
        for (target, spec) in &raw_def.targets {
            let fragment = spec
                .fragment
                .clone()
                .unwrap_or_else(|| default_fragment_name(target));
            if let Err(message) = validate_fragment_path(&fragment) {
                return Err(invalid(
                    &toml_path,
                    &format!("target `{target}` has invalid fragment `{fragment}`: {message}"),
                ));
            }
        }
        items.push(ParsedHookItem {
            def: HookDef {
                name,
                visibility: raw_def.visibility,
                targets: raw_def.targets,
                order: raw_def.order,
            },
            source_name: source_name.to_string(),
            package_depth,
            decl_order,
            hook_dir,
        });
    }
    Ok(items)
}

pub(crate) fn discover_resolved_hook_items(
    node: &crate::resolve::ResolvedNode,
    source_name: &crate::types::SourceName,
    package_depth: usize,
    decl_order: usize,
) -> Result<Vec<ParsedHookItem>, MarsError> {
    discover_hook_items(
        &node.rooted_ref.package_root,
        source_name.as_str(),
        package_depth,
        decl_order,
    )
    .map_err(|error| contextualize_dependency_error(error, source_name, node))
}

fn contextualize_dependency_error(
    error: MarsError,
    source_name: &crate::types::SourceName,
    node: &crate::resolve::ResolvedNode,
) -> MarsError {
    let MarsError::Config(ConfigError::RemovedHookSchema { message, .. }) = error else {
        return error;
    };
    let version = node
        .resolved_ref
        .version_tag
        .as_deref()
        .map(|version| version.trim_start_matches('v'))
        .or_else(|| {
            node.manifest
                .as_ref()
                .map(|manifest| manifest.package.version.as_str())
        })
        .unwrap_or("unknown");
    ConfigError::Invalid {
        message: format!(
            "source package `{source_name}` version `{version}` {message}; \
             suggested: `mars upgrade {source_name} --bump` or `mars remove {source_name}`"
        ),
    }
    .into()
}

pub fn load_merge_fragment(
    item: &ParsedHookItem,
    target_name: &str,
    known_events: &[&str],
    installed_hook_dir: &Path,
    diag: &mut DiagnosticCollector,
    emit_unchecked_warning: bool,
) -> Result<HookFragment, MarsError> {
    let target = &item.def.targets[target_name];
    let fragment_name = target
        .fragment
        .clone()
        .unwrap_or_else(|| default_fragment_name(target_name));
    let path = item.hook_dir.join(&fragment_name);
    let raw = std::fs::read_to_string(&path)
        .map_err(|error| invalid(&path, &format!("failed to read fragment: {error}")))?;
    let mut value: Value = serde_json::from_str(&raw)
        .map_err(|error| invalid(&path, &format!("fragment is not valid JSON: {error}")))?;
    if let Some(object) = value.as_object() {
        let wrapper_keys_ok = object
            .keys()
            .all(|key| matches!(key.as_str(), "hooks" | "version" | "description"));
        if object.contains_key("hooks") && wrapper_keys_ok {
            value = object.get("hooks").cloned().unwrap_or(Value::Null);
        }
    }
    let object = value.as_object().ok_or_else(|| {
        invalid(
            &path,
            "fragment top level must be an event-keyed JSON object",
        )
    })?;
    let mut events = BTreeMap::new();
    for (event, entries) in object {
        if !known_events.contains(&event.as_str()) {
            if target.unchecked.unwrap_or(false) {
                if emit_unchecked_warning {
                    diag.warn("hook-event-unchecked", format!("hook `{}` passes unknown event `{event}` through verbatim to `{target_name}` because `unchecked = true`", item.def.name));
                }
            } else {
                return Err(invalid(
                    &path,
                    &format!(
                        "unknown event `{event}` for target `{target_name}`; valid events: {}; use `unchecked = true` to pass a newer native event through verbatim",
                        known_events.join(", ")
                    ),
                ));
            }
        }
        let array = entries
            .as_array()
            .ok_or_else(|| invalid(&path, &format!("event `{event}` value must be an array")))?;
        events.insert(
            event.clone(),
            array
                .iter()
                .map(|entry| substitute_json_strings(entry.clone(), installed_hook_dir))
                .collect(),
        );
    }
    Ok(HookFragment { events })
}

/// Load a file-mode fragment and apply the same textual path substitution used
/// for JSON string values. File-mode fragments are otherwise opaque.
pub fn load_file_fragment(
    item: &ParsedHookItem,
    target_name: &str,
    installed_hook_dir: &Path,
) -> Result<String, MarsError> {
    let target = &item.def.targets[target_name];
    if target.unchecked.is_some() {
        return Err(invalid(
            &item.hook_dir.join("hook.toml"),
            &format!(
                "target `{target_name}` uses file-mode fragments; `unchecked` is not supported because file contents and events are not validated"
            ),
        ));
    }
    let fragment_name = target
        .fragment
        .clone()
        .unwrap_or_else(|| default_fragment_name(target_name));
    let path = item.hook_dir.join(fragment_name);
    let raw = std::fs::read_to_string(&path)
        .map_err(|error| invalid(&path, &format!("failed to read fragment: {error}")))?;
    Ok(substitute_hook_dir(&raw, installed_hook_dir))
}

fn substitute_hook_dir(text: &str, installed_hook_dir: &Path) -> String {
    let portable_hook_dir = installed_hook_dir.to_string_lossy().replace('\\', "/");
    text.replace("${MARS_HOOK_DIR}", &portable_hook_dir)
}

fn substitute_json_strings(mut value: Value, installed_hook_dir: &Path) -> Value {
    match &mut value {
        Value::String(text) => *text = substitute_hook_dir(text, installed_hook_dir),
        Value::Array(values) => {
            for value in values {
                *value = substitute_json_strings(value.take(), installed_hook_dir);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                *value = substitute_json_strings(value.take(), installed_hook_dir);
            }
        }
        _ => {}
    }
    value
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
    use super::{substitute_hook_dir, substitute_json_strings};
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn hook_dir_substitution_uses_escape_safe_separators_in_file_and_json_fragments() {
        let windows_path = Path::new(r"C:\temp\Users\hooks\audit");
        let substituted =
            substitute_hook_dir(r#"const SCRIPT = "${MARS_HOOK_DIR}/run.sh";"#, windows_path);
        assert_eq!(
            substituted,
            r#"const SCRIPT = "C:/temp/Users/hooks/audit/run.sh";"#
        );

        let substituted =
            substitute_json_strings(json!({"command": "${MARS_HOOK_DIR}/run.sh"}), windows_path);
        assert_eq!(
            substituted,
            json!({"command": "C:/temp/Users/hooks/audit/run.sh"})
        );
    }
}
