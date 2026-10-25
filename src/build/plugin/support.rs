use crate::compiler::hooks::ParsedHookItem;
use crate::config::PluginHarness;
use crate::diagnostic::DiagnosticCollector;

use super::tree::BuiltTree;

pub(super) fn emit(
    harnesses: &[PluginHarness],
    exported_hooks: &[ParsedHookItem],
    built_trees: &[BuiltTree],
    diag: &mut DiagnosticCollector,
) {
    let claude = built_trees
        .iter()
        .find(|tree| tree.profile.descriptor().canonical_id == "claude");
    if claude.is_some() {
        warn_hooks_without_claude_fragment(exported_hooks, diag);
    }
    if harnesses.contains(&PluginHarness::Codex)
        && let Some(claude) = claude
    {
        emit_codex_warnings(&claude.components, diag);
    }
}

fn warn_hooks_without_claude_fragment(
    exported_hooks: &[ParsedHookItem],
    diag: &mut DiagnosticCollector,
) {
    for hook in exported_hooks {
        if hook.def.targets.contains_key(".claude") {
            continue;
        }
        diag.warn(
            "plugin-hook-no-claude-fragment",
            format!(
                "hook `{}` from `{}` has no Claude fragment and is omitted from the shared plugin tree",
                hook.def.name, hook.source_name
            ),
        );
    }
}

fn emit_codex_warnings(summary: &super::PluginComponents, diag: &mut DiagnosticCollector) {
    if summary.agents > 0 {
        diag.warn(
            "plugin-codex-agents-dropped",
            format!(
                "codex: {} agents are present in the shared Claude tree but are not loaded by Codex plugins",
                summary.agents
            ),
        );
    }
    if summary.mcp_servers > 0 {
        diag.warn(
            "plugin-codex-mcp-unavailable",
            format!(
                "codex: {} MCP servers are listed from the shared Claude tree but are not callable in Codex 0.164.0",
                summary.mcp_servers
            ),
        );
    }
}
