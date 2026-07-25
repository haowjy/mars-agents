/// `.cursor` target adapter.
///
/// Handles MCP server registration for the Cursor IDE.
///
/// Cursor-native lowering:
/// - MCP: writes to `mcp.json` (mcpServers section), env vars as `${env:VAR}` syntax
///
use std::path::{Path, PathBuf};

use crate::error::MarsError;
use crate::lock::ItemKind;
use crate::types::DestPath;

use super::{ConfigEntry, HookEntry, HookFragmentMode, McpServerEntry, TargetAdapter};

#[derive(Debug)]
pub struct CursorAdapter;

impl TargetAdapter for CursorAdapter {
    fn name(&self) -> &str {
        ".cursor"
    }

    fn known_hook_events(&self) -> Option<&'static [&'static str]> {
        // cursor-agent 2026.07.16 installed-binary validator, extracted and
        // verified by p5714 on 2026-07-24; p5695 provided the docs baseline.
        Some(&[
            "beforeShellExecution",
            "beforeMCPExecution",
            "afterShellExecution",
            "afterMCPExecution",
            "beforeReadFile",
            "afterFileEdit",
            "beforeTabFileRead",
            "afterTabFileEdit",
            "stop",
            "beforeSubmitPrompt",
            "afterAgentResponse",
            "afterAgentThought",
            "sessionStart",
            "sessionEnd",
            "preCompact",
            "subagentStart",
            "subagentStop",
            "preToolUse",
            "postToolUse",
            "postToolUseFailure",
            "workspaceOpen",
        ])
    }

    fn hook_fragment_mode(&self) -> Option<HookFragmentMode> {
        Some(HookFragmentMode::MergeJson)
    }

    fn skill_variant_key(&self) -> Option<&str> {
        Some("cursor")
    }

    fn default_dest_path(&self, kind: ItemKind, name: &str) -> Option<DestPath> {
        match kind {
            ItemKind::Skill => Some(DestPath::from(format!("skills/{name}").as_str())),
            _ => None,
        }
    }

    fn write_config_entries(
        &self,
        _permit: crate::surface_ownership::retention::WritePermit<'_>,
        entries: &[ConfigEntry],
        target_dir: &Path,
    ) -> Result<Vec<PathBuf>, MarsError> {
        let mcp_servers: Vec<&McpServerEntry> = entries
            .iter()
            .filter_map(|e| {
                if let ConfigEntry::McpServer(s) = e {
                    Some(s)
                } else {
                    None
                }
            })
            .collect();
        let hooks: Vec<&HookEntry> = entries
            .iter()
            .filter_map(|entry| match entry {
                ConfigEntry::Hook(hook) => Some(hook),
                ConfigEntry::McpServer(_) => None,
            })
            .collect();

        let mut written = Vec::new();
        if !mcp_servers.is_empty() {
            written.push(write_cursor_mcp_json(target_dir, &mcp_servers)?);
        }
        if !hooks.is_empty() {
            written.push(write_cursor_hooks_json(target_dir, &hooks)?);
        }
        Ok(written)
    }

    fn mcp_config_file_names(&self) -> &'static [&'static str] {
        &["mcp.json"]
    }
    fn hook_config_file_names(&self) -> &'static [&'static str] {
        &["hooks.json"]
    }

    fn remove_owned_hook_entries(
        &self,
        _token: crate::surface_ownership::retention::RemovalToken<'_>,
        records: &std::collections::BTreeMap<String, crate::lock::ConfigEntryRecord>,
        target_dir: &Path,
        diag: &mut crate::diagnostic::DiagnosticCollector,
    ) -> Result<(), MarsError> {
        remove_owned_cursor_hooks(records, target_dir, diag)
    }

    fn remove_config_entries(
        &self,
        _token: crate::surface_ownership::retention::RemovalToken<'_>,
        entry_keys: &[String],
        target_dir: &Path,
    ) -> Result<(), MarsError> {
        remove_cursor_mcp_entries(entry_keys, target_dir)
    }
}

// Cursor uses a flat entry array and requires the version wrapper.
fn write_cursor_hooks_json(target_dir: &Path, hooks: &[&HookEntry]) -> Result<PathBuf, MarsError> {
    let path = target_dir.join("hooks.json");
    let mut root: serde_json::Value = if path.is_file() {
        super::parse_json_file(&path)?
    } else {
        serde_json::json!({})
    };
    let root_object = root.as_object_mut().ok_or_else(|| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("{} is not a JSON object", path.display()),
        })
    })?;
    root_object.insert("version".into(), serde_json::json!(1));
    let hooks_map = root_object
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            MarsError::Config(crate::error::ConfigError::Invalid {
                message: format!("{}: hooks is not an object", path.display()),
            })
        })?;
    for hook in hooks {
        super::append_json_event_entries(hooks_map, &hook.native_event, &hook.entries, &path)?;
    }
    crate::fs::atomic_write(
        &path,
        serde_json::to_string_pretty(&root)
            .map_err(|error| {
                MarsError::Config(crate::error::ConfigError::Invalid {
                    message: format!("failed to serialize {}: {error}", path.display()),
                })
            })?
            .as_bytes(),
    )?;
    Ok(path)
}

