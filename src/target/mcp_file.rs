//! The one MCP spec per adapter, and the one reconcile / validate path driven
//! by it.
//!
//! An adapter declares [`McpFile`]: the project-relative file its harness reads
//! MCP servers from, the format of the section holding them, and how one
//! [`McpServerEntry`] renders there. Preflight, the reconcile and the lock
//! record all derive from that declaration, so a harness's file location or
//! shape is stated exactly once. Hooks are not part of this: their event-array
//! semantics differ.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use mars_schema::mcp_servers::{Flavor, McpRender, render};
use serde_json::Value;

use super::config_doc::{Doc, SectionFormat};
use super::{MCP_KEY_PREFIX, McpServerEntry};
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::lock::ConfigEntryRecord;

/// A file and the format of its MCP section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpLocation {
    /// Path relative to the project root.
    pub path: &'static str,
    pub format: SectionFormat,
}

/// Where a harness reads MCP servers and how an entry renders there.
#[derive(Debug, Clone, Copy)]
pub struct McpFile {
    pub at: McpLocation,
    pub render: fn(&McpServerEntry) -> McpRender,
}

/// The entry Claude and Pi share: `${VAR}` references, remote servers typed `http`.
pub(super) fn claude_render(server: &McpServerEntry) -> McpRender {
    render(&server.transport, Flavor::Claude)
}

/// One server rendered for its adapter: the native value mars writes and
/// records. Rendered once, where the server is lowered.
#[derive(Debug, Clone)]
pub(crate) struct Rendered {
    pub name: String,
    pub value: Value,
}

impl Rendered {
    pub(crate) fn key(&self) -> String {
        format!("{MCP_KEY_PREFIX}{}", self.name)
    }

    /// The lock record: the canonical JSON of what is written.
    pub(crate) fn record(&self) -> ConfigEntryRecord {
        ConfigEntryRecord {
            emitted_json: self.value.to_string(),
        }
    }
}

/// Render `servers` for `file`, warning about what the native shape could not carry.
pub(crate) fn render_all(
    file: &McpFile,
    servers: &[McpServerEntry],
    diag: &mut DiagnosticCollector,
) -> Vec<Rendered> {
    servers
        .iter()
        .map(|server| {
            let rendered = (file.render)(server);
            for lost in rendered.lossy {
                diag.warn("mcp-lossy", format!("MCP server `{}`: {lost}", server.name));
            }
            Rendered {
                name: server.name.clone(),
                value: rendered.value,
            }
        })
        .collect()
}

/// What a lock record says about an entry found on disk.
pub(crate) enum Ownership {
    /// No record: the entry is the user's.
    Unowned,
    /// It equals what mars last wrote.
    Owned,
    /// Mars wrote it, and it has changed since.
    Edited,
}

pub(crate) fn ownership(record: Option<&ConfigEntryRecord>, current: &Value) -> Ownership {
    let Some(record) = record else {
        return Ownership::Unowned;
    };
    if serde_json::from_str::<Value>(&record.emitted_json).is_ok_and(|v| v == *current) {
        Ownership::Owned
    } else {
        Ownership::Edited
    }
}

/// Check the file at `path` parses and has the shape `location` expects, before
/// anything is written over it.
pub(crate) fn validate(location: &McpLocation, path: &Path) -> Result<(), MarsError> {
    Doc::load(path, location.format).map(|_| ())
}

/// The file being edited: loaded once, saved at most once. A file that does not
/// exist loads as an empty document.
struct Open {
    path: PathBuf,
    doc: Doc,
    /// The file was there to load.
    existed: bool,
    changed: bool,
}

impl Open {
    fn load(project_root: &Path, at: McpLocation) -> Result<Self, MarsError> {
        let path = project_root.join(at.path);
        let loaded = Doc::load(&path, at.format)?;
        Ok(Self {
            path,
            existed: loaded.is_some(),
            doc: loaded.unwrap_or_else(|| Doc::new(at.format)),
            changed: false,
        })
    }

    fn remove(&mut self, name: &str) {
        self.changed |= self.doc.remove(name);
    }

    fn insert(&mut self, name: &str, value: Value) {
        self.doc.insert(name, value);
        self.changed = true;
    }
}

/// What a reconcile leaves: the records the lock should hold, and the keys of
/// the stale entries it took off the file's books (removed, or already gone).
pub(crate) struct Reconciled {
    pub records: BTreeMap<String, ConfigEntryRecord>,
    pub removed: Vec<String>,
}

/// Bring `file` to `desired`: remove what the lock owns and mars no longer
/// wants, write what it wants, and return exactly the records the lock should
/// now hold. Each file is loaded once and saved at most once.
///
/// This is the one place an MCP entry is replaced or removed. An entry is
/// mars's to touch only if the lock owns it and it still equals what mars wrote
/// ([`Ownership::Owned`]). An edited entry stays, with a warning; a same-named
/// entry the lock does not own stays too, until `--force` adopts it. The file is
/// deleted when this reconcile leaves it holding nothing but an empty section.
///
/// On failure nothing is retired: the result is `previous`'s MCP records, so
/// the next sync retries.
pub(crate) fn reconcile(
    file: &McpFile,
    project_root: &Path,
    target: &str,
    previous: &BTreeMap<String, ConfigEntryRecord>,
    desired: &[Rendered],
    force: bool,
    diag: &mut DiagnosticCollector,
) -> Reconciled {
    let prior: BTreeMap<&str, &ConfigEntryRecord> = previous
        .iter()
        .filter_map(|(key, record)| Some((key.strip_prefix(MCP_KEY_PREFIX)?, record)))
        .collect();
    match try_reconcile(file, project_root, target, &prior, desired, force, diag) {
        Ok(reconciled) => reconciled,
        Err(error) => {
            diag.warn(
                "config-entry-write",
                format!("failed to reconcile MCP servers in `{target}`: {error}"),
            );
            Reconciled {
                records: prior
                    .into_iter()
                    .map(|(name, record)| (format!("{MCP_KEY_PREFIX}{name}"), record.clone()))
                    .collect(),
                removed: Vec::new(),
            }
        }
    }
}

