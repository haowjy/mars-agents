//! Projection of schema-owned MCP references to harness-native names.

use mars_schema::mcp::{McpRef, McpSegment};

#[cfg(test)]
use mars_schema::mcp::{
    ForeignMcpDialect, McpRefParseError, parse_foreign_mcp_token as parse_schema_mcp_token,
    parse_mcp_ref, try_parse_mcp_tool_name,
};

fn segment_display(segment: &McpSegment) -> String {
    match segment {
        McpSegment::Any => "*".to_owned(),
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
pub(crate) fn project_mcp_ref(r: &McpRef, harness: mars_schema::HarnessId) -> McpProjection {
    let descriptor = crate::compiler::harness_descriptor::descriptor(harness);
    match descriptor.mcp_projection {
        crate::compiler::harness_descriptor::McpProjectionPolicy::ClaudeToolList => {
            project_claude(r)
        }
        crate::compiler::harness_descriptor::McpProjectionPolicy::CursorToolList => {
            project_cursor(r)
        }
        crate::compiler::harness_descriptor::McpProjectionPolicy::OpenCodeToolList => {
            project_opencode(r)
        }
        crate::compiler::harness_descriptor::McpProjectionPolicy::CodexServerConfig => {
            McpProjection::Unsupported(McpUnsupportedReason::PerToolNeedsServerConfig)
        }
        crate::compiler::harness_descriptor::McpProjectionPolicy::Unsupported => {
            McpProjection::Unsupported(McpUnsupportedReason::HarnessDropsMcp)
        }
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
            let parsed = parse_schema_mcp_token(token, ForeignMcpDialect::Claude).unwrap();
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
            let parsed = parse_schema_mcp_token(token, ForeignMcpDialect::Cursor).unwrap();
            assert_eq!(parsed, expected, "token: {token}");
        }
    }

    #[test]
    fn parse_foreign_mcp_token_rejects_non_mcp_and_canonical() {
        for token in ["Read", "Bash(git *)", "mcp(github/tool)", "not_mcp__x"] {
            assert!(
                parse_schema_mcp_token(token, ForeignMcpDialect::Claude).is_none(),
                "expected None for {token}"
            );
        }
        assert!(parse_schema_mcp_token("mcp__github__tool", ForeignMcpDialect::Cursor).is_none());
    }

    #[test]
    fn parse_claude_mcp_wire_token_rejects_non_ascii_near_prefix_without_panic() {
        for token in ["mcp_é", "ab🚀cd"] {
            assert!(
                parse_schema_mcp_token(token, ForeignMcpDialect::Claude).is_none(),
                "expected None for {token:?}"
            );
        }
    }

    #[test]
    fn parse_foreign_cursor_mcp_token_namespaced_server() {
        let token = "Mcp(plugin:context7:context7:create_issue)";
        let parsed = parse_schema_mcp_token(token, ForeignMcpDialect::Cursor).unwrap();
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
        let harness_kind =
            crate::compiler::harness_descriptor::descriptor_for_canonical_id(harness)
                .expect("test harness exists")
                .kind;
        assert_eq!(
            project_mcp_ref(r, harness_kind),
            McpProjection::Token(expected.to_string()),
            "harness={harness}, ref={}",
            r.to_canonical()
        );
    }

    fn assert_unsupported(r: &McpRef, harness: &str, reason: McpUnsupportedReason) {
        let harness_kind =
            crate::compiler::harness_descriptor::descriptor_for_canonical_id(harness)
                .expect("test harness exists")
                .kind;
        assert_eq!(
            project_mcp_ref(r, harness_kind),
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
}
