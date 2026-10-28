/// `.codex` target adapter.
///
/// Handles MCP server registration and hook binding for the Codex harness.
///
/// Codex-native lowering:
/// - MCP: edits `[mcp_servers.<name>]` in `.codex/config.toml`, env vars forwarded by name
/// - Hooks: writes to `hooks.json` with Codex command hook entries
use std::path::Path;

use crate::error::MarsError;
use crate::types::DestPath;
use indexmap::IndexMap;
use mars_schema::item::ItemKind;
use mars_schema::mcp_server::{HeaderValue, McpTransport};
use mars_schema::mcp_servers::{McpRender, partition_env};

use super::{HookFragmentMode, McpFile, McpLocation, McpServerEntry, SectionFormat, TargetAdapter};

#[derive(Debug)]
pub struct CodexAdapter;

impl TargetAdapter for CodexAdapter {
    fn name(&self) -> &str {
        ".codex"
    }

    fn hook_fragment_mode(&self) -> Option<HookFragmentMode> {
        // https://developers.openai.com/codex/hooks — verified 2026-07-24.
        Some(HookFragmentMode::MergeJson {
            events: &[
                "SessionStart",
                // SessionEnd is documented at developers.openai.com/codex/hooks but was
                // runtime-verified non-firing in codex-cli 0.144.4 (2026-07-24). Re-add
                // once verified functional; authors can use `unchecked = true` meanwhile.
                "UserPromptSubmit",
                "PreToolUse",
                "PermissionRequest",
                "PostToolUse",
                "PreCompact",
                "PostCompact",
                "SubagentStart",
                "SubagentStop",
                "Stop",
            ],
        })
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

    fn write_hook_entries(
        &self,
        write: crate::surface_ownership::retention::ConfigWrite<'_>,
        project_root: &Path,
    ) -> Result<(), MarsError> {
        let (target_dir, hooks) = write.into_parts(project_root);
        if hooks.is_empty() {
            return Ok(());
        }
        super::hook_file::write(&target_dir.join("hooks.json"), &hooks, &[])
    }

    fn mcp_file(&self) -> Option<McpFile> {
        Some(McpFile {
            // Codex reads `[mcp_servers.<name>]` from the project's
            // `.codex/config.toml`, for trusted projects only.
            at: McpLocation {
                path: ".codex/config.toml",
                format: SectionFormat::TomlTable("mcp_servers"),
            },
            // Where mars wrote before. Remove in 0.25.
            legacy: Some(McpLocation {
                path: ".codex/codex_mcp.json",
                format: SectionFormat::JsonSection("mcpServers"),
            }),
            render,
        })
    }

    fn hook_config_file_names(&self) -> &'static [&'static str] {
        &["hooks.json"]
    }

    fn legacy_hook_config_file_names(&self) -> &'static [&'static str] {
        &["codex_hooks.json"]
    }

    fn remove_owned_hook_entries(
        &self,
        operation: crate::surface_ownership::retention::RemovalOperation<'_>,
        project_root: &Path,
        diag: &mut crate::diagnostic::DiagnosticCollector,
    ) -> crate::surface_ownership::retention::RemovalReport {
        let (target_dir, removal) = operation.into_parts(project_root);
        remove_owned_codex_hooks(&removal.prior_records, &target_dir, diag)
    }
}

// ---------------------------------------------------------------------------
// Codex MCP — `[mcp_servers.<name>]` entry shape
// ---------------------------------------------------------------------------
//
// Codex forwards an environment variable to the server by name, in
// `env_vars = ["VAR"]`. A mapping from a differently named key cannot be
// written, so a renamed reference is reported as lossy and dropped.

fn render(server: &McpServerEntry) -> McpRender {
    match &server.transport {
        McpTransport::Stdio { command, args, env } => {
            let mut entry = serde_json::json!({ "command": command, "args": args });
            let (forwarded, renamed) = partition_env(env);
            if !forwarded.is_empty() {
                entry["env_vars"] = serde_json::json!(forwarded);
            }
            McpRender {
                value: entry,
                lossy: renamed
                    .into_iter()
                    .map(|(key, variable)| {
                        format!(
                            "Codex forwards `{variable}` by name and cannot set it as `{key}`; `{key}` is not set for the server."
                        )
                    })
                    .collect(),
            }
        }
        McpTransport::Http { url, headers } => render_remote(url, headers),
    }
}

