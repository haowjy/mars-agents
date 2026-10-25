use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::sync::target::TargetItem;
use crate::target::ConfigEntry;
use mars_schema::HarnessId;

use super::{StandardTreeOutput, TreeProfile};

pub(super) static PROFILE: TreeProfile = TreeProfile {
    format: &crate::plugin::CURSOR_TREE_SPEC,
    harness: HarnessId::Cursor,
    agents: Some(preserve_agent),
    native_model_aliases: &[],
    plugin_root_token: Some("${CURSOR_PLUGIN_ROOT}"),
    check_hook,
    standard_output: Some(StandardTreeOutput {
        manifest_path: ".cursor-plugin/plugin.json",
        manifest_description: "Cursor plugin manifest",
    }),
    write_config: write_config_entries,
    finish: super::finish_standard_tree,
};

fn preserve_agent(
    lowered: &str,
    _source: &crate::frontmatter::Frontmatter,
    _item: &TargetItem,
    _diag: &mut DiagnosticCollector,
) -> Result<String, MarsError> {
    Ok(lowered.to_string())
}

fn check_hook(_hook: &crate::compiler::hooks::ParsedHookItem, _diag: &mut DiagnosticCollector) {}

fn write_config_entries(
    _profile: &TreeProfile,
    root: &std::path::Path,
    entries: &[ConfigEntry],
    _diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let (mcp, hooks) = super::standard_config_entries(entries);

    if !mcp.is_empty() {
        crate::target::cursor::write_cursor_mcp_json(&root.join("mcp.json"), &mcp)?;
    }
    if !hooks.is_empty() {
        crate::target::cursor::write_cursor_hooks_json(&root.join("hooks/hooks.json"), &hooks)?;
    }
    Ok(())
}
