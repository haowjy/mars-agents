//! Claude plugin MCP servers, translated into Mars [`McpServerDef`]s.
//!
//! Sources: the plugin root's `.mcp.json` (the `mcpServers` wrapper is optional) and
//! `plugin.json`'s `mcpServers` (an inline map, a path to a JSON file inside the plugin, or an
//! array of those); a manifest server replaces a `.mcp.json` server of the same name.
//! Formats: <https://code.claude.com/docs/en/plugins/components> and
//! <https://code.claude.com/docs/en/mcp>.
//!
//! A server is imported whole or not at all: one field Mars cannot represent leaves the server
//! out, with one diagnostic per such field, because importing it without that field would
//! connect differently from what the plugin declares. A field that changes nothing about how
//! the server connects (`timeout`, unknown keys) is dropped with a warning and the server stays.
//! Every server imports as `local`.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::claude::manifest::{MARKETPLACE_MANIFEST, PLUGIN_MANIFEST, PluginSpec};
use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity};
use crate::mcp_server::McpServerDef;
use crate::mcp_servers::{self, NAME_INVALID, NAME_RULE, Parsed};
use crate::tree::TreeSource;
use crate::visibility::Visibility;

const MCP_JSON: &str = ".mcp.json";

/// One server Claude declares, translated.
pub(crate) struct ImportedServer {
    /// The server's name in the plugin: its directory under `mcp/`.
    pub name: String,
    /// The file that declared it.
    pub source_path: String,
    pub def: McpServerDef,
}

#[derive(Default)]
pub(crate) struct Imported {
    pub servers: Vec<ImportedServer>,
    /// By source path.
    pub diagnostics: Vec<(String, SchemaDiagnostic)>,
}

/// A server as declared, before translation.
struct Declared<'a> {
    file: String,
    /// Where the server's map sits in `file`, for diagnostics.
    prefix: Vec<PathSegment>,
    config: &'a Value,
}

pub(crate) fn import(source: &impl TreeSource, spec: &PluginSpec) -> Imported {
    let mut out = Imported::default();
    // Parsed documents outlive the declarations that borrow them.
    let mut documents: Vec<(String, Value)> = Vec::new();
    if let Some(value) = read_json(source, MCP_JSON, &mut out) {
        documents.push((MCP_JSON.to_owned(), value));
    }
    let manifest_value = source
        .read(PLUGIN_MANIFEST)
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|manifest| manifest.get("mcpServers").cloned());
    match manifest_value {
        Some(value) => {
            let entries = match value {
                Value::Array(entries) => entries,
                other => vec![other],
            };
            for (index, entry) in entries.into_iter().enumerate() {
                match entry {
                    Value::Object(_) => documents.push((PLUGIN_MANIFEST.to_owned(), entry)),
                    Value::String(reference) => {
                        documents.extend(manifest_file(source, &reference, index, &mut out));
                    }
                    other => out.warn(
                        PLUGIN_MANIFEST,
                        "mcp.manifest-entry-unsupported",
                        vec![key("mcpServers")],
                        format!("`mcpServers` entry {other} is not a map or a path to a JSON file, so it was left out"),
                    ),
                }
            }
        }
        // The marketplace entry declares servers the plugin's own files do not carry; only
        // `plugin.json` is read.
        None if spec.mcp => out.warn(
            MARKETPLACE_MANIFEST,
            "mcp.manifest-entry-unsupported",
            Vec::new(),
            "the marketplace entry declares `mcpServers`, which are not imported; declare them in the plugin's `.mcp.json`".into(),
        ),
        None => {}
    }

    // In order: `.mcp.json` first, then each manifest entry, so a later server of the same name
    // replaces an earlier one.
    let mut declared: BTreeMap<String, Declared<'_>> = BTreeMap::new();
    for (file, document) in &documents {
        let Some((prefix, servers)) = server_map(document) else {
            out.warn(
                file,
                "mcp.no-servers",
                Vec::new(),
                format!("`{file}` holds no map of MCP servers, so nothing was imported from it"),
            );
            continue;
        };
        for (name, config) in servers {
            let mut prefix = prefix.clone();
            prefix.push(key(name));
            declared.insert(
                name.clone(),
                Declared {
                    file: file.clone(),
                    prefix,
                    config,
                },
            );
        }
    }

    for (name, declared) in declared {
        if !mcp_servers::is_server_name(&name) {
            out.warn(
                &declared.file,
                NAME_INVALID,
                declared.prefix,
                format!(
                    "MCP server `{name}` was not imported: its name becomes a directory, so it may use only {NAME_RULE}"
                ),
            );
            continue;
        }
        let shadow = format!("mcp/{name}");
        if source.is_dir(&shadow) {
            out.warn(
                &declared.file,
                "mcp.shadowed-by-authored",
                declared.prefix,
                format!("MCP server `{name}` was not imported: the plugin also ships `{shadow}/`, which is used instead"),
            );
            continue;
        }
        if let Some(def) = translate(&name, &declared, &mut out) {
            out.servers.push(ImportedServer {
                name,
                source_path: declared.file,
                def,
            });
        }
    }
    out
}