/// A Streamable HTTP server. Codex reads a header from the environment in
/// whole (`env_http_headers`, no prefix) and has one prefixed form,
/// `bearer_token_env_var` for `Authorization: Bearer <value>`; any other
/// prefixed env header has no spelling and is reported and dropped.
fn render_remote(url: &str, headers: &IndexMap<String, HeaderValue>) -> McpRender {
    let mut entry = serde_json::json!({ "url": url });
    let mut literals = serde_json::Map::new();
    let mut from_env = serde_json::Map::new();
    let mut lossy = Vec::new();
    for (name, value) in headers {
        match value {
            HeaderValue::Literal(text) => {
                literals.insert(name.clone(), text.as_str().into());
            }
            HeaderValue::Env { env, prefix } if prefix.is_empty() => {
                from_env.insert(name.clone(), env.var_name().into());
            }
            HeaderValue::Env { env, prefix }
                if name.eq_ignore_ascii_case("authorization") && prefix == "Bearer " =>
            {
                entry["bearer_token_env_var"] = env.var_name().into();
            }
            HeaderValue::Env { env, prefix } => lossy.push(format!(
                "Codex reads header `{name}` from `{}` whole and cannot prepend `{prefix}`; `{name}` is not set for the server.",
                env.var_name()
            )),
        }
    }
    if !literals.is_empty() {
        entry["http_headers"] = literals.into();
    }
    if !from_env.is_empty() {
        entry["env_http_headers"] = from_env.into();
    }
    McpRender {
        value: entry,
        lossy,
    }
}

// ---------------------------------------------------------------------------
// Codex hooks — `hooks.json` format
// ---------------------------------------------------------------------------
//
// Codex command hook entries.
// {
//   "hooks": {
//     "PreToolUse": [
//       {
//         "matcher": "Bash",
//         "hooks": [
//           { "type": "command", "command": "bash /path/to/script.sh" }
//         ]
//       }
//     ]
//   }
// }

fn remove_managed_hook_entries(bindings: &mut Vec<serde_json::Value>, hook_name: &str) -> bool {
    let mut removed = false;
    bindings.retain_mut(|binding| {
        if let Some(command) = binding.as_str() {
            let is_managed = is_managed_hook_command_for(command, hook_name);
            removed |= is_managed;
            return !is_managed;
        }

        let Some(hooks) = binding.get_mut("hooks").and_then(|v| v.as_array_mut()) else {
            return true;
        };
        let mut removed_from_binding = false;
        hooks.retain(|hook| {
            let is_managed = hook
                .get("command")
                .and_then(|v| v.as_str())
                .map(|command| is_managed_hook_command_for(command, hook_name))
                .unwrap_or(false);
            removed_from_binding |= is_managed;
            !is_managed
        });
        removed |= removed_from_binding;
        !removed_from_binding || !hooks.is_empty()
    });
    removed
}

fn is_managed_hook_command_for(command: &str, hook_name: &str) -> bool {
    let normalized = command.replace('\\', "/").replace("//", "/");
    normalized.contains(&format!("/hooks/{hook_name}/"))
}

