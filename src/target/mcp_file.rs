//! The one MCP spec per adapter, and the one write / remove / validate path
//! driven by it.
//!
//! An adapter declares [`McpFile`]: the project-relative file its harness reads
//! MCP servers from, the format of the section holding them, and how one
//! [`McpServerEntry`] renders there. Preflight, the write, the removal and the
//! lock record all derive from that declaration, so a harness's file location
//! or shape is stated exactly once. Hooks are not part of this: their
//! event-array semantics differ.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use indexmap::IndexMap;
use mars_schema::mcp_server::{EnvRef, HeaderValue, McpTransport};
use serde_json::Value;

use super::mcp_doc::Doc;
use super::{ConfigEntry, McpServerEntry, TargetAdapter};
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::lock::ConfigEntryRecord;
use crate::surface_ownership::retention::{ConfigWrite, RemovalOperation, RemovalReport};

/// How the file holds its MCP servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpFormat {
    /// A JSON object at this top-level key maps server name to entry.
    JsonSection(&'static str),
    /// A TOML table at this top-level key holds one `[key.<name>]` table per server.
    TomlTable(&'static str),
}

/// A file and the format of its MCP section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpLocation {
    /// Path relative to the project root.
    pub path: &'static str,
    pub format: McpFormat,
}

/// One server rendered into a harness's native shape.
#[derive(Debug, Clone)]
pub struct McpRender {
    pub value: Value,
    /// What the native shape could not carry, one full sentence each; each
    /// becomes a warning.
    pub lossy: Vec<String>,
}

/// Where a harness reads MCP servers and how an entry renders there.
#[derive(Debug, Clone, Copy)]
pub struct McpFile {
    pub at: McpLocation,
    /// Where an older mars wrote entries, named by lock records that carry no
    /// `emitted_json`. `None` means no such record can name a file: it is dropped.
    /// Delete after one release.
    pub legacy: Option<McpLocation>,
    pub render: fn(&McpServerEntry) -> McpRender,
}

impl McpRender {
    pub fn exact(value: Value) -> Self {
        Self {
            value,
            lossy: Vec::new(),
        }
    }
}

/// An `mcpServers`-family entry. Claude, Cursor and Pi share it and differ only
/// in the interpolation and in whether a remote entry names `"type": "http"`:
/// stdio is `{ command, args, env: { KEY: <var> } }`, remote is
/// `{ url, headers: { NAME: <prefix><var> } }`.
pub(super) fn servers_entry(
    server: &McpServerEntry,
    interpolate: impl Fn(&str) -> String,
    typed_http: bool,
) -> Value {
    match &server.transport {
        McpTransport::Stdio { command, args, env } => {
            let mut entry = serde_json::json!({ "command": command, "args": args });
            if !env.is_empty() {
                entry["env"] = env
                    .iter()
                    .map(|(key, env_ref)| {
                        (key.clone(), Value::String(interpolate(env_ref.var_name())))
                    })
                    .collect::<serde_json::Map<_, _>>()
                    .into();
            }
            entry
        }
        McpTransport::Http { url, headers } => {
            let mut entry = serde_json::json!({ "url": url });
            if typed_http {
                entry["type"] = "http".into();
            }
            insert_headers(&mut entry, headers, interpolate);
            entry
        }
    }
}

/// Set `entry.headers` when there are any: a literal as written, an env
/// header as its prefix followed by the harness's reference to the variable.
pub(crate) fn insert_headers(
    entry: &mut Value,
    headers: &IndexMap<String, HeaderValue>,
    interpolate: impl Fn(&str) -> String,
) {
    if headers.is_empty() {
        return;
    }
    entry["headers"] = headers
        .iter()
        .map(|(name, value)| {
            let text = match value {
                HeaderValue::Literal(text) => text.clone(),
                HeaderValue::Env { env, prefix } => {
                    format!("{prefix}{}", interpolate(env.var_name()))
                }
            };
            (name.clone(), Value::String(text))
        })
        .collect::<serde_json::Map<_, _>>()
        .into();
}

/// The `${VAR}` rendering Claude and Pi share.
pub(crate) fn braced(var: &str) -> String {
    format!("${{{var}}}")
}

/// The entry Claude and Pi share: `${VAR}` references, remote servers typed `http`.
pub(super) fn braced_render(server: &McpServerEntry) -> McpRender {
    McpRender::exact(servers_entry(server, braced, true))
}

