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
/// - Produce `MarsTargetMcpEntry` per target for adapter config writing
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::diagnostic::DiagnosticCollector;
use crate::error::{ConfigError, MarsError};

// ---------------------------------------------------------------------------
// Schema types
// ---------------------------------------------------------------------------

/// A symbolic environment reference.
///
/// `from = "env"` is the only supported kind in V0.
/// The value is never resolved — it flows through as a reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from")]
pub enum EnvRef {
    /// Read from the process environment at the harness's runtime.
    #[serde(rename = "env")]
    Env {
        /// Name of the environment variable.
        var: String,
    },
}

impl EnvRef {
    /// Return the environment variable name for preflight checking.
    pub fn var_name(&self) -> &str {
        match self {
            EnvRef::Env { var } => var.as_str(),
        }
    }
}

/// MCP transport type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    /// Local stdio subprocess transport.
    #[default]
    Stdio,
    /// Remote HTTP transport.
    Http,
}

/// A header value — either an env ref or a plain string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HeaderValue {
    /// Symbolic environment reference.
    EnvRef(EnvRef),
    /// Plain header literal.
    Plain(String),
}

/// Parsed content of a single `mcp/<name>/mcp.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerDef {
    /// Server name — matches the directory name by convention but can be
    /// overridden in the TOML file.
    #[serde(default)]
    pub name: Option<String>,
    /// Transport type (`stdio` default, `http` optional).
    #[serde(default)]
    pub r#type: McpTransport,
    /// Command to launch the MCP server (required for stdio, forbidden for http).
    pub command: Option<String>,
    /// Arguments to pass to the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// HTTP endpoint URL (required for http, forbidden for stdio).
    pub url: Option<String>,
    /// Optional HTTP headers for http transport (forbidden for stdio).
    #[serde(default)]
    pub headers: indexmap::IndexMap<String, HeaderValue>,
    /// Symbolic environment references.
    #[serde(default)]
    pub env: indexmap::IndexMap<String, EnvRef>,
    /// Visibility: "local" (default) or "exported".
    /// Exported MCP servers propagate to transitive consumers.
    #[serde(default = "default_visibility")]
    pub visibility: String,
    /// Optional target filter — if absent, applies to all targets.
    #[serde(default)]
    pub targets: Vec<String>,
}

impl McpServerDef {
    fn validate(&self, source_path: &Path) -> Result<(), MarsError> {
        let transport = match self.r#type {
            McpTransport::Stdio => "stdio",
            McpTransport::Http => "http",
        };

        let invalid = |message: String| {
            MarsError::Config(ConfigError::Invalid {
                message: format!(
                    "invalid MCP server in {} ({transport}): {message}",
                    source_path.display()
                ),
            })
        };

        match self.r#type {
            McpTransport::Stdio => {
                if self
                    .command
                    .as_ref()
                    .map(|cmd| cmd.trim().is_empty())
                    .unwrap_or(true)
                {
                    return Err(invalid(
                        "`command` is required and must be non-empty for stdio transport"
                            .to_string(),
                    ));
                }
                if self.url.is_some() {
                    return Err(invalid(
                        "`url` is only allowed for http transport".to_string(),
                    ));
                }
                if !self.headers.is_empty() {
                    return Err(invalid(
                        "`headers` is only allowed for http transport".to_string(),
                    ));
                }
            }
            McpTransport::Http => {
                if self
                    .url
                    .as_ref()
                    .map(|url| url.trim().is_empty())
                    .unwrap_or(true)
                {
                    return Err(invalid(
                        "`url` is required and must be non-empty for http transport".to_string(),
                    ));
                }
                if self.command.is_some() {
                    return Err(invalid(
                        "`command` is forbidden for http transport".to_string(),
                    ));
                }
                if !self.args.is_empty() {
                    return Err(invalid(
                        "`args` is forbidden for http transport".to_string(),
                    ));
                }
            }
        }

        Ok(())
    }
}

fn default_visibility() -> String {
    "local".to_string()
}

/// A discovered MCP server item with provenance.
#[derive(Debug, Clone)]
pub struct ParsedMcpItem {
    /// Resolved server name (directory name, unless overridden in TOML).
    pub name: String,
    /// Parsed definition.
    pub def: McpServerDef,
    /// Source package name this item came from.
    pub source_name: String,
    /// Declaration order of the source package in the consumer graph.
    pub decl_order: usize,
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// Discover MCP server items from a package root.
///
/// Scans `<package_root>/mcp/<name>/mcp.toml` for each subdirectory.
/// Returns the parsed items in directory-sorted order.
pub fn discover_mcp_items(
    package_root: &Path,
    source_name: &str,
    decl_order: usize,
) -> Result<Vec<ParsedMcpItem>, MarsError> {
    let mcp_dir = package_root.join("mcp");
    if !mcp_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut items = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(&mcp_dir)
        .map_err(MarsError::from)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
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

        let raw = std::fs::read_to_string(&toml_path).map_err(MarsError::from)?;
        let def: McpServerDef = toml::from_str(&raw).map_err(|e| {
            MarsError::Config(ConfigError::Invalid {
                message: format!("failed to parse {}: {e}", toml_path.display()),
            })
        })?;
        def.validate(&toml_path)?;

        // Resolved name: TOML override wins, else directory name.
        let resolved_name = def.name.as_deref().unwrap_or(&server_name).to_string();

        items.push(ParsedMcpItem {
            name: resolved_name,
            def,
            source_name: source_name.to_string(),
            decl_order,
        });
    }

