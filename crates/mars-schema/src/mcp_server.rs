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

/// How a server is reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpTransport {
    /// A local process the harness launches and talks to over stdio.
    Stdio {
        /// Command to launch the MCP server.
        command: String,
        /// Arguments to pass to the command.
        args: Vec<String>,
        /// Symbolic environment references, by the variable name the server reads.
        env: IndexMap<String, EnvRef>,
    },
    /// A remote server over Streamable HTTP. There is no SSE: no harness Mars
    /// targets takes it uniformly, so the vocabulary cannot ask for it.
    Http {
        url: String,
        /// Request headers, by header name.
        headers: IndexMap<String, HeaderValue>,
    },
}

/// A request header's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderValue {
    Literal(String),
    /// `prefix` followed by the value of environment variable `var`, read by
    /// the harness at runtime. The prefix is literal and may be empty.
    Env { var: String, prefix: String },
}

/// Parsed content of a single `mcp/<name>/mcp.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerDef {
    /// Overrides the server's directory name as its config key.
    pub name: Option<String>,
    pub transport: McpTransport,
    /// `local` or `exported`, as authored; the binary interprets it.
    pub visibility: String,
    /// Optional target filter — if empty, applies to all targets.
    pub targets: Vec<String>,
}

#[derive(Deserialize)]
struct RawServer {
    #[serde(default)]
    name: Option<String>,
    command: Option<String>,
    url: Option<String>,
    args: Option<Vec<String>>,
    env: Option<IndexMap<String, EnvRef>>,
    headers: Option<IndexMap<String, RawHeader>>,
    #[serde(default = "default_visibility")]
    visibility: String,
    #[serde(default)]
    targets: Vec<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawHeader {
    Literal(String),
    Env(RawEnvHeader),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEnvHeader {
    #[serde(rename = "from")]
    _from: EnvKind,
    var: String,
    #[serde(default)]
    prefix: String,
}

#[derive(Deserialize)]
enum EnvKind {
    #[serde(rename = "env")]
    Env,
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

/// Parse the text of one `mcp.toml`. A server sets exactly one of `command`
/// (stdio) or `url` (remote).
pub fn parse_mcp_toml(text: &str) -> Result<McpServerDef, McpParseError> {
    let raw: RawServer =
        toml::from_str(text).map_err(|error| McpParseError(error.to_string()))?;
    let fail = |message: &str| Err(McpParseError(message.to_string()));
    let transport = match (raw.command, raw.url) {
        (Some(command), None) => {
            if raw.headers.is_some() {
                return fail("`headers` applies only to a server with `url`");
            }
            McpTransport::Stdio {
                command,
                args: raw.args.unwrap_or_default(),
                env: raw.env.unwrap_or_default(),
            }
        }
        (None, Some(url)) => {
            if raw.args.is_some() || raw.env.is_some() {
                return fail("`args` and `env` apply only to a server with `command`");
            }
            let headers = raw
                .headers
                .unwrap_or_default()
                .into_iter()
                .map(|(name, value)| {
                    let value = match value {
                        RawHeader::Literal(text) => HeaderValue::Literal(text),
                        RawHeader::Env(env) => HeaderValue::Env {
                            var: env.var,
                            prefix: env.prefix,
                        },
                    };
                    (name, value)
                })
                .collect();
            McpTransport::Http { url, headers }
        }
        (Some(_), Some(_)) => {
            return fail("set exactly one of `command` or `url`, found both");
        }
        (None, None) => {
            return fail("set exactly one of `command` or `url`, found neither");
        }
    };
    Ok(McpServerDef {
        name: raw.name,
        transport,
        visibility: raw.visibility,
        targets: raw.targets,
    })
}