fn remove_owned_cursor_hooks(
    records: &std::collections::BTreeMap<String, crate::lock::ConfigEntryRecord>,
    target_dir: &Path,
    diag: &mut crate::diagnostic::DiagnosticCollector,
) -> Result<(), MarsError> {
    let path = target_dir.join("hooks.json");
    if !path.is_file() {
        return Ok(());
    }
    let mut root = super::parse_json_file(&path)?;
    let mut changed = false;
    if let Some(hooks_map) = root
        .get_mut("hooks")
        .and_then(serde_json::Value::as_object_mut)
    {
        for (key, record) in records {
            let Some((event, name)) = key
                .strip_prefix("hook:")
                .and_then(|rest| rest.split_once(':'))
            else {
                continue;
            };
            let Some(expected) = record
                .emitted_json
                .as_deref()
                .and_then(|json| serde_json::from_str::<Vec<serde_json::Value>>(json).ok())
            else {
                continue;
            };
            let update = super::remove_json_event_entries(hooks_map, event, &expected);
            changed |= update.changed;
            if update.missing > 0 {
                diag.warn(
                    "config-divergence",
                    format!(
                        "config-divergence: managed hook `{name}` diverged in target `.cursor` at `{}`; preserving edited config and appending the package entry",
                        path.display()
                    ),
                );
            }
        }
    }
    if changed
        && root
            .get("hooks")
            .and_then(serde_json::Value::as_object)
            .is_some_and(serde_json::Map::is_empty)
    {
        root.as_object_mut().unwrap().remove("hooks");
    }
    if !changed {
        return Ok(());
    }
    crate::fs::atomic_write(
        &path,
        serde_json::to_string_pretty(&root)
            .map_err(|error| {
                MarsError::Config(crate::error::ConfigError::Invalid {
                    message: format!("failed to serialize {}: {error}", path.display()),
                })
            })?
            .as_bytes(),
    )
}

// ---------------------------------------------------------------------------
// Cursor MCP — `mcp.json` format
// ---------------------------------------------------------------------------
//
// Cursor uses `${env:VAR_NAME}` interpolation syntax for env vars.
// {
//   "mcpServers": {
//     "server-name": {
//       "command": "...",
//       "args": [...],
//       "env": { "KEY": "${env:VAR_NAME}" }
//     }
//   }
// }

fn write_cursor_mcp_json(
    target_dir: &Path,
    servers: &[&McpServerEntry],
) -> Result<PathBuf, MarsError> {
    let path = target_dir.join("mcp.json");

    let mut root: serde_json::Value = if path.is_file() {
        super::parse_json_file(&path)?
    } else {
        serde_json::json!({})
    };

    let mcp_obj = root
        .as_object_mut()
        .ok_or_else(|| {
            MarsError::Config(crate::error::ConfigError::Invalid {
                message: format!("{} is not a JSON object", path.display()),
            })
        })?
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    let mcp_map = mcp_obj.as_object_mut().ok_or_else(|| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("{}: mcpServers is not an object", path.display()),
        })
    })?;

    for server in servers {
        let mut entry = serde_json::json!({
            "command": server.command,
            "args": server.args,
        });

        // Cursor env: `${env:VAR_NAME}` interpolation syntax.
        if !server.env.is_empty() {
            let env_obj: serde_json::Map<String, serde_json::Value> = server
                .env
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        serde_json::Value::String(format!("${{env:{v}}}")),
                    )
                })
                .collect();
            entry["env"] = serde_json::Value::Object(env_obj);
        }

        mcp_map.insert(server.name.clone(), entry);
    }

    let content = serde_json::to_string_pretty(&root).map_err(|e| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("failed to serialize {}: {e}", path.display()),
        })
    })?;
    crate::fs::atomic_write(&path, content.as_bytes())?;

    Ok(path)
}

