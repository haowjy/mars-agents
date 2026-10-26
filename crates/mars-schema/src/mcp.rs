//! Canonical MCP tool references in `tools:` / `disallowed-tools:` lists.
//!
//! Mars expresses per-tool MCP gating with a scoped head `mcp(...)` where the inner
//! payload names a server and optional tool, `/`-separated. Server and tool segments
//! are preserved **verbatim** — MCP tool names are case-sensitive on every harness and
//! must not be normalized during parse or projection.
//!
//! Foreign harness tokens (Claude `mcp__…`, Cursor `Mcp(server:tool)`) are parsed by
//! [`parse_foreign_mcp_token`] during inbound lift and converted to canonical `mcp(...)`.
//!
//! `*` is the only wildcard:
//! - `mcp(server)` — whole server (equivalent to `mcp(server/*)`)
//! - `mcp(server/tool)` — one specific tool
//! - `mcp(server/*)` — all tools on one server
//! - `mcp(*/tool)` — a tool name across any server
//! - `mcp(*/*)` — all MCP tools

/// Human-readable grammar for valid `mcp(...)` tool-list entries (used in validation errors).
pub(crate) const MCP_TOOL_NAME_GRAMMAR: &str = "valid mcp(server), mcp(server/tool), mcp(server/*), mcp(*/tool), or mcp(*/*) reference (* is the only wildcard; segments non-empty)";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpSegment {
    Any,
    Named(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpRef {
    pub server: McpSegment,
    pub tool: McpSegment,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpRefParseError {
    Empty,
    EmptyServerSegment,
    EmptyToolSegment,
    TooManySegments,
    WhitespaceOnly,
}

impl std::fmt::Display for McpRefParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl McpRefParseError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::Empty => "MCP reference payload is empty",
            Self::EmptyServerSegment => "MCP reference has an empty server segment",
            Self::EmptyToolSegment => "MCP reference has an empty tool segment",
            Self::TooManySegments => "MCP reference must have at most one '/' separator",
            Self::WhitespaceOnly => "MCP reference payload is whitespace only",
        }
    }
}

impl std::fmt::Display for McpRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_canonical())
    }
}

impl McpRef {
    /// Canonical `mcp(server/tool)` spelling. Shorthand `mcp(server)` normalizes to `mcp(server/*)`.
    pub fn to_canonical(&self) -> String {
        format!("mcp({})", self.to_pattern())
    }

    /// The `server/tool` payload inside `mcp(...)`, as carried by an MCP tool rule.
    pub(crate) fn to_pattern(&self) -> String {
        format!(
            "{}/{}",
            segment_display(&self.server),
            segment_display(&self.tool)
        )
    }
}

