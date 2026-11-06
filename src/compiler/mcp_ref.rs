//! Projection of schema-owned MCP references to harness-native names.

use mars_schema::mcp::{McpRef, McpSegment};

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
