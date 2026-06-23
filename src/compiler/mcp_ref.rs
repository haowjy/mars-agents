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
pub(crate) fn parse_foreign_mcp_token(
    raw: &str,
    dialect: crate::dialect::Dialect,
) -> Option<McpRef> {
    let trimmed = raw.trim();

    match dialect {
        crate::dialect::Dialect::Cursor => {
            if let Some(parsed) = parse_cursor_mcp_token(trimmed) {
                return Some(parsed);
            }
        }
        crate::dialect::Dialect::Claude => {
            if let Some(parsed) = parse_claude_mcp_wire_token(trimmed) {
                return Some(parsed);
            }
        }
        // Phase 4+: Codex/OpenCode inbound MCP token forms if they gain tool-list MCP refs.
        crate::dialect::Dialect::Codex
        | crate::dialect::Dialect::OpenCode
        | crate::dialect::Dialect::MarsNative => {}
    }

    // Already-canonical `mcp(...)` entries are not foreign tokens.
    None
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

fn segment_display(segment: &McpSegment) -> String {
    match segment {
        McpSegment::Any => "*".to_string(),
        McpSegment::Named(name) => name.clone(),
    }
}

/// Per-harness native MCP token projection from a canonical [`McpRef`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum McpProjection {
    /// Emit this native token in the harness tool/permission list (or launch bundle).
    Token(String),
    /// Target cannot represent this ref; caller records lossiness and omits it.
    Unsupported(McpUnsupportedReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum McpUnsupportedReason {
    /// Harness cannot scope one tool across all servers (e.g. Claude `mcp(*/tool)`).
    CrossServerTool,
    /// Per-tool MCP gating lives in server config, not the tool list (Codex).
    PerToolNeedsServerConfig,
    /// Harness has no MCP tool surface (Pi).
    HarnessDropsMcp,
}

impl McpUnsupportedReason {
    pub fn message(self) -> &'static str {
        match self {
            Self::CrossServerTool => "Claude cannot scope a single MCP tool across all servers",
            Self::PerToolNeedsServerConfig => {
                "Codex per-tool MCP gating lives in server config, not the tool list"
            }
            Self::HarnessDropsMcp => "Harness has no MCP tool surface",
        }
    }
}

/// Project a canonical MCP ref to a harness-native permission token.
///
/// `harness` is a lowercase id (`claude`, `codex`, `cursor`, `opencode`, `pi`).
/// Server and tool segments are preserved verbatim — never re-cased.
///
/// Unknown harness ids passthrough canonical `mcp(server/tool)` rather than inventing
/// a native wire form that might be wrong for that target.
pub(crate) fn project_mcp_ref(r: &McpRef, harness: &str) -> McpProjection {
    match harness.trim().to_ascii_lowercase().as_str() {
        "claude" => project_claude(r),
        "cursor" => project_cursor(r),
        "opencode" => project_opencode(r),
        "codex" => McpProjection::Unsupported(McpUnsupportedReason::PerToolNeedsServerConfig),
        "pi" => McpProjection::Unsupported(McpUnsupportedReason::HarnessDropsMcp),
        _ => McpProjection::Token(r.to_canonical()),
    }
}

fn project_claude(r: &McpRef) -> McpProjection {
    match (&r.server, &r.tool) {
        (McpSegment::Any, McpSegment::Named(_)) => {
            McpProjection::Unsupported(McpUnsupportedReason::CrossServerTool)
        }
        (McpSegment::Any, McpSegment::Any) => McpProjection::Token("mcp__*".to_string()),
        (McpSegment::Named(server), tool) => {
            let tool_seg = segment_display(tool);
            McpProjection::Token(format!("mcp__{server}__{tool_seg}"))
        }
    }
}

fn project_cursor(r: &McpRef) -> McpProjection {
    McpProjection::Token(format!(
        "Mcp({}:{})",
        segment_display(&r.server),
        segment_display(&r.tool)
    ))
}

fn project_opencode(r: &McpRef) -> McpProjection {
    McpProjection::Token(format!(
        "{}_{}",
        segment_display(&r.server),
        segment_display(&r.tool)
    ))
}

/// Project canonical MCP refs to harness-native tokens for emission.
///
/// Unsupported refs are omitted from `tokens` (never broaden permissions) and returned
/// in `unsupported` for lossiness reporting.
pub(crate) fn project_mcp_ref_tokens(
    refs: &[McpRef],
    harness: &str,
) -> (Vec<String>, Vec<(String, McpUnsupportedReason)>) {
    let mut seen = std::collections::HashSet::new();
    let mut tokens = Vec::new();
    let mut unsupported = Vec::new();

    for mcp_ref in refs {
        match project_mcp_ref(mcp_ref, harness) {
            McpProjection::Token(token) => {
                if seen.insert(token.clone()) {
                    tokens.push(token);
                }
            }
            McpProjection::Unsupported(reason) => {
                unsupported.push((mcp_ref.to_canonical(), reason));
            }
        }
    }

    (tokens, unsupported)
}

/// Project MCP refs for harness emission and report each unsupported ref.
///
/// Unsupported refs are omitted from the returned tokens (never broaden permissions).
pub(crate) fn project_mcp_refs_for_emission(
    refs: &[McpRef],
    harness: &str,
    mut on_unsupported: impl FnMut(&str, McpUnsupportedReason),
) -> Vec<String> {
    let (tokens, unsupported) = project_mcp_ref_tokens(refs, harness);
    for (canonical, reason) in unsupported {
        on_unsupported(&canonical, reason);
    }
    tokens
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

    fn is_whole_server(r: &McpRef) -> bool {
        matches!(&r.tool, McpSegment::Any) && matches!(&r.server, McpSegment::Named(_))
    }

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
        assert!(is_whole_server(&parsed));
        assert_eq!(parsed.to_canonical(), "mcp(context7/*)");

        let per_tool = try_parse_mcp_tool_name("mcp(github/delete_repo)").unwrap();
        assert!(!is_whole_server(&per_tool));
        assert_eq!(per_tool.to_canonical(), "mcp(github/delete_repo)");
    }

    #[test]
    fn parse_foreign_claude_mcp_wire_tokens() {
        use crate::dialect::Dialect;

        let cases = [
            (
                "mcp__github__create_issue",
                McpRef {
                    server: McpSegment::Named("github".into()),
                    tool: McpSegment::Named("create_issue".into()),
                },
            ),
            (
                "mcp__GitHub__CreateIssue",
                McpRef {
                    server: McpSegment::Named("GitHub".into()),
                    tool: McpSegment::Named("CreateIssue".into()),
                },
            ),
            (
                "mcp__context7__*",
                McpRef {
                    server: McpSegment::Named("context7".into()),
                    tool: McpSegment::Any,
                },
            ),
            (
                "mcp__github",
                McpRef {
                    server: McpSegment::Named("github".into()),
                    tool: McpSegment::Any,
                },
            ),
            (
                "mcp__*",
                McpRef {
                    server: McpSegment::Any,
                    tool: McpSegment::Any,
                },
            ),
        ];

        for (token, expected) in cases {
            let parsed = parse_foreign_mcp_token(token, Dialect::Claude).unwrap();
            assert_eq!(parsed, expected, "token: {token}");
            assert_eq!(
                try_parse_mcp_tool_name(&parsed.to_canonical()).unwrap(),
                expected,
                "round-trip: {token}"
            );
        }
    }

    #[test]
    fn parse_foreign_cursor_mcp_tokens() {
        use crate::dialect::Dialect;

        let cases = [
            (
                "Mcp(github:create_issue)",
                McpRef {
                    server: McpSegment::Named("github".into()),
                    tool: McpSegment::Named("create_issue".into()),
                },
            ),
            (
                "Mcp(server:*)",
                McpRef {
                    server: McpSegment::Named("server".into()),
                    tool: McpSegment::Any,
                },
            ),
            (
                "Mcp(*:tool)",
                McpRef {
                    server: McpSegment::Any,
                    tool: McpSegment::Named("tool".into()),
                },
            ),
            (
                "Mcp(*:*)",
                McpRef {
                    server: McpSegment::Any,
                    tool: McpSegment::Any,
                },
            ),
        ];

        for (token, expected) in cases {
            let parsed = parse_foreign_mcp_token(token, Dialect::Cursor).unwrap();
            assert_eq!(parsed, expected, "token: {token}");
        }
    }

    #[test]
    fn parse_foreign_mcp_token_rejects_non_mcp_and_canonical() {
        use crate::dialect::Dialect;

        for token in ["Read", "Bash(git *)", "mcp(github/tool)", "not_mcp__x"] {
            assert!(
                parse_foreign_mcp_token(token, Dialect::Claude).is_none(),
                "expected None for {token}"
            );
        }
        assert!(parse_foreign_mcp_token("mcp__github__tool", Dialect::Cursor).is_none());
    }

    #[test]
    fn parse_claude_mcp_wire_token_rejects_non_ascii_near_prefix_without_panic() {
        use crate::dialect::Dialect;

        for token in ["mcp_é", "ab🚀cd"] {
            assert!(
                parse_foreign_mcp_token(token, Dialect::Claude).is_none(),
                "expected None for {token:?}"
            );
        }
    }

    #[test]
    fn parse_foreign_cursor_mcp_token_namespaced_server() {
        use crate::dialect::Dialect;

        let token = "Mcp(plugin:context7:context7:create_issue)";
        let parsed = parse_foreign_mcp_token(token, Dialect::Cursor).unwrap();
        assert_eq!(
            parsed,
            McpRef {
                server: McpSegment::Named("plugin:context7:context7".into()),
                tool: McpSegment::Named("create_issue".into()),
            }
        );
        let canonical = parsed.to_canonical();
        assert_eq!(canonical, "mcp(plugin:context7:context7/create_issue)");
        let lifted = try_parse_mcp_tool_name(&canonical).unwrap();
        assert!(!is_whole_server(&lifted));
        assert_eq!(lifted, parsed);
    }

    fn assert_token(r: &McpRef, harness: &str, expected: &str) {
        assert_eq!(
            project_mcp_ref(r, harness),
            McpProjection::Token(expected.to_string()),
            "harness={harness}, ref={}",
            r.to_canonical()
        );
    }

    fn assert_unsupported(r: &McpRef, harness: &str, reason: McpUnsupportedReason) {
        assert_eq!(
            project_mcp_ref(r, harness),
            McpProjection::Unsupported(reason),
            "harness={harness}, ref={}",
            r.to_canonical()
        );
    }

    #[test]
    fn project_mcp_ref_claude_matrix() {
        let per_tool = parse("GitHub/CreateIssue");
        assert_token(&per_tool, "claude", "mcp__GitHub__CreateIssue");

        let whole_server = parse("GitHub/*");
        assert_token(&whole_server, "claude", "mcp__GitHub__*");

        let server_shorthand = parse("GitHub");
        assert_token(&server_shorthand, "claude", "mcp__GitHub__*");

        let cross_server = parse("*/CreateIssue");
        assert_unsupported(
            &cross_server,
            "claude",
            McpUnsupportedReason::CrossServerTool,
        );

        let global = parse("*/*");
        assert_token(&global, "claude", "mcp__*");

        let namespaced = parse("plugin:context7:context7/echo");
        assert_token(&namespaced, "claude", "mcp__plugin:context7:context7__echo");
    }

    #[test]
    fn project_mcp_ref_cursor_matrix() {
        let per_tool = parse("GitHub/CreateIssue");
        assert_token(&per_tool, "cursor", "Mcp(GitHub:CreateIssue)");

        let whole_server = parse("GitHub/*");
        assert_token(&whole_server, "cursor", "Mcp(GitHub:*)");

        let server_shorthand = parse("GitHub");
        assert_token(&server_shorthand, "cursor", "Mcp(GitHub:*)");

        let cross_server = parse("*/CreateIssue");
        assert_token(&cross_server, "cursor", "Mcp(*:CreateIssue)");

        let global = parse("*/*");
        assert_token(&global, "cursor", "Mcp(*:*)");

        let namespaced = parse("plugin:context7:context7/echo");
        assert_token(&namespaced, "cursor", "Mcp(plugin:context7:context7:echo)");
    }

    #[test]
    fn project_mcp_ref_opencode_matrix() {
        let per_tool = parse("GitHub/CreateIssue");
        assert_token(&per_tool, "opencode", "GitHub_CreateIssue");

        let whole_server = parse("GitHub/*");
        assert_token(&whole_server, "opencode", "GitHub_*");

        let server_shorthand = parse("GitHub");
        assert_token(&server_shorthand, "opencode", "GitHub_*");

        let cross_server = parse("*/CreateIssue");
        assert_token(&cross_server, "opencode", "*_CreateIssue");

        let global = parse("*/*");
        assert_token(&global, "opencode", "*_*");

        let namespaced = parse("plugin:context7:context7/echo");
        assert_token(&namespaced, "opencode", "plugin:context7:context7_echo");
    }

    #[test]
    fn project_mcp_ref_codex_all_unsupported() {
        let forms = [
            parse("GitHub/CreateIssue"),
            parse("GitHub/*"),
            parse("GitHub"),
            parse("*/CreateIssue"),
            parse("*/*"),
            parse("plugin:context7:context7/echo"),
        ];

        for r in forms {
            assert_unsupported(&r, "codex", McpUnsupportedReason::PerToolNeedsServerConfig);
        }
    }

    #[test]
    fn project_mcp_ref_pi_all_unsupported() {
        let forms = [
            parse("GitHub/CreateIssue"),
            parse("GitHub/*"),
            parse("GitHub"),
            parse("*/CreateIssue"),
            parse("*/*"),
            parse("plugin:context7:context7/echo"),
        ];

        for r in forms {
            assert_unsupported(&r, "pi", McpUnsupportedReason::HarnessDropsMcp);
        }
    }

    #[test]
    fn project_mcp_ref_unknown_harness_passthrough_canonical() {
        let r = parse("GitHub/CreateIssue");
        assert_token(&r, "future", "mcp(GitHub/CreateIssue)");
        assert_token(&r, "MARS", "mcp(GitHub/CreateIssue)");
    }
}