/// Split a stdio server's env by whether a harness can forward it by name:
/// `key == variable` is `forwarded`, anything else is `renamed` as
/// `(key, variable)`.
pub(crate) fn partition_env(env: &IndexMap<String, EnvRef>) -> (Vec<&str>, Vec<(&str, &str)>) {
    let mut forwarded = Vec::new();
    let mut renamed = Vec::new();
    for (key, env_ref) in env {
        let variable = env_ref.var_name();
        if key == variable {
            forwarded.push(variable);
        } else {
            renamed.push((key.as_str(), variable));
        }
    }
    (forwarded, renamed)
}

/// The lock record for one server: the canonical JSON of what is written.
/// Rendering also warns about what the native shape could not carry, so a
/// server is rendered once for both.
pub(crate) fn record_and_warn(
    file: &McpFile,
    server: &McpServerEntry,
    diag: &mut DiagnosticCollector,
) -> ConfigEntryRecord {
    let rendered = (file.render)(server);
    for lost in rendered.lossy {
        diag.warn("mcp-lossy", format!("MCP server `{}`: {lost}", server.name));
    }
    ConfigEntryRecord {
        emitted_json: Some(rendered.value.to_string()),
    }
}

/// What a lock record says about an entry found on disk.
pub(crate) enum Ownership {
    /// No record, or one that never recorded content: mars cannot vouch for it.
    Unowned,
    /// It equals what mars last wrote.
    Owned,
    /// Mars wrote it, and it has changed since.
    Edited,
}

pub(crate) fn ownership(record: Option<&ConfigEntryRecord>, current: &Value) -> Ownership {
    match record.and_then(|record| record.emitted_json.as_deref()) {
        None => Ownership::Unowned,
        Some(json) if serde_json::from_str::<Value>(json).is_ok_and(|v| v == *current) => {
            Ownership::Owned
        }
        Some(_) => Ownership::Edited,
    }
}

/// Check the file at `path` parses and has the shape `location` expects, before
/// anything is written over it.
pub(crate) fn validate(location: &McpLocation, path: &Path) -> Result<(), MarsError> {
    Doc::load(path, location.format).map(|_| ())
}

/// What a write may overwrite: the entries the previous lock recorded for this
/// target, and whether `--force` adopts the rest.
pub(crate) struct WriteOwnership<'a> {
    pub target_root: &'a str,
    pub previous: Option<&'a BTreeMap<String, ConfigEntryRecord>>,
    pub force: bool,
}

/// Write `write`'s MCP entries into the adapter's file. Returns the keys of the
/// entries now on disk as mars rendered them.
///
/// This is the one place an entry is replaced. A same-named entry is
/// overwritten only if the lock owns it and it still equals what mars wrote.
/// Anything else is the user's: preserved with a warning, or adopted under
/// `--force`.
pub(crate) fn write_entries(
    file: &McpFile,
    write: ConfigWrite<'_>,
    project_root: &Path,
    owner: &WriteOwnership<'_>,
    diag: &mut DiagnosticCollector,
) -> Result<BTreeSet<String>, MarsError> {
    let path = project_root.join(file.at.path);
    let (_, entries) = write.into_parts(project_root);
    let mut doc = match Doc::load(&path, file.at.format)? {
        Some(doc) => doc,
        None => Doc::new(file.at.format),
    };
    let target = owner.target_root;
    let owners = owner.previous;
    let force = owner.force;
    let mut written = BTreeSet::new();
    let mut changed = false;
    for entry in &entries {
        let ConfigEntry::McpServer(server) = entry else {
            continue;
        };
        let value = (file.render)(server).value;
        let name = &server.name;
        let recorded = owners.and_then(|records| records.get(&entry.key()));
        match doc.get(name) {
            Some(existing) if existing == value => {}
            Some(existing) => {
                match ownership(recorded, &existing) {
                    Ownership::Owned => {}
                    Ownership::Edited if force => {}
                    Ownership::Edited => {
                        diag.warn(
                            "mcp-divergent",
                            format!(
                                "target `{target}` MCP server `{name}` in `{}` was edited after mars wrote it (preserved; run `{}` to take it over)",
                                file.at.path,
                                crate::types::managed_cmd("mars sync --force")
                            ),
                        );
                        continue;
                    }
                    Ownership::Unowned if force => diag.warn(
                        "mcp-unmanaged-adopted",
                        format!(
                            "target `{target}` MCP server `{name}` in `{}` was not owned by mars; adopting with `--force`",
                            file.at.path
                        ),
                    ),
                    Ownership::Unowned => {
                        diag.warn(
                            "mcp-unmanaged-collision",
                            format!(
                                "target `{target}` MCP server `{name}` in `{}` exists but mars does not own it (preserved; run `{}` to take it over)",
                                file.at.path,
                                crate::types::managed_cmd("mars sync --force")
                            ),
                        );
                        continue;
                    }
                }
                doc.insert(name, value);
                changed = true;
            }
            None => {
                doc.insert(name, value);
                changed = true;
            }
        }
        written.insert(entry.key());
    }
    if changed {
        doc.save(&path)?;
    }
    Ok(written)
}

