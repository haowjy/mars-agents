/// `.codex` target adapter.
///
/// Handles MCP server registration and hook binding for the Codex harness.
///
/// Codex-native lowering:
/// - MCP: writes to `config.toml` (`[mcp.servers.*]`), env vars as plain names
/// - Hooks: writes to `codex_hooks.json` with structural hook entries
use std::path::{Path, PathBuf};

use crate::compiler::mcp::{HeaderValue, McpTransport};
use crate::error::MarsError;
use crate::lock::ItemKind;
use crate::types::DestPath;
use toml_edit::{Array, DocumentMut, Item, Table, Value, value};

use super::{ConfigEntry, HookEntry, McpServerEntry, TargetAdapter, hook_command};

#[derive(Debug)]
pub struct CodexAdapter;

const CODEX_CONFIG_TOML: &str = "config.toml";
const LEGACY_CODEX_MCP_JSON: &str = "codex_mcp.json";

impl TargetAdapter for CodexAdapter {
    fn name(&self) -> &str {
        ".codex"
    }

    fn skill_variant_key(&self) -> Option<&str> {
        Some("codex")
    }

    fn default_dest_path(&self, kind: ItemKind, name: &str) -> Option<DestPath> {
        match kind {
            ItemKind::Skill => Some(DestPath::from(format!("skills/{name}").as_str())),
            _ => None,
        }
    }

    fn write_config_entries(
        &self,
        entries: &[ConfigEntry],
        target_dir: &Path,
    ) -> Result<Vec<PathBuf>, MarsError> {
        let mut written = Vec::new();

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
            .filter_map(|e| {
                if let ConfigEntry::Hook(h) = e {
                    Some(h)
                } else {
                    None
                }
            })
            .collect();

        if !mcp_servers.is_empty() {
            let path = write_codex_mcp_toml(target_dir, &mcp_servers)?;
            written.push(path);
        }

        if !hooks.is_empty() {
            let path = write_codex_hooks_json(target_dir, &hooks)?;
            written.push(path);
        }

        Ok(written)
    }

    fn emit_pre_write_diagnostics(
        &self,
        entries: &[ConfigEntry],
        target_dir: &Path,
        diag: &mut crate::diagnostic::DiagnosticCollector,
    ) {
        let has_mcp_entries = entries
            .iter()
            .any(|entry| matches!(entry, ConfigEntry::McpServer(_)));
        if !has_mcp_entries {
            return;
        }

        let legacy_path = target_dir.join(LEGACY_CODEX_MCP_JSON);
        if legacy_path.is_file() {
            diag.info(
                "legacy-config-cleanup",
                format!(
                    "target `.codex`: removing legacy MCP config `{}` during sync",
                    legacy_path.display()
                ),
            );
        }

        let config_path = target_dir.join(CODEX_CONFIG_TOML);
        if config_path.is_file()
            && let Err(err) = parse_existing_toml_document(&config_path)
        {
            diag.warn(
                "codex-config-parse-error",
                format!(
                    "target `.codex`: cannot parse `{}`; skipping Codex MCP writes/removals until fixed: {err}",
                    config_path.display()
                ),
            );
        }
    }

