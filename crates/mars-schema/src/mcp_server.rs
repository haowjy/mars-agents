//! The authored MCP server definition: the vocabulary and the pure parse of one
//! `mcp/<name>/mcp.toml`.
//!
//! [`crate::mcp`] is the other MCP module here and is unrelated: it parses the
//! `mcp(server/tool)` references in tool lists. This one owns what a server
//! *is*. Discovery, `${MARS_MCP_DIR}` substitution, env preflight and target
//! rendering stay in the binary, which consumes these types.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// A symbolic environment reference.
///
/// `from = "env"` is the only supported kind. The value is never resolved: it
/// flows through as a reference for the harness to read at runtime.
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
    /// The environment variable name, for rendering and preflight checks.
    pub fn var_name(&self) -> &str {
        match self {
            EnvRef::Env { var } => var.as_str(),
        }
    }
}

/// Parsed content of a single `mcp/<name>/mcp.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct McpServerDef {
    /// Overrides the server's directory name as its config key.
    #[serde(default)]
    pub name: Option<String>,
    /// Command to launch the MCP server.
    pub command: String,
    /// Arguments to pass to the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Symbolic environment references.
    #[serde(default)]
    pub env: IndexMap<String, EnvRef>,
    /// `local` or `exported`, as authored; the binary interprets it.
    #[serde(default = "default_visibility")]
    pub visibility: String,
    /// Optional target filter — if empty, applies to all targets.
    #[serde(default)]
    pub targets: Vec<String>,
}

fn default_visibility() -> String {
    "local".to_string()
}

/// Why an `mcp.toml` did not parse. The caller prefixes the file it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpParseError(String);

impl std::fmt::Display for McpParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for McpParseError {}

/// Parse the text of one `mcp.toml`.
pub fn parse_mcp_toml(text: &str) -> Result<McpServerDef, McpParseError> {
    toml::from_str(text).map_err(|error| McpParseError(error.to_string()))
}
