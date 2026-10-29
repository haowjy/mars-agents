//! Projection of schema-owned MCP references to harness-native names.

use mars_schema::mcp::{McpRef, McpSegment};

#[cfg(test)]
use mars_schema::mcp::parse_mcp_ref;

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

    fn parse(payload: &str) -> McpRef {
        parse_mcp_ref(payload).unwrap()
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
