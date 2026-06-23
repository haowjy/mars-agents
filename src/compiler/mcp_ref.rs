//! Canonical MCP tool references in `tools:` / `disallowed-tools:` lists.
//!
//! Mars expresses per-tool MCP gating with a scoped head `mcp(...)` where the inner
//! payload names a server and optional tool, `/`-separated. Server and tool segments
//! are preserved **verbatim** — MCP tool names are case-sensitive on every harness and
//! must not be normalized during parse or projection.
//!
//! `*` is the only wildcard:
//! - `mcp(server)` — whole server (equivalent to `mcp(server/*)`)
//! - `mcp(server/tool)` — one specific tool
//! - `mcp(server/*)` — all tools on one server
//! - `mcp(*/tool)` — a tool name across any server
//! - `mcp(*/*)` — all MCP tools

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum McpSegment {
    Any,
    Named(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpRef {
    pub server: McpSegment,
    pub tool: McpSegment,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum McpRefParseError {
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
        format!(
            "mcp({}/{})",
            segment_display(&self.server),
            segment_display(&self.tool)
        )
    }

    /// Whole-server ref: named server segment and wildcard (or implicit) tool.
    pub fn is_whole_server(&self) -> bool {
        matches!(&self.tool, McpSegment::Any) && matches!(&self.server, McpSegment::Named(_))
    }
}

/// Parse a full `mcp(...)` tool-list entry into an [`McpRef`].
pub(crate) fn try_parse_mcp_tool_name(raw: &str) -> Option<McpRef> {
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

/// Derive the legacy `mcp-tools:` emission value for an allowed MCP ref.
///
/// Whole-server refs (including `mcp(server)` shorthand) render as the server segment
/// verbatim — matching historical `mcp-tools: [server]` spelling. Per-tool allowed refs
/// render as canonical `mcp(server/tool)` until Phase 4 per-harness projection exists.
pub(crate) fn mcp_ref_to_emission_value(mcp_ref: &McpRef) -> String {
    if mcp_ref.is_whole_server() {
        match &mcp_ref.server {
            McpSegment::Named(server) => server.clone(),
            McpSegment::Any => mcp_ref.to_canonical(),
        }
    } else {
        // Phase 4: real per-harness projection for per-tool allowed MCP refs.
        mcp_ref.to_canonical()
    }
}

fn segment_display(segment: &McpSegment) -> String {
    match segment {
        McpSegment::Any => "*".to_string(),
        McpSegment::Named(name) => name.clone(),
    }
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

/// Parse the inner payload of `mcp(...)`, without the surrounding parentheses.
pub(crate) fn parse_mcp_ref(payload: &str) -> Result<McpRef, McpRefParseError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(payload: &str) -> McpRef {
        parse_mcp_ref(payload).unwrap()
    }

    #[test]
    fn accepts_all_valid_forms_and_round_trips() {
        let cases = [
            ("github", "mcp(github/*)"),
            ("github/create_issue", "mcp(github/create_issue)"),
            ("github/*", "mcp(github/*)"),
            ("*/create_issue", "mcp(*/create_issue)"),
            ("*/*", "mcp(*/*)"),
        ];

        for (payload, expected_canonical) in cases {
            let parsed = parse(payload);
            assert_eq!(
                parsed.to_canonical(),
                expected_canonical,
                "payload: {payload}"
            );
            assert_eq!(parsed.to_string(), expected_canonical, "payload: {payload}");
        }
    }

    #[test]
    fn preserves_verbatim_segment_casing() {
        let parsed = parse("GitHub/CreateIssue");
        assert_eq!(parsed.server, McpSegment::Named("GitHub".to_string()));
        assert_eq!(parsed.tool, McpSegment::Named("CreateIssue".to_string()));
        assert_eq!(parsed.to_canonical(), "mcp(GitHub/CreateIssue)");
    }

    #[test]
    fn rejects_invalid_payloads() {
        let cases = [
            ("", McpRefParseError::WhitespaceOnly),
            ("/x", McpRefParseError::EmptyServerSegment),
            ("x/", McpRefParseError::EmptyToolSegment),
            ("a/b/c", McpRefParseError::TooManySegments),
            ("   ", McpRefParseError::WhitespaceOnly),
        ];

        for (payload, expected) in cases {
            let err = parse_mcp_ref(payload).unwrap_err();
            assert_eq!(err, expected, "payload: {payload:?}");
        }
    }

    #[test]
    fn error_messages_are_clear() {
        assert!(!McpRefParseError::EmptyServerSegment.message().is_empty());
        assert!(!McpRefParseError::TooManySegments.message().is_empty());
    }

    #[test]
    fn try_parse_mcp_tool_name_accepts_scoped_entries() {
        let parsed = try_parse_mcp_tool_name("mcp(context7)").unwrap();
        assert!(parsed.is_whole_server());
        assert_eq!(mcp_ref_to_emission_value(&parsed), "context7");

        let per_tool = try_parse_mcp_tool_name("mcp(github/delete_repo)").unwrap();
        assert!(!per_tool.is_whole_server());
        assert_eq!(
            mcp_ref_to_emission_value(&per_tool),
            "mcp(github/delete_repo)"
        );
    }
}