fn remove_cursor_mcp_entries(entry_keys: &[String], target_dir: &Path) -> Result<(), MarsError> {
    let path = target_dir.join("mcp.json");
    if !path.is_file() {
        return Ok(());
    }

    let mut root = super::parse_json_file(&path)?;

    if let Some(mcp_map) = root
        .as_object_mut()
        .and_then(|o| o.get_mut("mcpServers"))
        .and_then(|v| v.as_object_mut())
    {
        for key in entry_keys {
            if let Some(name) = key.strip_prefix("mcp:") {
                mcp_map.remove(name);
            }
        }
    }

    let content = serde_json::to_string_pretty(&root).map_err(|e| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("failed to serialize {}: {e}", path.display()),
        })
    })?;
    crate::fs::atomic_write(&path, content.as_bytes())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface_ownership::retention::{RemovalToken, WritePermit};

    fn write_permit(entries: &[ConfigEntry]) -> WritePermit<'static> {
        WritePermit::for_test(".cursor", entries[0].surface())
    }
    use crate::target::McpServerEntry;
    use indexmap::IndexMap;
    use tempfile::TempDir;

    fn make_mcp_entry(name: &str, env_var: Option<(&str, &str)>) -> ConfigEntry {
        let mut env = IndexMap::new();
        if let Some((k, v)) = env_var {
            env.insert(k.to_string(), v.to_string());
        }
        ConfigEntry::McpServer(McpServerEntry {
            name: name.to_string(),
            command: "npx".to_string(),
            args: vec![],
            env,
        })
    }

    #[test]
    fn write_mcp_creates_mcp_json() {
        let tmp = TempDir::new().unwrap();
        let adapter = CursorAdapter;
        let entries = vec![make_mcp_entry("context7", None)];
        let written = adapter
            .write_config_entries(write_permit(&entries), &entries, tmp.path())
            .unwrap();
        assert_eq!(written.len(), 1);
        assert!(tmp.path().join("mcp.json").exists());

        let raw = std::fs::read_to_string(tmp.path().join("mcp.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(json["mcpServers"]["context7"].is_object());
    }

    #[test]
    fn write_mcp_env_uses_cursor_interpolation() {
        let tmp = TempDir::new().unwrap();
        let adapter = CursorAdapter;
        let entries = vec![make_mcp_entry("server", Some(("API_KEY", "MY_SECRET")))];
        adapter
            .write_config_entries(write_permit(&entries), &entries, tmp.path())
            .unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("mcp.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        // Cursor uses ${env:VAR_NAME} interpolation syntax
        assert_eq!(
            json["mcpServers"]["server"]["env"]["API_KEY"],
            "${env:MY_SECRET}"
        );
    }

    #[test]
    fn remove_mcp_entries_preserves_others() {
        let tmp = TempDir::new().unwrap();
        let adapter = CursorAdapter;
        let entries = vec![
            make_mcp_entry("to-remove", None),
            make_mcp_entry("to-keep", None),
        ];
        adapter
            .write_config_entries(write_permit(&entries), &entries, tmp.path())
            .unwrap();

        adapter
            .remove_config_entries(
                RemovalToken::for_test(),
                &["mcp:to-remove".to_string()],
                tmp.path(),
            )
            .unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("mcp.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(json["mcpServers"]["to-remove"].is_null());
        assert!(json["mcpServers"]["to-keep"].is_object());
    }

    #[test]
    fn divergent_structural_removal_does_not_rewrite_hooks() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("hooks.json");
        let original =
            br#"{"version":1,"hooks":{"sessionStart":[{"command":"edited"}]},"keep":true}"#;
        std::fs::write(&path, original).unwrap();
        let before_modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let records = std::collections::BTreeMap::from([(
            "hook:sessionStart:audit".to_string(),
            crate::lock::ConfigEntryRecord {
                emitted_json: Some(serde_json::json!([{"command":"original"}]).to_string()),
            },
        )]);

        remove_owned_cursor_hooks(
            &records,
            tmp.path(),
            &mut crate::diagnostic::DiagnosticCollector::new(),
        )
        .unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before_modified
        );
    }

    #[test]
    fn removal_prunes_empty_hooks_object() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("hooks.json");
        let owned = serde_json::json!({"command":"owned"});
        std::fs::write(
            &path,
            serde_json::json!({
                "version": 1,
                "hooks": {"sessionStart": [owned.clone()]}
            })
            .to_string(),
        )
        .unwrap();
        let records = std::collections::BTreeMap::from([(
            "hook:sessionStart:audit".to_string(),
            crate::lock::ConfigEntryRecord {
                emitted_json: Some(serde_json::json!([owned]).to_string()),
            },
        )]);

        remove_owned_cursor_hooks(
            &records,
            tmp.path(),
            &mut crate::diagnostic::DiagnosticCollector::new(),
        )
        .unwrap();

        let after: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(after, serde_json::json!({"version": 1}));
    }
}
