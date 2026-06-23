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
}