    fn remove_config_entries(
        &self,
        entry_keys: &[String],
        target_dir: &Path,
    ) -> Result<(), MarsError> {
        remove_legacy_codex_mcp_json(target_dir)?;
        remove_codex_mcp_entries(entry_keys, target_dir)?;
        remove_codex_hook_entries(entry_keys, target_dir)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Codex MCP — `config.toml` format
// ---------------------------------------------------------------------------
//
// Codex uses plain environment variable names (no interpolation syntax).
// Format:
// [mcp.servers.my-server]
// command = "npx"
// args = ["-y", "my-server@latest"]
// env = ["MY_API_KEY"]

fn write_codex_mcp_toml(
    target_dir: &Path,
    servers: &[&McpServerEntry],
) -> Result<PathBuf, MarsError> {
    let path = target_dir.join(CODEX_CONFIG_TOML);
    remove_legacy_codex_mcp_json(target_dir)?;

    let mut doc = load_or_new_toml_document(&path)?;
    let mcp_servers = ensure_mcp_servers_table(&mut doc, &path)?;

    for server in servers {
        let mut server_table = Table::new();
        match server.transport {
            McpTransport::Stdio => {
                if let Some(command) = server.command.as_ref() {
                    server_table["command"] = value(command.as_str());
                }
                server_table["args"] = toml_string_array(server.args.clone());
            }
            McpTransport::Http => {
                if let Some(url) = server.url.as_ref() {
                    server_table["url"] = value(url.as_str());
                }

                let mut bearer_token_env_var: Option<String> = None;
                let mut http_headers = Table::new();
                for (header, value_ref) in &server.headers {
                    match value_ref {
                        HeaderValue::Plain(plain_value) => {
                            http_headers[header.as_str()] = value(plain_value.as_str());
                        }
                        HeaderValue::EnvRef(env_ref) => {
                            if header.eq_ignore_ascii_case("Authorization") {
                                bearer_token_env_var = Some(env_ref.var_name().to_string());
                            } else {
                                http_headers[header.as_str()] = value(env_ref.var_name());
                            }
                        }
                    }
                }

                if let Some(token_var) = bearer_token_env_var {
                    server_table["bearer_token_env_var"] = value(token_var);
                }
                if !http_headers.is_empty() {
                    server_table["http_headers"] = Item::Table(http_headers);
                }
            }
        }

        // Codex env: list of variable names (not a map with values).
        if !server.env.is_empty() {
            let env_vars: Vec<String> = server.env.values().cloned().collect();
            server_table["env"] = toml_string_array(env_vars);
        }

        mcp_servers.insert(server.name.as_str(), Item::Table(server_table));
    }

    crate::fs::atomic_write(&path, doc.to_string().as_bytes())?;
    Ok(path)
}

fn remove_codex_mcp_entries(entry_keys: &[String], target_dir: &Path) -> Result<(), MarsError> {
    let path = target_dir.join(CODEX_CONFIG_TOML);
    if !path.is_file() {
        return Ok(());
    }

    let mut doc = load_or_new_toml_document(&path)?;
    let mcp_servers = ensure_mcp_servers_table(&mut doc, &path)?;

    for key in entry_keys {
        if let Some(name) = key.strip_prefix("mcp:") {
            mcp_servers.remove(name);
        }
    }

    crate::fs::atomic_write(&path, doc.to_string().as_bytes())?;
    Ok(())
}

fn load_or_new_toml_document(path: &Path) -> Result<DocumentMut, MarsError> {
    if !path.is_file() {
        return Ok(DocumentMut::new());
    }

    parse_existing_toml_document(path)
}

fn parse_existing_toml_document(path: &Path) -> Result<DocumentMut, MarsError> {
    let raw = std::fs::read_to_string(path).map_err(MarsError::from)?;
    raw.parse::<DocumentMut>().map_err(|e| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!(
                "{}: failed to parse TOML; refusing to overwrite existing config: {e}",
                path.display()
            ),
        })
    })
}

fn toml_string_array(values: impl IntoIterator<Item = String>) -> Item {
    let mut array = Array::new();
    for value in values {
        array.push(value);
    }
    Item::Value(Value::Array(array))
}

fn ensure_mcp_servers_table<'a>(
    doc: &'a mut DocumentMut,
    path: &Path,
) -> Result<&'a mut Table, MarsError> {
    let root = doc.as_table_mut();

    let mcp_item = root
        .entry("mcp")
        .or_insert_with(|| Item::Table(Table::new()));
    let mcp_table = mcp_item.as_table_mut().ok_or_else(|| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("{}: mcp is not a table", path.display()),
        })
    })?;

    let servers_item = mcp_table
        .entry("servers")
        .or_insert_with(|| Item::Table(Table::new()));
    servers_item.as_table_mut().ok_or_else(|| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("{}: mcp.servers is not a table", path.display()),
        })
    })
}

fn remove_legacy_codex_mcp_json(target_dir: &Path) -> Result<(), MarsError> {
    let legacy_path = target_dir.join(LEGACY_CODEX_MCP_JSON);
    if !legacy_path.is_file() {
        return Ok(());
    }

    std::fs::remove_file(&legacy_path).map_err(MarsError::from)
}

