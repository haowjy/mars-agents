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

use super::{McpFile, McpLocation, McpServerEntry, SectionFormat, TargetAdapter};

#[derive(Debug)]
pub struct CodexAdapter;

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
            render,
        })
    }

    fn hook_config_file_names(&self) -> &'static [&'static str] {
        &["hooks.json"]
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

fn remove_owned_codex_hooks(
    records: &std::collections::BTreeMap<String, crate::lock::ConfigEntryRecord>,
    target_dir: &Path,
    diag: &mut crate::diagnostic::DiagnosticCollector,
) -> crate::surface_ownership::retention::RemovalReport {
    match super::hook_file::remove_recorded(&target_dir.join("hooks.json"), records, ".codex", diag)
    {
        Ok(()) => crate::surface_ownership::retention::RemovalReport::confirmed(),
        Err(error) => {
            crate::surface_ownership::retention::RemovalReport::failed(error, records.clone())
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
                emitted_json: serde_json::json!([{"hooks":[{"command":"original"}]}]).to_string(),
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
}