    Ok(items)
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
        for (key, env_ref) in &item.def.env {
            let var_name = env_ref.var_name();
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
        for (header_key, header_value) in &item.def.headers {
            let HeaderValue::EnvRef(env_ref) = header_value else {
                continue;
            };
            let var_name = env_ref.var_name();
            if std::env::var(var_name).is_err() {
                let msg = format!(
                    "MCP server `{}` (from `{}`): env var `{var_name}` (referenced by header `{header_key}`) \
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

/// A fully lowered MCP server entry ready for a target adapter to write.
#[derive(Debug, Clone)]
pub struct TargetMcpEntry {
    /// Server name as it appears in the target config.
    pub name: String,
    /// Transport kind.
    pub transport: McpTransport,
    /// Launch command (stdio only).
    pub command: Option<String>,
    /// Launch arguments.
    pub args: Vec<String>,
    /// Env vars: key → variable name (symbolic — adapters write the native form).
    pub env: indexmap::IndexMap<String, String>,
    /// Remote URL (http only).
    pub url: Option<String>,
    /// Header values (http only).
    pub headers: indexmap::IndexMap<String, HeaderValue>,
}

impl TargetMcpEntry {
    /// Build from a parsed item.
    pub fn from_parsed(item: &ParsedMcpItem) -> Self {
        let env = item
            .def
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.var_name().to_string()))
            .collect();
        Self {
            name: item.name.clone(),
            transport: item.def.r#type.clone(),
            command: item.def.command.clone(),
            args: item.def.args.clone(),
            env,
            url: item.def.url.clone(),
            headers: item.def.headers.clone(),
        }
    }
}

/// Lower all MCP items for a specific target root.
///
/// Filters to items that apply to the given target (empty target list = all targets).
#[cfg(test)]
pub fn lower_for_target<'a>(items: &'a [ParsedMcpItem], target_root: &str) -> Vec<TargetMcpEntry> {
    let mut applicable: Vec<(usize, &'a ParsedMcpItem)> = items
        .iter()
        .enumerate()
        .filter(|item| {
            item.1.def.targets.is_empty() || item.1.def.targets.iter().any(|t| t == target_root)
        })
        .collect();
    applicable.sort_by_key(|(original_index, item)| (item.decl_order, *original_index));
    applicable
        .into_iter()
        .map(|(_, item)| TargetMcpEntry::from_parsed(item))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    // qa-validated: mars-tools-abstraction
    use super::*;
    use tempfile::TempDir;

    fn make_mcp_toml_dir(dir: &Path, server_name: &str, toml: &str) {
        let server_dir = dir.join("mcp").join(server_name);
        std::fs::create_dir_all(&server_dir).unwrap();
        std::fs::write(server_dir.join("mcp.toml"), toml).unwrap();
    }

    #[test]
    fn discover_returns_empty_without_mcp_directory() {
        let tmp = TempDir::new().unwrap();
        let items = discover_mcp_items(tmp.path(), "base", 0).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn discover_parses_stdio_with_name_override_and_env_refs() {
        let tmp = TempDir::new().unwrap();
        make_mcp_toml_dir(
            tmp.path(),
            ".hidden-server",
            r#"
command = "npx"
"#,
        );
        make_mcp_toml_dir(
            tmp.path(),
            "dir-name",
            r#"
name = "custom-name"
command = "node"
args = ["server.js"]
[env]
API_KEY = { from = "env", var = "MY_API_KEY" }
"#,
        );

        let items = discover_mcp_items(tmp.path(), "base", 0).unwrap();
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.name, "custom-name");
        assert_eq!(item.def.r#type, McpTransport::Stdio);
        assert_eq!(item.def.command.as_deref(), Some("node"));
        assert_eq!(item.def.args, ["server.js"]);
        assert_eq!(item.def.env["API_KEY"].var_name(), "MY_API_KEY");
    }

    #[test]
    fn discover_parses_http_url_and_headers() {
        let tmp = TempDir::new().unwrap();
        make_mcp_toml_dir(
            tmp.path(),
            "remote",
            r#"
type = "http"
url = "https://example.com/mcp"
[headers]
Authorization = { from = "env", var = "API_TOKEN" }
X-Custom = "literal"
"#,
        );
        let items = discover_mcp_items(tmp.path(), "base", 0).unwrap();
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.def.r#type, McpTransport::Http);
        assert_eq!(item.def.url.as_deref(), Some("https://example.com/mcp"));
        assert!(matches!(
            item.def.headers.get("Authorization"),
            Some(HeaderValue::EnvRef(EnvRef::Env { var })) if var == "API_TOKEN"
        ));
        assert!(matches!(
            item.def.headers.get("X-Custom"),
            Some(HeaderValue::Plain(v)) if v == "literal"
        ));
    }

    #[test]
    fn discover_rejects_invalid_transport_field_combinations() {
        let cases = [
            ("missing-url", r#"type = "http""#),
            (
                "http-with-command",
                r#"type = "http"
url = "https://example.com/mcp"
command = "npx""#,
            ),
            (
                "http-with-args",
                r#"type = "http"
url = "https://example.com/mcp"
args = ["--bad"]"#,
            ),
            (
                "stdio-with-url",
                r#"command = "npx"
url = "https://example.com/mcp""#,
            ),
            (
                "stdio-with-headers",
                r#"command = "npx"
[headers]
X-Test = "value""#,
            ),
            ("stdio-whitespace-command", r#"command = "   ""#),
        ];

        for (name, toml) in cases {
            let tmp = TempDir::new().unwrap();
            make_mcp_toml_dir(tmp.path(), name, toml);
            assert!(
                discover_mcp_items(tmp.path(), "base", 0).is_err(),
                "expected invalid config to fail: {name}"
            );
        }
    }

    #[test]
    fn check_env_refs_warns_in_non_strict_mode_and_errors_in_strict_mode() {
        let tmp = TempDir::new().unwrap();
        make_mcp_toml_dir(
            tmp.path(),
            "server",
            r#"
type = "http"
url = "https://example.com/mcp"
[env]
KEY = { from = "env", var = "MARS_TEST_DEFINITELY_NOT_SET_ENV_XYZ123" }
[headers]
Authorization = { from = "env", var = "MARS_TEST_DEFINITELY_NOT_SET_HEADER_ABC999" }
"#,
        );
        let items = discover_mcp_items(tmp.path(), "base", 0).unwrap();

        let mut non_strict = DiagnosticCollector::new();
        check_env_refs(&items, false, &mut non_strict).unwrap();
        let warnings = non_strict.drain();
        assert_eq!(warnings.len(), 2);
        assert!(warnings.iter().any(|d| d.message.contains("_ENV_XYZ123")));
        assert!(
            warnings
                .iter()
                .any(|d| d.message.contains("_HEADER_ABC999"))
        );

        let mut strict = DiagnosticCollector::new();
        assert!(check_env_refs(&items, true, &mut strict).is_err());
    }

    #[test]
    fn lower_for_target_filters_entries_by_target() {
        let tmp = TempDir::new().unwrap();
        make_mcp_toml_dir(
            tmp.path(),
            "claude-only",
            r#"command = "npx"
targets = [".claude"]"#,
        );
        make_mcp_toml_dir(tmp.path(), "all-targets", r#"command = "node""#);

        let items = discover_mcp_items(tmp.path(), "base", 0).unwrap();

        let claude_entries = lower_for_target(&items, ".claude");
        assert_eq!(claude_entries.len(), 2);

        let codex_entries = lower_for_target(&items, ".codex");
        assert_eq!(codex_entries.len(), 1);
        assert_eq!(codex_entries[0].name, "all-targets");
    }

    #[test]
    fn target_entry_preserves_symbolic_values_for_stdio_and_http() {
        let stdio_tmp = TempDir::new().unwrap();
        make_mcp_toml_dir(
            stdio_tmp.path(),
            "stdio",
            r#"
command = "npx"
[env]
TOKEN = { from = "env", var = "SECRET_TOKEN" }
"#,
        );
        let stdio_items = discover_mcp_items(stdio_tmp.path(), "base", 0).unwrap();
        let stdio_entry = TargetMcpEntry::from_parsed(&stdio_items[0]);
        assert_eq!(stdio_entry.transport, McpTransport::Stdio);
        assert_eq!(stdio_entry.command.as_deref(), Some("npx"));
        assert_eq!(stdio_entry.env["TOKEN"], "SECRET_TOKEN");

        let http_tmp = TempDir::new().unwrap();
        make_mcp_toml_dir(
            http_tmp.path(),
            "remote",
            r#"
type = "http"
url = "https://example.com/mcp"
[headers]
Authorization = { from = "env", var = "API_TOKEN" }
"#,
        );
        let http_items = discover_mcp_items(http_tmp.path(), "base", 0).unwrap();
        let http_entry = TargetMcpEntry::from_parsed(&http_items[0]);
        assert_eq!(http_entry.transport, McpTransport::Http);
        assert_eq!(http_entry.command, None);
        assert_eq!(http_entry.url.as_deref(), Some("https://example.com/mcp"));
        assert!(matches!(
            http_entry.headers.get("Authorization"),
            Some(HeaderValue::EnvRef(EnvRef::Env { var })) if var == "API_TOKEN"
        ));
    }
}