// ---------------------------------------------------------------------------
// Codex hooks — `codex_hooks.json` format
// ---------------------------------------------------------------------------
//
// Structural hook entries — Codex uses event → command list mapping.
// {
//   "hooks": {
//     "pre-exec": ["bash /path/to/script.sh"],
//     "post-exec": [...]
//   }
// }

fn write_codex_hooks_json(target_dir: &Path, hooks: &[&HookEntry]) -> Result<PathBuf, MarsError> {
    let path = target_dir.join("codex_hooks.json");

    let mut root: serde_json::Value = if path.is_file() {
        let raw = std::fs::read_to_string(&path).map_err(MarsError::from)?;
        serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let hooks_section = root
        .as_object_mut()
        .ok_or_else(|| {
            MarsError::Config(crate::error::ConfigError::Invalid {
                message: format!("{} is not a JSON object", path.display()),
            })
        })?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    let hooks_map = hooks_section.as_object_mut().ok_or_else(|| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("{}: hooks is not an object", path.display()),
        })
    })?;

    for hook in hooks {
        let command = hook_command(&hook.script_path);
        let native_event = hook.native_event.clone();
        let event_hooks = hooks_map
            .entry(native_event.clone())
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .ok_or_else(|| {
                MarsError::Config(crate::error::ConfigError::Invalid {
                    message: format!("{}: hooks.{native_event} is not an array", path.display()),
                })
            })?;
        remove_managed_hook_commands(event_hooks, &hook.name);
        event_hooks.push(serde_json::Value::String(command));
    }

    let content = serde_json::to_string_pretty(&root).map_err(|e| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("failed to serialize {}: {e}", path.display()),
        })
    })?;
    crate::fs::atomic_write(&path, content.as_bytes())?;

    Ok(path)
}

fn remove_managed_hook_commands(commands: &mut Vec<serde_json::Value>, hook_name: &str) {
    commands.retain(|cmd| {
        cmd.as_str()
            .map(|cmd| !is_managed_hook_command_for(cmd, hook_name))
            .unwrap_or(true)
    });
}

fn is_managed_hook_command_for(command: &str, hook_name: &str) -> bool {
    let normalized = command.replace('\\', "/").replace("//", "/");
    normalized.contains(&format!("/hooks/{hook_name}/"))
}

