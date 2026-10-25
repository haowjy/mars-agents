use crate::config::PluginHarness;
use crate::diagnostic::DiagnosticCollector;
use crate::lock::ItemKind;
use crate::sync::target::TargetState;

use super::PluginTree;
use super::tree::BuiltTree;

pub(super) fn emit(
    harnesses: &[PluginHarness],
    target: &TargetState,
    built_trees: &[BuiltTree],
    diag: &mut DiagnosticCollector,
) {
    let claude = built_trees
        .iter()
        .find(|tree| tree.tree() == PluginTree::Claude);
    if claude.is_some() {
        warn_hooks_without_claude_fragment(target.exported_hooks(), diag);
    }
    if harnesses.contains(&PluginHarness::Codex)
        && let Some(claude) = claude
    {
        let agents = count_items(target, ItemKind::Agent);
        emit_codex_warnings(agents, &claude.components, diag);
    }
    if count_items(target, ItemKind::BootstrapDoc) > 0 {
        diag.info(
            "plugin-bootstrap-skipped",
            "bootstrap documents have no plugin surface and were skipped",
        );
    }
}

fn count_items(target: &TargetState, kind: ItemKind) -> usize {
    target
        .items
        .values()
        .filter(|item| item.id.kind == kind)
        .count()
}

fn warn_hooks_without_claude_fragment(
    exported_hooks: &[crate::compiler::hooks::ParsedHookItem],
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

fn emit_codex_warnings(
    agents: usize,
    summary: &super::PluginComponents,
    diag: &mut DiagnosticCollector,
) {
    if agents > 0 {
        diag.warn(
            "plugin-codex-agents-dropped",
            format!(
                "codex: {} agents are present in the shared Claude tree but are not loaded by Codex plugins",
                agents
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
