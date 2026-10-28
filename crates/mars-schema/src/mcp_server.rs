//! The authored MCP server definition: the vocabulary and the pure parse of one
//! `mcp/<name>/mcp.toml`.
//!
//! [`crate::mcp`] is the other MCP module here and is unrelated: it parses the
//! `mcp(server/tool)` references in tool lists. This one owns what a server
//! *is*. Discovery, `${MARS_MCP_DIR}` substitution, env preflight and target
//! rendering stay in the binary, which consumes these types.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// The authored placeholder for the directory a stdio server's files are
/// installed in. A remote server has no such directory, so [`parse_mcp_toml`]
/// rejects it there.
pub const MCP_DIR_TOKEN: &str = "${MARS_MCP_DIR}";

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
    /// `prefix` followed by the value of the referenced environment variable,
    /// read by the harness at runtime. The prefix is literal and may be empty.
    Env {
        env: EnvRef,
        prefix: String,
    },
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

/// A parsed `mcp.toml` and the fields it set that its transport does not use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMcpToml {
    pub def: McpServerDef,
    /// `headers` on a `command` server; `args` or `env` on a `url` server.
    /// Ignored, like an unknown key, so a stray field cannot drop a server.
    pub ignored_fields: Vec<&'static str>,
}

#[derive(Deserialize)]
struct RawServer {
    #[serde(default)]
    name: Option<String>,
    command: Option<String>,
    url: Option<String>,
    args: Option<Vec<String>>,
    env: Option<IndexMap<String, EnvRef>>,
    headers: Option<IndexMap<String, toml::Value>>,
    #[serde(default = "default_visibility")]
    visibility: String,
    #[serde(default)]
    targets: Vec<String>,
}

pub fn default_visibility() -> String {
    "local".to_string()
}

/// Why an `mcp.toml` did not parse. The caller prefixes the file it came from.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct McpParseError(String);

const HEADER_SHAPE: &str = "expected a string or `{ from = \"env\", var, prefix? }`";

fn parse_header(name: &str, value: toml::Value) -> Result<HeaderValue, McpParseError> {
    let fail = |detail: String| McpParseError(format!("header `{name}`: {detail}"));
    let mut table = match value {
        toml::Value::String(text) => return Ok(HeaderValue::Literal(text)),
        toml::Value::Table(table) => table,
        _ => return Err(fail(HEADER_SHAPE.to_string())),
    };
    if let Some(key) = table
        .keys()
        .find(|key| !matches!(key.as_str(), "from" | "var" | "prefix"))
    {
        return Err(fail(format!("unknown key `{key}`; {HEADER_SHAPE}")));
    }
    let prefix = match table.remove("prefix") {
        None => String::new(),
        Some(toml::Value::String(prefix)) => prefix,
        Some(_) => return Err(fail("`prefix` must be a string".to_string())),
    };
    let env = toml::Value::Table(table)
        .try_into::<EnvRef>()
        .map_err(|error| fail(format!("{}; {HEADER_SHAPE}", error.message())))?;
    Ok(HeaderValue::Env { env, prefix })
}

fn http_strings<'a>(
    url: &'a str,
    headers: &'a IndexMap<String, HeaderValue>,
) -> impl Iterator<Item = &'a str> {
    std::iter::once(url).chain(headers.values().map(|value| match value {
        HeaderValue::Literal(text) | HeaderValue::Env { prefix: text, .. } => text.as_str(),
    }))
}

/// Parse the text of one `mcp.toml`. A server sets exactly one of `command`
/// (stdio) or `url` (remote).
pub fn parse_mcp_toml(text: &str) -> Result<ParsedMcpToml, McpParseError> {
    let raw: RawServer = toml::from_str(text).map_err(|error| McpParseError(error.to_string()))?;
    let fail = |message: &str| Err(McpParseError(message.to_string()));
    let mut ignored_fields = Vec::new();
    let transport = match (raw.command, raw.url) {
        (Some(command), None) => {
            if raw.headers.is_some() {
                ignored_fields.push("headers");
            }
            McpTransport::Stdio {
                command,
                args: raw.args.unwrap_or_default(),
                env: raw.env.unwrap_or_default(),
            }
        }
        (None, Some(url)) => {
            if raw.args.is_some() {
                ignored_fields.push("args");
            }
            if raw.env.is_some() {
                ignored_fields.push("env");
            }
            let headers = raw
                .headers
                .unwrap_or_default()
                .into_iter()
                .map(|(name, value)| -> Result<_, McpParseError> {
                    Ok((name.clone(), parse_header(&name, value)?))
                })
                .collect::<Result<IndexMap<_, _>, _>>()?;
            if http_strings(&url, &headers).any(|text| text.contains(MCP_DIR_TOKEN)) {
                return fail(&format!(
                    "{MCP_DIR_TOKEN} applies only to a server with `command`; a remote server has no directory"
                ));
            }
            McpTransport::Http { url, headers }
        }
        (Some(_), Some(_)) => {
            return fail("set exactly one of `command` or `url`, found both");
        }
        (None, None) => {
            return fail("set exactly one of `command` or `url`, found neither");
        }
    };
    Ok(ParsedMcpToml {
        def: McpServerDef {
            name: raw.name,
            transport,
            visibility: raw.visibility,
            targets: raw.targets,
        },
        ignored_fields,
    })
}

/// The `mcp.toml` text for `def`: what [`parse_mcp_toml`] reads back as `def`. The one renderer,
/// for the import's staged files and the bytes `normalize` reports.
pub fn render_mcp_toml(def: &McpServerDef) -> String {
    use toml_edit::{DocumentMut, InlineTable, Value, value};

    let env_ref = |var: &str, prefix: &str| {
        let mut table = InlineTable::new();
        table.insert("from", "env".into());
        table.insert("var", var.into());
        if !prefix.is_empty() {
            table.insert("prefix", prefix.into());
        }
        Value::InlineTable(table)
    };
    let mut doc = DocumentMut::new();
    if let Some(name) = &def.name {
        doc["name"] = value(name.as_str());
    }
    doc["visibility"] = value(def.visibility.as_str());
    if !def.targets.is_empty() {
        doc["targets"] = value(
            def.targets
                .iter()
                .map(String::as_str)
                .collect::<toml_edit::Array>(),
        );
    }
    match &def.transport {
        McpTransport::Stdio { command, args, env } => {
            doc["command"] = value(command.as_str());
            if !args.is_empty() {
                doc["args"] = value(
                    args.iter()
                        .map(String::as_str)
                        .collect::<toml_edit::Array>(),
                );
            }
            if !env.is_empty() {
                let mut table = InlineTable::new();
                for (name, reference) in env {
                    table.insert(name, env_ref(reference.var_name(), ""));
                }
                doc["env"] = value(table);
            }
        }
        McpTransport::Http { url, headers } => {
            doc["url"] = value(url.as_str());
            if !headers.is_empty() {
                let mut table = InlineTable::new();
                for (name, header) in headers {
                    table.insert(
                        name,
                        match header {
                            HeaderValue::Literal(text) => text.as_str().into(),
                            HeaderValue::Env { env, prefix } => env_ref(env.var_name(), prefix),
                        },
                    );
                }
                doc["headers"] = value(table);
            }
        }
    }
    doc.to_string()
}