fn remove_codex_hook_entries(entry_keys: &[String], target_dir: &Path) -> Result<(), MarsError> {
    let path = target_dir.join("codex_hooks.json");
    if !path.is_file() {
        return Ok(());
    }

    let hook_keys: Vec<(String, &str)> = entry_keys
        .iter()
        .filter_map(|k| {
            let rest = k.strip_prefix("hook:")?;
            let (event, name) = rest.split_once(':')?;
            Some((codex_hook_event(event)?.to_string(), name))
        })
        .collect();

    if hook_keys.is_empty() {
        return Ok(());
    }

    let raw = std::fs::read_to_string(&path).map_err(MarsError::from)?;
    let mut root: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({}));

    if let Some(hooks_map) = root
        .as_object_mut()
        .and_then(|o| o.get_mut("hooks"))
        .and_then(|v| v.as_object_mut())
    {
        for (event, name) in &hook_keys {
            if let Some(arr) = hooks_map.get_mut(event).and_then(|v| v.as_array_mut()) {
                arr.retain(|cmd| {
                    let cmd_str = cmd.as_str().unwrap_or("");
                    // Exact path-segment match to avoid partial name collisions.
                    !is_managed_hook_command_for(cmd_str, name)
                });
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

fn codex_hook_event(event: &str) -> Option<&'static str> {
    match event {
        "session.start" => Some("start"),
        "session.end" => Some("stop"),
        "tool.pre" => Some("pre-exec"),
        "tool.post" => Some("post-exec"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::DiagnosticCollector;
    use indexmap::IndexMap;
    use tempfile::TempDir;
    use toml::Value as TomlValue;

    fn make_stdio_mcp_entry(name: &str) -> ConfigEntry {
        let mut env = IndexMap::new();
        env.insert("API_KEY".to_string(), "MY_SECRET".to_string());
        ConfigEntry::McpServer(McpServerEntry {
            name: name.to_string(),
            transport: McpTransport::Stdio,
            command: Some("npx".to_string()),
            args: vec!["-y".to_string(), "some-mcp@latest".to_string()],
            env,
            url: None,
            headers: IndexMap::new(),
        })
    }

    fn make_http_mcp_entry(name: &str) -> ConfigEntry {
        let mut headers = IndexMap::new();
        headers.insert(
            "Authorization".to_string(),
            HeaderValue::EnvRef(crate::compiler::mcp::EnvRef::Env {
                var: "API_TOKEN".to_string(),
            }),
        );
        headers.insert(
            "X-Custom".to_string(),
            HeaderValue::Plain("static-value".to_string()),
        );

        ConfigEntry::McpServer(McpServerEntry {
            name: name.to_string(),
            transport: McpTransport::Http,
            command: None,
            args: vec![],
            env: IndexMap::new(),
            url: Some("https://api.example.com/mcp".to_string()),
            headers,
        })
    }

    fn make_hook_entry_with_path(name: &str, native: &str, script_path: &str) -> ConfigEntry {
        ConfigEntry::Hook(HookEntry {
            name: name.to_string(),
            event: "tool.pre".to_string(),
            native_event: native.to_string(),
            script_path: script_path.to_string(),
            order: 0,
        })
    }

    #[test]
    fn write_mcp_uses_config_toml_schema_and_preserves_non_mcp_content() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join(CODEX_CONFIG_TOML),
            r#"
[ui]
theme = "dark"
"#,
        )
        .unwrap();
        std::fs::write(tmp.path().join(LEGACY_CODEX_MCP_JSON), "{}").unwrap();

        let adapter = CodexAdapter;
        adapter
            .write_config_entries(
                &[
                    make_stdio_mcp_entry("stdio-server"),
                    make_http_mcp_entry("http-server"),
                ],
                tmp.path(),
            )
            .unwrap();

        assert!(!tmp.path().join(LEGACY_CODEX_MCP_JSON).exists());

        let raw = std::fs::read_to_string(tmp.path().join(CODEX_CONFIG_TOML)).unwrap();
        let toml: TomlValue = toml::from_str(&raw).unwrap();
        assert_eq!(toml["ui"]["theme"].as_str(), Some("dark"));

        let stdio = &toml["mcp"]["servers"]["stdio-server"];
        assert_eq!(stdio["command"].as_str(), Some("npx"));
        assert_eq!(stdio["args"][0].as_str(), Some("-y"));
        let env_arr = stdio["env"].as_array().unwrap();
        assert!(env_arr.iter().any(|v| v.as_str() == Some("MY_SECRET")));

        let http = &toml["mcp"]["servers"]["http-server"];
        assert_eq!(http["url"].as_str(), Some("https://api.example.com/mcp"));
        assert_eq!(http["bearer_token_env_var"].as_str(), Some("API_TOKEN"));
        assert_eq!(
            http["http_headers"]["X-Custom"].as_str(),
            Some("static-value")
        );
        assert!(http.get("command").is_none());
    }

    #[test]
    fn emit_pre_write_diagnostics_flags_legacy_cleanup_and_toml_parse_errors() {
        let adapter = CodexAdapter;

        let legacy_tmp = TempDir::new().unwrap();
        std::fs::write(legacy_tmp.path().join(LEGACY_CODEX_MCP_JSON), "{}").unwrap();
        let mut legacy_diag = DiagnosticCollector::new();
        adapter.emit_pre_write_diagnostics(
            &[make_stdio_mcp_entry("context7")],
            legacy_tmp.path(),
            &mut legacy_diag,
        );
        let legacy_messages = legacy_diag.drain();
        assert_eq!(legacy_messages.len(), 1);
        assert_eq!(legacy_messages[0].code, "legacy-config-cleanup");

        let invalid_tmp = TempDir::new().unwrap();
        std::fs::write(
            invalid_tmp.path().join(CODEX_CONFIG_TOML),
            "[ui
",
        )
        .unwrap();
        let mut invalid_diag = DiagnosticCollector::new();
        adapter.emit_pre_write_diagnostics(
            &[make_stdio_mcp_entry("context7")],
            invalid_tmp.path(),
            &mut invalid_diag,
        );
        let invalid_messages = invalid_diag.drain();
        assert_eq!(invalid_messages.len(), 1);
        assert_eq!(invalid_messages[0].code, "codex-config-parse-error");
    }

    #[test]
    fn invalid_toml_is_not_clobbered_during_write_or_remove() {
        let original = r#"[ui]
theme = "dark"
invalid =
"#;

        let write_tmp = TempDir::new().unwrap();
        std::fs::write(write_tmp.path().join(CODEX_CONFIG_TOML), original).unwrap();
        let adapter = CodexAdapter;
        let write_err = adapter
            .write_config_entries(&[make_stdio_mcp_entry("context7")], write_tmp.path())
            .expect_err("invalid TOML should fail and not be overwritten");
        assert!(write_err.to_string().contains("failed to parse TOML"));
        assert_eq!(
            std::fs::read_to_string(write_tmp.path().join(CODEX_CONFIG_TOML)).unwrap(),
            original
        );

        let remove_tmp = TempDir::new().unwrap();
        std::fs::write(remove_tmp.path().join(CODEX_CONFIG_TOML), original).unwrap();
        let remove_err = adapter
            .remove_config_entries(&["mcp:context7".to_string()], remove_tmp.path())
            .expect_err("invalid TOML should fail and not be overwritten");
        assert!(remove_err.to_string().contains("failed to parse TOML"));
        assert_eq!(
            std::fs::read_to_string(remove_tmp.path().join(CODEX_CONFIG_TOML)).unwrap(),
            original
        );
    }

    #[test]
    fn write_hooks_replaces_existing_managed_hook_for_same_name_and_event() {
        let tmp = TempDir::new().unwrap();
        let adapter = CodexAdapter;
        adapter
            .write_config_entries(
                &[make_hook_entry_with_path(
                    "audit",
                    "pre-exec",
                    "/old/hooks/audit/run.sh",
                )],
                tmp.path(),
            )
            .unwrap();
        adapter
            .write_config_entries(
                &[make_hook_entry_with_path(
                    "audit",
                    "pre-exec",
                    "/new/hooks/audit/run.sh",
                )],
                tmp.path(),
            )
            .unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("codex_hooks.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let hooks = json["hooks"]["pre-exec"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert!(hooks[0].as_str().unwrap().contains("/new/hooks/audit/"));
    }

    #[test]
    fn remove_entries_clean_up_mcp_and_only_matching_hook_event() {
        let tmp = TempDir::new().unwrap();
        let adapter = CodexAdapter;

        adapter
            .write_config_entries(
                &[
                    make_stdio_mcp_entry("to-remove"),
                    make_stdio_mcp_entry("to-keep"),
                    make_hook_entry_with_path("audit", "pre-exec", "/pkg/hooks/audit/run.sh"),
                    make_hook_entry_with_path("audit", "post-exec", "/pkg/hooks/audit/run.sh"),
                ],
                tmp.path(),
            )
            .unwrap();

        adapter
            .remove_config_entries(
                &[
                    "mcp:to-remove".to_string(),
                    "hook:tool.pre:audit".to_string(),
                ],
                tmp.path(),
            )
            .unwrap();

        let raw_toml = std::fs::read_to_string(tmp.path().join(CODEX_CONFIG_TOML)).unwrap();
        let toml: TomlValue = toml::from_str(&raw_toml).unwrap();
        assert!(toml["mcp"]["servers"].get("to-remove").is_none());
        assert!(toml["mcp"]["servers"]["to-keep"].is_table());

        let raw_hooks = std::fs::read_to_string(tmp.path().join("codex_hooks.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw_hooks).unwrap();
        assert!(json["hooks"]["pre-exec"].as_array().unwrap().is_empty());
        assert_eq!(json["hooks"]["post-exec"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn remove_hook_entries_match_windows_backslash_paths_without_partial_name_collision() {
        let tmp = TempDir::new().unwrap();
        let existing = serde_json::json!({
            "hooks": {
                "pre-exec": [
                    r#"bash "C:\\pkg\\hooks\\audit\\run.sh""#,
                    r#"bash "C:\\pkg\\hooks\\audit-extended\\run.sh""#
                ]
            }
        });
        std::fs::write(
            tmp.path().join("codex_hooks.json"),
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        remove_codex_hook_entries(&["hook:tool.pre:audit".to_string()], tmp.path()).unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("codex_hooks.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let hooks = json["hooks"]["pre-exec"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert!(hooks[0].as_str().unwrap().contains("audit-extended"));
    }
}
