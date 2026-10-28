/// MCP server compiler lane.
///
/// Discovers, parses, validates, and lowers MCP server definitions from
/// package trees into per-target config entries.
///
/// Responsibilities:
/// - Parse `mcp/<name>/mcp.toml` from package roots
/// - Preserve env references symbolically (mars never resolves secrets)
/// - Warn (or error under `--strict`) when an env var is absent at sync time
/// - Provide parsed MCP items for per-target collision resolution
/// - Lower each item to a `McpServerEntry { name, transport }` per target for
///   adapter config writing
use std::path::Path;

pub(crate) use mars_schema::mcp_server::MCP_DIR_TOKEN;
use mars_schema::mcp_server::{
    HeaderValue, McpServerDef, McpTransport, ParsedMcpToml, parse_mcp_toml,
};

use crate::compiler::visibility::Visibility;
use crate::diagnostic::DiagnosticCollector;
use crate::error::{ConfigError, MarsError};

// ---------------------------------------------------------------------------
// Discovered items
// ---------------------------------------------------------------------------

/// A discovered MCP server item with provenance.
#[derive(Debug, Clone)]
pub struct ParsedMcpItem {
    /// Resolved server name (directory name, unless overridden in TOML).
    pub name: String,
    /// Parsed definition.
    pub def: McpServerDef,
    /// Whether this server crosses package boundaries.
    pub visibility: Visibility,
    /// Source package name this item came from.
    pub source_name: String,
    /// Authored definition used for content diagnostics.
    pub source_path: std::path::PathBuf,
    /// Shortest dependency distance from the consumer package.
    pub depth: usize,
    /// Declaration order of the source package in the consumer graph.
    pub decl_order: usize,
}

impl ParsedMcpItem {
    /// Authored directory holding `mcp.toml` and any files the server ships.
    pub fn server_dir(&self) -> &Path {
        self.source_path
            .parent()
            .expect("discovered MCP definition belongs to a server directory")
    }