/// One entry to remove: its name, and the lock record that says mars wrote it.
type Removal<'a> = (&'a str, &'a ConfigEntryRecord);

/// Remove the entries `operation` names from where their lock records say they
/// live. An entry whose content no longer equals its record is the user's now:
/// it stays, with a warning. A record without `emitted_json` predates content
/// ownership and removes by name from the adapter's legacy file, if it has one.
pub(crate) fn remove_entries(
    adapter: &dyn TargetAdapter,
    operation: RemovalOperation<'_>,
    project_root: &Path,
    diag: &mut DiagnosticCollector,
) -> RemovalReport {
    let target = operation.target_root().to_owned();
    let (_, removal) = operation.into_parts(project_root);
    let Some(file) = adapter.mcp_file() else {
        return RemovalReport::confirmed();
    };
    // Group by where each record says its entry lives.
    let mut by_location: Vec<(McpLocation, Vec<Removal<'_>>)> = Vec::new();
    for key in &removal.keys_to_remove {
        let (Some(name), Some(record)) = (key.strip_prefix("mcp:"), removal.prior_records.get(key))
        else {
            continue;
        };
        let Some(at) = record
            .emitted_json
            .as_ref()
            .map_or(file.legacy, |_| Some(file.at))
        else {
            continue;
        };
        match by_location.iter_mut().find(|(seen, _)| *seen == at) {
            Some((_, names)) => names.push((name, record)),
            None => by_location.push((at, vec![(name, record)])),
        }
    }
    let mut kept = BTreeSet::new();
    for (at, names) in by_location {
        let delete_if_empty = at.path != file.at.path;
        match remove_names(project_root, at, &names, delete_if_empty, &target, diag) {
            Ok(left) => kept.extend(left.into_iter().map(|name| format!("mcp:{name}"))),
            Err(error) => return RemovalReport::failed(error, removal.prior_records.clone()),
        }
    }
    RemovalReport::confirmed_keeping(kept)
}

fn remove_names(
    project_root: &Path,
    at: McpLocation,
    names: &[Removal<'_>],
    delete_if_empty: bool,
    target: &str,
    diag: &mut DiagnosticCollector,
) -> Result<Vec<String>, MarsError> {
    let path = project_root.join(at.path);
    let Some(mut doc) = Doc::load(&path, at.format)? else {
        return Ok(Vec::new());
    };
    let mut changed = false;
    let mut kept = Vec::new();
    for (name, record) in names {
        // `Unowned` here is a record from before content ownership: it removes by name.
        if let Some(current) = doc.get(name)
            && matches!(ownership(Some(record), &current), Ownership::Edited)
        {
            diag.warn(
                "mcp-divergent",
                format!(
                    "target `{target}` MCP server `{name}` in `{}` was edited after mars wrote it (left in place; delete it by hand if unwanted)",
                    at.path
                ),
            );
            kept.push((*name).to_owned());
            continue;
        }
        changed |= doc.remove(name);
    }
    if delete_if_empty && doc.is_empty() {
        // The file an older mars wrote and no harness reads, now holding nothing.
        std::fs::remove_file(&path)?;
        return Ok(kept);
    }
    if changed {
        doc.save(&path)?;
    }
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface_ownership::retention::{RemovalPlan, Surface, WritePermit};
    use crate::target::TargetRegistry;
    use std::collections::BTreeMap;

    fn server(name: &str, env: &[(&str, &str)]) -> ConfigEntry {
        ConfigEntry::McpServer(McpServerEntry {
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
        })
    }

    fn write_with(
        target: &str,
        root: &Path,
        entries: Vec<ConfigEntry>,
        previous: Option<&BTreeMap<String, ConfigEntryRecord>>,
        force: bool,
    ) -> (BTreeSet<String>, Vec<String>) {
        let registry = TargetRegistry::new();
        let file = registry.get(target).unwrap().mcp_file().unwrap();
        let permit =
            WritePermit::for_test(Box::leak(target.to_owned().into_boxed_str()), Surface::Mcp);
        let mut diag = DiagnosticCollector::new();
        let written = write_entries(
            &file,
            permit.bind_config_entries(entries).unwrap(),
            root,
            &WriteOwnership {
                target_root: target,
                previous,
                force,
            },
            &mut diag,
        )
        .unwrap();
        (
            written,
            diag.drain()
                .into_iter()
                .map(|d| d.code.to_string())
                .collect(),
        )
    }

    fn write(target: &str, root: &Path, entries: Vec<ConfigEntry>) -> BTreeSet<String> {
        write_with(target, root, entries, None, false).0
    }

    fn remove(
        target: &str,
        root: &Path,
        previous: BTreeMap<String, ConfigEntryRecord>,
    ) -> Vec<String> {
        let registry = TargetRegistry::new();
        let adapter = registry.get(target).unwrap();
        let previous = BTreeMap::from([(target.to_owned(), previous)]);
        let mut diag = DiagnosticCollector::new();
        RemovalPlan::build(&previous, &BTreeMap::new())
            .execute(
                |operation, diag| remove_entries(adapter, operation, root, diag),
                &mut diag,
            )
            .into_retained_records();
        diag.drain()
            .into_iter()
            .map(|d| d.code.to_string())
            .collect()
    }

    fn record_of(target: &str, entry: &ConfigEntry) -> ConfigEntryRecord {
        let ConfigEntry::McpServer(server) = entry else {
            unreachable!()
        };
        let file = TargetRegistry::new()
            .get(target)
            .unwrap()
            .mcp_file()
            .unwrap();
        record_and_warn(&file, server, &mut DiagnosticCollector::new())
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
            write(target, tmp.path(), vec![server("srv", &env)]);
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
        let ConfigEntry::McpServer(renamed) = server("r", &[("A", "B")]) else {
            unreachable!()
        };
        record_and_warn(&adapter.mcp_file().unwrap(), &renamed, &mut diag);
        assert_eq!(diag.drain().len(), 1);

        write(".codex", tmp.path(), vec![entry.clone()]);
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
    fn an_old_record_moves_to_the_new_file_and_the_emptied_old_file_goes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let old = tmp.path().join(".claude/.mcp.json");
        let user = tmp.path().join(".codex/codex_mcp.json");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::create_dir_all(user.parent().unwrap()).unwrap();
        std::fs::write(&old, r#"{"mcpServers":{"srv":{"command":"node"}}}"#).unwrap();
        std::fs::write(&user, r#"{"mcpServers":{"srv":{},"theirs":{}}}"#).unwrap();
        let legacy = BTreeMap::from([(
            "mcp:srv".to_owned(),
            ConfigEntryRecord { emitted_json: None },
        )]);

        remove(".claude", tmp.path(), legacy.clone());
        remove(".codex", tmp.path(), legacy);

        assert!(!old.exists());
        assert_eq!(
            read(&user),
            serde_json::json!({"mcpServers": {"theirs": {}}})
        );
    }

    #[test]
    fn removal_keeps_other_entries_and_a_record_without_json_uses_the_legacy_section() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join(".opencode/opencode.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"mcp":{"new":{"type":"local"},"keep":{}},"mcpServers":{"old":{}}}"#,
        )
        .unwrap();
        remove(
            ".opencode",
            tmp.path(),
            BTreeMap::from([
                (
                    "mcp:new".to_owned(),
                    ConfigEntryRecord {
                        emitted_json: Some(r#"{"type":"local"}"#.to_owned()),
                    },
                ),
                (
                    "mcp:old".to_owned(),
                    ConfigEntryRecord { emitted_json: None },
                ),
            ]),
        );
        assert_eq!(read(&path), serde_json::json!({"mcp": {"keep": {}}}));
    }

    #[test]
    fn a_same_named_entry_the_lock_does_not_own_survives_until_forced() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join(".cursor/mcp.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"mcpServers":{"srv":{"command":"mine"}}}"#).unwrap();
        let entry = server("srv", &[]);

        let (written, codes) = write_with(".cursor", tmp.path(), vec![entry.clone()], None, false);
        assert!(written.is_empty());
        assert_eq!(codes, ["mcp-unmanaged-collision"]);
        assert_eq!(read(&path)["mcpServers"]["srv"]["command"], "mine");

        let (written, codes) = write_with(".cursor", tmp.path(), vec![entry], None, true);
        assert_eq!(written.len(), 1);
        assert_eq!(codes, ["mcp-unmanaged-adopted"]);
        assert_eq!(read(&path)["mcpServers"]["srv"]["command"], "node");
    }

    #[test]
    fn removal_leaves_an_entry_the_user_edited() {
        let tmp = tempfile::TempDir::new().unwrap();
        let entry = server("srv", &[]);
        write(".cursor", tmp.path(), vec![entry.clone()]);
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