impl Imported {
    fn warn(&mut self, file: &str, code: &str, path: Vec<PathSegment>, message: String) {
        self.diagnostics.push((
            file.to_owned(),
            SchemaDiagnostic::new(code, Severity::Warning, path, message),
        ));
    }
}

fn key(name: &str) -> PathSegment {
    PathSegment::Key(name.to_owned())
}

fn read_json(source: &impl TreeSource, file: &str, out: &mut Imported) -> Option<Value> {
    let bytes = source.read(file)?;
    match serde_json::from_slice(&bytes) {
        Ok(value) => Some(value),
        Err(error) => {
            out.warn(
                file,
                "mcp.invalid-json",
                Vec::new(),
                format!("`{file}` is not valid JSON ({error}), so nothing was imported from it"),
            );
            None
        }
    }
}

/// A `plugin.json` `mcpServers` string: a JSON file inside the plugin. A `.mcpb`/`.dxt` bundle or
/// a URL needs fetching or unpacking, which a pure reader of the tree cannot do.
fn manifest_file(
    source: &impl TreeSource,
    reference: &str,
    index: usize,
    out: &mut Imported,
) -> Option<(String, Value)> {
    let path = reference
        .strip_prefix("./")
        .unwrap_or(reference)
        .trim_end_matches('/');
    let local = !reference.contains("://")
        && !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."));
    if !local || !path.ends_with(".json") {
        out.warn(
            PLUGIN_MANIFEST,
            "mcp.manifest-entry-unsupported",
            vec![key("mcpServers"), PathSegment::Index(index)],
            format!("`mcpServers` entry `{reference}` is not a JSON file inside the plugin (a `.mcpb` bundle or a URL cannot be imported), so it was left out"),
        );
        return None;
    }
    if !source.is_file(path) {
        out.warn(
            PLUGIN_MANIFEST,
            "mcp.manifest-entry-unsupported",
            vec![key("mcpServers"), PathSegment::Index(index)],
            format!("`mcpServers` entry `{reference}` names a file the plugin does not contain, so it was left out"),
        );
        return None;
    }
    read_json(source, path, out).map(|value| (path.to_owned(), value))
}

/// The map of servers in a document, and where it sits: `{"mcpServers": {...}}` or the bare map.
fn server_map(document: &Value) -> Option<(Vec<PathSegment>, &Map<String, Value>)> {
    let object = document.as_object()?;
    match object.get("mcpServers") {
        Some(Value::Object(servers)) => Some((vec![key("mcpServers")], servers)),
        _ => Some((Vec::new(), object)),
    }
}

/// Translate one declared server, placing the codec's notes in the file that declared it.
fn translate(name: &str, declared: &Declared<'_>, out: &mut Imported) -> Option<McpServerDef> {
    let Parsed { transport, notes } = mcp_servers::parse(name, declared.config);
    for note in notes {
        let mut path = declared.prefix.clone();
        path.extend(note.field.as_deref().map(key));
        out.warn(&declared.file, note.code, path, note.message);
    }
    Some(McpServerDef {
        name: None,
        transport: transport?,
        visibility: Visibility::Local,
        targets: Vec::new(),
    })
}