    /// Whether `command` or `args` reference the server's own directory. Only
    /// such a server gets its directory installed beside a target's config.
    pub fn uses_dir_token(&self) -> bool {
        def_uses_dir_token(&self.def)
    }
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// Discover MCP server items from a package root.
///
/// Scans `<package_root>/mcp/<name>/mcp.toml` for each subdirectory.
/// Returns the parsed items in directory-sorted order.
///
/// `depth` is the source's shortest graph distance from the consumer; the local
/// package is 0, so the caller supplies the precedence rather than inferring it
/// from `source_name`.
pub(crate) fn discover_mcp_items(
    package_root: &Path,
    source_name: &str,
    depth: usize,
    decl_order: usize,
    diag: &mut DiagnosticCollector,
) -> Result<Vec<ParsedMcpItem>, MarsError> {
    let definitions = discover_mcp_definition_paths(package_root, diag)?;
    let mut items = Vec::new();
    for toml_path in definitions {
        let entry_dir = toml_path
            .parent()
            .expect("discovered MCP definition belongs to a server directory");
        let server_name = entry_dir
            .file_name()
            .expect("discovered MCP server directory has a name")
            .to_string_lossy();

        let raw = std::fs::read_to_string(&toml_path).map_err(MarsError::from)?;
        let ParsedMcpToml {
            def,
            ignored_fields,
        } = parse_mcp_toml(&raw).map_err(|e| {
            MarsError::Config(ConfigError::Invalid {
                message: format!("failed to parse {}: {e}", toml_path.display()),
            })
        })?;
        let resolved_name = def.name.as_deref().unwrap_or(&server_name).to_string();
        for field in ignored_fields {
            diag.warn(
                "mcp-ignored-field",
                format!(
                    "{}: `{field}` does not apply to this server's transport and is ignored",
                    toml_path.display()
                ),
            );
        }
        let visibility = Visibility::parse(
            &def.visibility,
            "MCP server",
            &resolved_name,
            source_name,
            diag,
        );
        // Only a server that installs its own directory needs a name that is a
        // safe path component; a token-free server is only a config entry.
        if def_uses_dir_token(&def)
            && let Err(message) = crate::compiler::validate_path_component(&resolved_name)
        {
            let relative = toml_path.strip_prefix(package_root).unwrap_or(&toml_path);
            diag.warn(
                "mcp-invalid-name",
                format!(
                    "MCP server `{resolved_name}` in `{source_name}` at `{}` uses {MCP_DIR_TOKEN}, so its directory name {message}; skipping this server",
                    relative.display()
                ),
            );
            continue;
        }

        items.push(ParsedMcpItem {
            name: resolved_name,
            def,
            visibility,
            source_name: source_name.to_string(),
            source_path: toml_path,
            depth,
            decl_order,
        });
    }
    Ok(items)
}

pub(crate) fn discover_mcp_definition_paths(
    package_root: &Path,
    diag: &mut DiagnosticCollector,
) -> Result<Vec<std::path::PathBuf>, MarsError> {
    if skip_generated_tree(package_root, diag) {
        return Ok(Vec::new());
    }
    let mcp_dir = package_root.join("mcp");
    if !mcp_dir.is_dir() {
        return Ok(Vec::new());
    }
    if skip_generated_tree(&mcp_dir, diag) {
        return Ok(Vec::new());
    }

    let mut entries: Vec<_> = std::fs::read_dir(&mcp_dir)
        .map_err(MarsError::from)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    let mut definitions = Vec::new();
    for entry in entries {
        if skip_generated_tree(&entry.path(), diag) {
            continue;
        }
        let dir_name = entry.file_name();
        let server_name = dir_name.to_string_lossy();
        // Skip hidden directories.
        if server_name.starts_with('.') {
            continue;
        }

        let toml_path = entry.path().join("mcp.toml");
        if !toml_path.is_file() {
            continue;
        }

        definitions.push(toml_path);
    }
    Ok(definitions)
}

fn skip_generated_tree(path: &Path, diag: &mut DiagnosticCollector) -> bool {
    let generated = crate::plugin_metadata::has_valid_plugin_sentinel(path);
    if generated {
        crate::fs::report_generated_tree(path, diag);
    }
    generated
}

// ---------------------------------------------------------------------------
// Env var preflight
// ---------------------------------------------------------------------------

/// Check that env references name variables present in the current environment.
///
/// In normal mode: emits a warning per missing variable.
/// Under `strict`: returns an error for the first missing variable.
pub fn check_env_refs(
    items: &[ParsedMcpItem],
    strict: bool,
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    for item in items {
        let refs: Vec<(&str, &str)> = match &item.def.transport {
            McpTransport::Stdio { env, .. } => env
                .iter()
                .map(|(key, env_ref)| (key.as_str(), env_ref.var_name()))
                .collect(),
            McpTransport::Http { headers, .. } => headers
                .iter()
                .filter_map(|(key, value)| match value {
                    HeaderValue::Env { env, .. } => Some((key.as_str(), env.var_name())),
                    HeaderValue::Literal(_) => None,
                })
                .collect(),
        };
        for (key, var_name) in refs {
            if std::env::var(var_name).is_err() {
                let msg = format!(
                    "MCP server `{}` (from `{}`): env var `{var_name}` (referenced by `{key}`) \
                     is not set — the server may fail at runtime",
                    item.name, item.source_name
                );
                if strict {
                    return Err(MarsError::Config(ConfigError::Invalid { message: msg }));
                }
                diag.warn("mcp-env-missing", msg);
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Target lowering
// ---------------------------------------------------------------------------

fn substitute_mcp_dir(text: &str, mcp_dir: &str) -> String {
    text.replace(MCP_DIR_TOKEN, mcp_dir)
}

fn def_uses_dir_token(def: &McpServerDef) -> bool {
    match &def.transport {
        McpTransport::Stdio { command, args, .. } => {
            command.contains(MCP_DIR_TOKEN) || args.iter().any(|arg| arg.contains(MCP_DIR_TOKEN))
        }
        McpTransport::Http { .. } => false,
    }
}

/// The transport a target writes for a parsed item, with `${MARS_MCP_DIR}`
/// resolved to the directory this target installs the server's files in.
/// Env stays symbolic: adapters write each harness's own reference syntax.
pub fn lower_transport(item: &ParsedMcpItem, mcp_dir: &str) -> McpTransport {
    match &item.def.transport {
        McpTransport::Stdio { command, args, env } => McpTransport::Stdio {
            command: substitute_mcp_dir(command, mcp_dir),
            args: args
                .iter()
                .map(|arg| substitute_mcp_dir(arg, mcp_dir))
                .collect(),
            env: env.clone(),
        },
        http @ McpTransport::Http { .. } => http.clone(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_mcp_toml_dir(dir: &Path, server_name: &str, toml: &str) {
        let server_dir = dir.join("mcp").join(server_name);
        std::fs::create_dir_all(&server_dir).unwrap();
        std::fs::write(server_dir.join("mcp.toml"), toml).unwrap();
    }

    #[test]
    fn discover_finds_mcp_items() {
        let tmp = TempDir::new().unwrap();
        make_mcp_toml_dir(
            tmp.path(),
            "context7",
            r#"
command = "npx"
args = ["-y", "@upstash/context7-mcp@latest"]
"#,
        );

        let items =
            discover_mcp_items(tmp.path(), "base", 1, 0, &mut DiagnosticCollector::new()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "context7");
        let McpTransport::Stdio { command, args, .. } = &items[0].def.transport else {
            panic!("stdio server expected");
        };
        assert_eq!(command, "npx");
        assert_eq!(args, &["-y", "@upstash/context7-mcp@latest"]);
    }

    #[test]
    fn discover_empty_when_no_mcp_dir() {
        let tmp = TempDir::new().unwrap();
        let items =
            discover_mcp_items(tmp.path(), "base", 1, 0, &mut DiagnosticCollector::new()).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn discover_skips_dir_without_mcp_toml() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("mcp/no-toml")).unwrap();
        let items =
            discover_mcp_items(tmp.path(), "base", 1, 0, &mut DiagnosticCollector::new()).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn discover_respects_name_override() {
        let tmp = TempDir::new().unwrap();
        make_mcp_toml_dir(
            tmp.path(),
            "dir-name",
            r#"
name = "custom-name"
command = "node"
"#,
        );
        let items =
            discover_mcp_items(tmp.path(), "base", 1, 0, &mut DiagnosticCollector::new()).unwrap();
        assert_eq!(items[0].name, "custom-name");
    }

    #[test]
    fn discover_parses_env_refs() {
        let tmp = TempDir::new().unwrap();
        make_mcp_toml_dir(
            tmp.path(),
            "api-server",
            r#"
command = "npx"
[env]
API_KEY = { from = "env", var = "MY_API_KEY" }
"#,
        );
        let items =
            discover_mcp_items(tmp.path(), "base", 1, 0, &mut DiagnosticCollector::new()).unwrap();
        let McpTransport::Stdio { env, .. } = &items[0].def.transport else {
            panic!("stdio server expected");
        };
        assert_eq!(env.len(), 1);
        let env_ref = &env["API_KEY"];
        assert_eq!(env_ref.var_name(), "MY_API_KEY");
    }

    #[test]
    fn check_env_refs_warns_when_missing() {
        let tmp = TempDir::new().unwrap();
        make_mcp_toml_dir(
            tmp.path(),
            "server",
            r#"
command = "npx"
[env]
KEY = { from = "env", var = "MARS_TEST_DEFINITELY_NOT_SET_XYZ123" }
"#,
        );
        let items =
            discover_mcp_items(tmp.path(), "base", 1, 0, &mut DiagnosticCollector::new()).unwrap();
        let mut diag = DiagnosticCollector::new();
        check_env_refs(&items, false, &mut diag).unwrap();
        let collected = diag.drain();
        assert_eq!(collected.len(), 1);
        assert!(
            collected[0]
                .message
                .contains("MARS_TEST_DEFINITELY_NOT_SET_XYZ123")
        );
    }

    #[test]
    fn check_env_refs_strict_errors_when_missing() {
        let tmp = TempDir::new().unwrap();
        make_mcp_toml_dir(
            tmp.path(),
            "server",
            r#"
command = "npx"
[env]
KEY = { from = "env", var = "MARS_TEST_DEFINITELY_NOT_SET_XYZ456" }
"#,
        );
        let items =
            discover_mcp_items(tmp.path(), "base", 1, 0, &mut DiagnosticCollector::new()).unwrap();
        let mut diag = DiagnosticCollector::new();
        let result = check_env_refs(&items, true, &mut diag);
        assert!(result.is_err());
    }

    #[test]
    fn env_ref_preserves_symbolic_var_name() {
        let tmp = TempDir::new().unwrap();
        make_mcp_toml_dir(
            tmp.path(),
            "server",
            r#"
command = "npx"
[env]
TOKEN = { from = "env", var = "SECRET_TOKEN" }
"#,
        );
        let items =
            discover_mcp_items(tmp.path(), "base", 1, 0, &mut DiagnosticCollector::new()).unwrap();
        let McpTransport::Stdio { env, .. } = lower_transport(&items[0], "/installed/mcp/server")
        else {
            panic!("stdio server expected");
        };
        // The env map carries the variable name, not the resolved value.
        assert_eq!(env["TOKEN"].var_name(), "SECRET_TOKEN");
    }
}