fn remove_owned_codex_hooks(
    records: &std::collections::BTreeMap<String, crate::lock::ConfigEntryRecord>,
    target_dir: &Path,
    diag: &mut crate::diagnostic::DiagnosticCollector,
) -> crate::surface_ownership::retention::RemovalReport {
    if let Err(error) =
        super::hook_file::remove_recorded(&target_dir.join("hooks.json"), records, ".codex", diag)
    {
        return crate::surface_ownership::retention::RemovalReport::failed(error, records.clone());
    }
    // Legacy-Codex sweep for records without `emitted_json` (hook records have
    // carried it since v0.12.0). Remove in 0.25.
    let legacy_records: std::collections::BTreeMap<_, _> = records
        .iter()
        .filter(|(_, record)| record.emitted_json.is_none())
        .map(|(key, record)| (key.clone(), record.clone()))
        .collect();
    if legacy_records.is_empty() {
        return crate::surface_ownership::retention::RemovalReport::confirmed();
    }
    let names: Vec<&str> = legacy_records
        .keys()
        .filter_map(|key| key.strip_prefix(super::HOOK_KEY_PREFIX)?.split_once(':'))
        .map(|(_, name)| name)
        .collect();
    let sweep = |file: &str| {
        super::hook_file::sweep_events(&target_dir.join(file), |bindings| {
            names.iter().fold(false, |changed, name| {
                remove_managed_hook_entries(bindings, name) | changed
            })
        })
    };
    match sweep("hooks.json").and_then(|()| sweep("codex_hooks.json")) {
        Ok(()) => crate::surface_ownership::retention::RemovalReport::confirmed(),
        Err(error) => {
            crate::surface_ownership::retention::RemovalReport::failed(error, legacy_records)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface_ownership::retention::WritePermit;
    use crate::target::HookEntry;
    use tempfile::TempDir;

    fn make_hook_entry(name: &str, native: &str) -> HookEntry {
        HookEntry {
            name: name.to_string(),
            native_event: native.to_string(),
            entries: vec![
                serde_json::json!({"matcher": "Bash", "hooks": [{"type": "command", "command": format!("bash '/hooks/{name}/run.sh'")} ]}),
            ],
        }
    }

    fn make_hook_entry_with_path(name: &str, native: &str, script_path: &str) -> HookEntry {
        HookEntry {
            name: name.to_string(),
            native_event: native.to_string(),
            entries: vec![
                serde_json::json!({"hooks": [{"type": "command", "command": format!("bash '{script_path}'")} ]}),
            ],
        }
    }

    #[test]
    fn write_hooks_creates_hooks_json() {
        let tmp = TempDir::new().unwrap();
        let adapter = CodexAdapter;
        let entries = vec![make_hook_entry("audit", "PreToolUse")];
        adapter
            .write_hook_entries(
                WritePermit::for_test("").bind_hooks(entries.clone()),
                tmp.path(),
            )
            .unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("hooks.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let hooks = json["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(hooks[0]["matcher"], "Bash");
        assert_eq!(hooks[0]["hooks"][0]["type"], "command");
        assert!(
            hooks[0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("/hooks/audit/")
        );
    }

    #[test]
    fn write_hooks_appends_opaque_entries_in_call_order() {
        let tmp = TempDir::new().unwrap();
        let adapter = CodexAdapter;
        adapter
            .write_hook_entries(
                WritePermit::for_test("").bind_hooks(vec![make_hook_entry_with_path(
                    "audit",
                    "PreToolUse",
                    "/old/hooks/audit/run.sh",
                )]),
                tmp.path(),
            )
            .unwrap();
        adapter
            .write_hook_entries(
                WritePermit::for_test("").bind_hooks(vec![make_hook_entry_with_path(
                    "audit",
                    "PreToolUse",
                    "/new/hooks/audit/run.sh",
                )]),
                tmp.path(),
            )
            .unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("hooks.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let hooks = json["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 2);
        assert!(
            hooks[1]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("/new/hooks/audit/")
        );
    }

    #[test]
    fn remove_hook_entries_matches_backslash_commands() {
        let tmp = TempDir::new().unwrap();
        let existing = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [
                            { "type": "command", "command": "bash \"C:\\\\pkg\\\\hooks\\\\audit\\\\run.sh\"" }
                        ]
                    },
                    {
                        "matcher": "Bash",
                        "hooks": [
                            { "type": "command", "command": "bash \"C:\\\\pkg\\\\hooks\\\\audit-extended\\\\run.sh\"" }
                        ]
                    }
                ]
            }
        });
        std::fs::write(
            tmp.path().join("hooks.json"),
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        let records = std::collections::BTreeMap::from([(
            "hook:tool.pre:audit".to_string(),
            crate::lock::ConfigEntryRecord { emitted_json: None },
        )]);
        remove_owned_codex_hooks(
            &records,
            tmp.path(),
            &mut crate::diagnostic::DiagnosticCollector::new(),
        )
        .unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("hooks.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let hooks = json["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert!(
            hooks[0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("audit-extended")
        );
    }

    #[test]
    fn divergent_structural_removal_does_not_rewrite_hooks() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("hooks.json");
        let original =
            br#"{"hooks":{"SessionStart":[{"hooks":[{"command":"edited"}]}]},"keep":true}"#;
        std::fs::write(&path, original).unwrap();
        let before_modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let records = std::collections::BTreeMap::from([(
            "hook:SessionStart:audit".to_string(),
            crate::lock::ConfigEntryRecord {
                emitted_json: Some(
                    serde_json::json!([{"hooks":[{"command":"original"}]}]).to_string(),
                ),
            },
        )]);

        remove_owned_codex_hooks(
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
    fn remove_hook_entries_cleans_real_legacy_codex_hooks_json_only_by_managed_path() {
        let tmp = TempDir::new().unwrap();
        let legacy = serde_json::json!({
            "userSetting": "preserved",
            "hooks": {
                "pre-exec": [
                    "bash \"/cache/pkg/hooks/audit/run.sh\"",
                    "printf user-owned"
                ],
                "post-exec": ["bash \"/cache/pkg/hooks/audit/run.sh\""],
                "user-empty": []
            }
        });
        std::fs::write(
            tmp.path().join("codex_hooks.json"),
            serde_json::to_string_pretty(&legacy).unwrap(),
        )
        .unwrap();

        let records = std::collections::BTreeMap::from([(
            "hook:tool.pre:audit".to_string(),
            crate::lock::ConfigEntryRecord { emitted_json: None },
        )]);
        remove_owned_codex_hooks(
            &records,
            tmp.path(),
            &mut crate::diagnostic::DiagnosticCollector::new(),
        )
        .unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("codex_hooks.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(json["userSetting"], "preserved");
        assert_eq!(
            json["hooks"]["pre-exec"],
            serde_json::json!(["printf user-owned"])
        );
        assert!(json["hooks"]["post-exec"].is_null());
        assert_eq!(json["hooks"]["user-empty"], serde_json::json!([]));
    }
}
