//! The MCP spec per adapter: where a harness reads MCP servers and how one
//! [`McpServerEntry`] renders there.
//!
//! An adapter declares [`McpFile`]; preflight, the reconcile and the lock record
//! all derive from it, so a harness's file location or shape is stated exactly
//! once. Ownership and reconcile are the shared keyed-entries engine.

use mars_schema::mcp_servers::{Flavor, McpRender, render};

use super::McpServerEntry;
use super::keyed_entries::{KeyedKind, KeyedSection, Rendered};
use crate::diagnostic::DiagnosticCollector;

pub(crate) const MCP_KIND: KeyedKind = KeyedKind {
    key_prefix: "mcp:",
    code: "mcp",
    noun: "MCP server",
    plural: "MCP servers",
};

#[derive(Debug, Clone, Copy)]
pub struct McpFile {
    pub at: KeyedSection,
    pub render: fn(&McpServerEntry) -> McpRender,
}

/// The entry Claude and Pi share: `${VAR}` references, remote servers typed `http`.
pub(super) fn claude_render(server: &McpServerEntry) -> McpRender {
    render(&server.transport, Flavor::Claude)
}

pub(crate) fn render_all(
    file: &McpFile,
    servers: &[McpServerEntry],
    diag: &mut DiagnosticCollector,
) -> Vec<Rendered> {
    servers
        .iter()
        .map(|server| {
            let rendered = (file.render)(server);
            // The native entry does less than the server declares, whoever wrote it; the text names
            // the step that is the reader's own.
            let own = server.is_own();
            for lost in rendered.lossy {
                let message = if own {
                    format!("MCP server `{}`: {}", server.name, lost.text(true))
                } else {
                    format!(
                        "MCP server `{}` (from `{}`): {}",
                        server.name,
                        server.source,
                        lost.text(false)
                    )
                };
                diag.hold_warning(file.at.entry_key(&server.name), "mcp-lossy", message);
            }
            Rendered {
                name: server.name.clone(),
                value: rendered.value,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::ConfigEntryRecord;
    use crate::target::TargetRegistry;
    use crate::target::keyed_entries::reconcile;
    use mars_schema::mcp_server::McpTransport;
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::path::Path;

    fn server(name: &str, env: &[(&str, &str)]) -> McpServerEntry {
        McpServerEntry {
            name: name.to_owned(),
            transport: McpTransport::Stdio {
                command: "node".to_owned(),
                args: vec!["server.js".to_owned()],
                env: env
                    .iter()
                    .map(|(key, var)| {
                        (
                            (*key).to_owned(),
                            mars_schema::mcp_server::EnvRef::Env {
                                var: (*var).to_owned(),
                            },
                        )
                    })
                    .collect(),
            },
            source: "_self".to_owned(),
        }
    }

    fn reconcile_with(
        target: &str,
        root: &Path,
        previous: &BTreeMap<String, ConfigEntryRecord>,
        desired: &[McpServerEntry],
        force: bool,
    ) -> (BTreeMap<String, ConfigEntryRecord>, Vec<String>) {
        let registry = TargetRegistry::new();
        let file = registry.get(target).unwrap().mcp_file().unwrap();
        let mut diag = DiagnosticCollector::new();
        let rendered = render_all(&file, desired, &mut diag);
        let records = reconcile(
            &MCP_KIND, &file.at, root, target, previous, &rendered, force, false, &mut diag,
        )
        .records;
        (
            records,
            diag.drain()
                .into_iter()
                .map(|d| d.code.to_string())
                .collect(),
        )
    }

    fn write(target: &str, root: &Path, entries: &[McpServerEntry]) {
        reconcile_with(target, root, &BTreeMap::new(), entries, false);
    }

    fn remove(
        target: &str,
        root: &Path,
        previous: BTreeMap<String, ConfigEntryRecord>,
    ) -> Vec<String> {
        reconcile_with(target, root, &previous, &[], false).1
    }

    fn record_of(target: &str, server: &McpServerEntry) -> ConfigEntryRecord {
        let file = TargetRegistry::new()
            .get(target)
            .unwrap()
            .mcp_file()
            .unwrap();
        render_all(
            &file,
            std::slice::from_ref(server),
            &mut DiagnosticCollector::new(),
        )[0]
        .record()
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn codex_toml_edit_keeps_user_content_and_forwards_env_by_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join(".codex/config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "# mine\nmodel = \"o3\"\n\n[mcp_servers.theirs]\ncommand = \"x\" # keep\n",
        )
        .unwrap();
        let entry = server("srv", &[("TOKEN", "TOKEN")]);
        let registry = TargetRegistry::new();
        let adapter = registry.get(".codex").unwrap();
        let mut diag = DiagnosticCollector::new();
        let file = adapter.mcp_file().unwrap();
        render_all(&file, &[server("r", &[("A", "B")])], &mut diag);
        // The loss is held until the entry is written.
        diag.release(&file.at.entry_key("r"));
        assert_eq!(diag.drain().len(), 1);

        write(".codex", tmp.path(), std::slice::from_ref(&entry));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# mine\nmodel = \"o3\"\n"), "{text}");
        assert!(text.contains("command = \"x\" # keep"), "{text}");
        assert!(text.contains("[mcp_servers.srv]"), "{text}");
        assert!(text.contains("env_vars = [\"TOKEN\"]"), "{text}");
        assert!(!text.contains("[mcp_servers]\n"), "{text}");

        remove(
            ".codex",
            tmp.path(),
            BTreeMap::from([("mcp:srv".to_owned(), record_of(".codex", &entry))]),
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("[mcp_servers.theirs]") && !text.contains("srv"),
            "{text}"
        );
    }

    #[test]
    fn a_same_named_entry_the_lock_does_not_own_survives_until_forced() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join(".cursor/mcp.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"mcpServers":{"srv":{"command":"mine"}}}"#).unwrap();
        let entry = server("srv", &[]);
        let none = BTreeMap::new();

        let (records, codes) = reconcile_with(
            ".cursor",
            tmp.path(),
            &none,
            std::slice::from_ref(&entry),
            false,
        );
        assert!(records.is_empty());
        assert_eq!(codes, ["mcp-unmanaged-collision"]);
        assert_eq!(read(&path)["mcpServers"]["srv"]["command"], "mine");

        let (records, codes) = reconcile_with(".cursor", tmp.path(), &none, &[entry], true);
        assert_eq!(records.len(), 1);
        assert_eq!(codes, ["mcp-unmanaged-adopted"]);
        assert_eq!(read(&path)["mcpServers"]["srv"]["command"], "node");
    }

    #[test]
    fn removal_leaves_an_entry_the_user_edited() {
        let tmp = tempfile::TempDir::new().unwrap();
        let entry = server("srv", &[]);
        write(".cursor", tmp.path(), std::slice::from_ref(&entry));
        let path = tmp.path().join(".cursor/mcp.json");
        let mut json = read(&path);
        json["mcpServers"]["srv"]["command"] = "edited".into();
        std::fs::write(&path, json.to_string()).unwrap();

        let codes = remove(
            ".cursor",
            tmp.path(),
            BTreeMap::from([("mcp:srv".to_owned(), record_of(".cursor", &entry))]),
        );
        assert!(codes.contains(&"mcp-divergent".to_owned()));
        assert_eq!(read(&path)["mcpServers"]["srv"]["command"], "edited");
    }
}