/// Parse a foreign harness MCP permission token into an [`McpRef`].
///
/// Returns `None` for non-MCP tool tokens (including already-canonical `mcp(...)` entries).
///
/// ## Claude wire form (`mcp__` prefix)
///
/// The first `__` after `mcp__` separates server from tool when a tool segment is present;
/// if there is no further `__`, the remainder names a whole server. Segments are preserved
/// verbatim (no case change).
///
/// | Token | `McpRef` |
/// |---|---|
/// | `mcp__server__tool` | `Named(server)`, `Named(tool)` |
/// | `mcp__server__*` | `Named(server)`, `Any` |
/// | `mcp__server` | `Named(server)`, `Any` |
/// | `mcp__*` | `Any`, `Any` |
///
/// **Limitation:** server names cannot contain `__` reliably — the first `__` after `mcp__`
/// is always treated as the server/tool boundary.
///
/// ## Cursor form (`Mcp(server:tool)`)
///
/// Colon-separated payload inside `Mcp(...)`. Only attempted when `dialect` is Cursor.
pub fn parse_foreign_mcp_token(raw: &str, dialect: ForeignMcpDialect) -> Option<McpRef> {
    match dialect {
        ForeignMcpDialect::Claude => parse_claude_mcp_wire_token(raw.trim()),
        ForeignMcpDialect::Cursor => parse_cursor_mcp_token(raw.trim()),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForeignMcpDialect {
    Claude,
    Cursor,
}

fn parse_claude_mcp_wire_token(raw: &str) -> Option<McpRef> {
    const PREFIX: &str = "mcp__";
    let remainder = match raw.get(..PREFIX.len()) {
        Some(p) if p.eq_ignore_ascii_case(PREFIX) => &raw[PREFIX.len()..],
        _ => return None,
    };
    if remainder.is_empty() {
        return None;
    }

    match remainder.split_once("__") {
        None => {
            let server = parse_segment(remainder).ok()?;
            Some(McpRef {
                server,
                tool: McpSegment::Any,
            })
        }
        Some((server_part, tool_part)) => {
            let server = parse_segment(server_part).ok()?;
            let tool = parse_segment(tool_part).ok()?;
            Some(McpRef { server, tool })
        }
    }
}

fn parse_cursor_mcp_token(raw: &str) -> Option<McpRef> {
    let open = raw.find('(')?;
    let head = raw[..open].trim();
    if !head.eq_ignore_ascii_case("mcp") {
        return None;
    }
    let inner = extract_scoped_payload(&raw[open..])?;
    // Tool names are simple (no `:`); server ids may contain `:` (e.g. `plugin:context7:context7`).
    let (server_part, tool_part) = inner.rsplit_once(':')?;
    let server = parse_segment(server_part).ok()?;
    let tool = parse_segment(tool_part).ok()?;
    Some(McpRef { server, tool })
}

fn parse_segment(segment: &str) -> Result<McpSegment, McpRefParseError> {
    let trimmed = segment.trim();
    if trimmed.is_empty() {
        return Err(McpRefParseError::Empty);
    }
    if trimmed == "*" {
        Ok(McpSegment::Any)
    } else {
        Ok(McpSegment::Named(trimmed.to_string()))
    }
}

/// Parse a full `mcp(...)` tool-list entry into an [`McpRef`].
pub fn try_parse_mcp_tool_name(raw: &str) -> Option<McpRef> {
    let trimmed = raw.trim();
    let open = trimmed.find('(')?;
    let head = trimmed[..open].trim();
    if !head.eq_ignore_ascii_case("mcp") {
        return None;
    }
    let payload = &trimmed[open..];
    let inner = extract_scoped_payload(payload)?;
    parse_mcp_ref(inner).ok()
}

fn extract_scoped_payload(payload: &str) -> Option<&str> {
    let trimmed = payload.trim();
    if trimmed.len() < 2 || !trimmed.starts_with('(') || !trimmed.ends_with(')') {
        return None;
    }
    Some(&trimmed[1..trimmed.len() - 1])
}

fn segment_display(segment: &McpSegment) -> String {
    match segment {
        McpSegment::Any => "*".to_string(),
        McpSegment::Named(name) => name.clone(),
    }
}

/// Parse the inner payload of `mcp(...)`, without the surrounding parentheses.
pub fn parse_mcp_ref(payload: &str) -> Result<McpRef, McpRefParseError> {
    let trimmed = payload.trim();
    if trimmed.is_empty() {
        return Err(McpRefParseError::WhitespaceOnly);
    }

    match trimmed.split_once('/') {
        None => {
            let server = parse_segment(trimmed)?;
            Ok(McpRef {
                server,
                tool: McpSegment::Any,
            })
        }
        Some((server_part, tool_part)) => {
            if trimmed.matches('/').count() > 1 {
                return Err(McpRefParseError::TooManySegments);
            }
            let server = parse_segment(server_part).map_err(|err| match err {
                McpRefParseError::Empty => McpRefParseError::EmptyServerSegment,
                other => other,
            })?;
            let tool = parse_segment(tool_part).map_err(|err| match err {
                McpRefParseError::Empty => McpRefParseError::EmptyToolSegment,
                other => other,
            })?;
            Ok(McpRef { server, tool })
        }
    }
}