fn try_reconcile(
    file: &McpFile,
    project_root: &Path,
    target: &str,
    prior: &BTreeMap<&str, &ConfigEntryRecord>,
    desired: &[Rendered],
    force: bool,
    diag: &mut DiagnosticCollector,
) -> Result<Reconciled, MarsError> {
    let mut open = Open::load(project_root, file.at)?;
    let wanted: BTreeSet<&str> = desired.iter().map(|server| server.name.as_str()).collect();
    let mut kept = BTreeSet::new();

    // Records first: what the lock owns and mars no longer wants goes.
    for (&name, &record) in prior {
        if wanted.contains(name) {
            continue;
        }
        let Some(current) = open.doc.get(name) else {
            continue;
        };
        match ownership(Some(record), &current) {
            Ownership::Owned => open.remove(name),
            _ => {
                diag.warn(
                    "mcp-divergent",
                    format!(
                        "target `{target}` MCP server `{name}` in `{}` was edited after mars wrote it (left in place; delete it by hand if unwanted)",
                        file.at.path
                    ),
                );
                kept.insert(name);
            }
        }
    }

    let mut records = BTreeMap::new();
    for server in desired {
        let name = server.name.as_str();
        let recorded = prior.get(name).copied();
        let existing = open.doc.get(name);
        let write = match &existing {
            Some(existing) if *existing == server.value => true,
            Some(existing) => match ownership(recorded, existing) {
                Ownership::Owned => true,
                Ownership::Edited if force => true,
                Ownership::Edited => {
                    diag.warn(
                        "mcp-divergent",
                        format!(
                            "target `{target}` MCP server `{name}` in `{}` was edited after mars wrote it (preserved; run `{}` to take it over)",
                            file.at.path,
                            crate::types::managed_cmd("mars sync --force")
                        ),
                    );
                    false
                }
                Ownership::Unowned if force => {
                    diag.warn(
                        "mcp-unmanaged-adopted",
                        format!(
                            "target `{target}` MCP server `{name}` in `{}` was not owned by mars; adopting with `--force`",
                            file.at.path
                        ),
                    );
                    true
                }
                Ownership::Unowned => {
                    diag.warn(
                        "mcp-unmanaged-collision",
                        format!(
                            "target `{target}` MCP server `{name}` in `{}` exists but mars does not own it (preserved; run `{}` to take it over)",
                            file.at.path,
                            crate::types::managed_cmd("mars sync --force")
                        ),
                    );
                    false
                }
            },
            None => true,
        };
        if write {
            if existing.as_ref() != Some(&server.value) {
                open.insert(name, server.value.clone());
            }
            records.insert(server.key(), server.record());
        } else if let Some(record) = recorded {
            // Still on disk as the lock recorded it.
            records.insert(server.key(), record.clone());
        }
    }

    // Only a file this reconcile changed can be deleted: one mars never touched
    // may be the user's, empty on purpose.
    if open.changed {
        if open.existed && open.doc.is_empty() {
            std::fs::remove_file(&open.path)?;
        } else {
            open.doc.save(&open.path)?;
        }
    }

    let removed = prior
        .keys()
        .filter(|name| !wanted.contains(*name) && !kept.contains(*name))
        .map(|name| format!("{MCP_KEY_PREFIX}{name}"))
        .collect();
    Ok(Reconciled { records, removed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::TargetRegistry;
    use mars_schema::mcp_server::McpTransport;

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
        let records = reconcile(&file, root, target, previous, &rendered, force, &mut diag).records;
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
    fn each_adapter_writes_its_declared_file_and_shape() {
        let tmp = tempfile::TempDir::new().unwrap();
        let env = [("API_KEY", "MY_SECRET")];
        for (target, path, entry_at, expected) in [
            (
                ".claude",
                ".mcp.json",
                "/mcpServers/srv",
                serde_json::json!({"command": "node", "args": ["server.js"], "env": {"API_KEY": "${MY_SECRET}"}}),
            ),
            (
                ".cursor",
                ".cursor/mcp.json",
                "/mcpServers/srv",
                serde_json::json!({"command": "node", "args": ["server.js"], "env": {"API_KEY": "${env:MY_SECRET}"}}),
            ),
            (
                ".pi",
                ".pi/mcp.json",
                "/mcpServers/srv",
                serde_json::json!({"command": "node", "args": ["server.js"], "env": {"API_KEY": "${MY_SECRET}"}}),
            ),
            (
                ".opencode",
                ".opencode/opencode.json",
                "/mcp/srv",
                serde_json::json!({"type": "local", "command": ["node", "server.js"], "environment": {"API_KEY": "{env:MY_SECRET}"}}),
            ),
            // Codex forwards by name, so a renamed ref has no TOML spelling.
            (
                ".codex",
                ".codex/config.toml",
                "/mcp_servers/srv",
                serde_json::json!({"command": "node", "args": ["server.js"]}),
            ),
        ] {
            write(target, tmp.path(), &[server("srv", &env)]);
            let file = tmp.path().join(path);
            let root = if path.ends_with(".toml") {
                toml::from_str::<Value>(&std::fs::read_to_string(file).unwrap()).unwrap()
            } else {
                read(&file)
            };
            assert_eq!(root.pointer(entry_at), Some(&expected), "{target}");
        }
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
        render_all(
            &adapter.mcp_file().unwrap(),
            &[server("r", &[("A", "B")])],
            &mut diag,
        );
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
