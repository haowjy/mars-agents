use crate::compiler::agents::HarnessKind;
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::sync::target::TargetItem;
use crate::target::ConfigEntry;

use super::super::spec::{
    Finish, NameRule, PluginHarnessSpec, PluginRoot, RootFile, TreeProfile,
    valid_cursor_plugin_name,
};
use crate::plugin_metadata::CURSOR_ROOT_FILE;

pub(in crate::build::plugin) static PROFILE: TreeProfile = TreeProfile {
    key: "cursor",
    harness: HarnessKind::Cursor,
    root_file: RootFile::Marketplace {
        file: CURSOR_ROOT_FILE,
        catalog_directory: ".cursor-plugin",
        source_prefix: "",
        name_rule: NameRule {
            validate: valid_cursor_plugin_name,
            message: "Cursor marketplace names must contain only lowercase letters, digits, dots, or hyphens and start and end with a letter or digit",
        },
    },
    dropped_agent_fields: &[],
    approximate_lowers_to_dropped: true,
    agents: Some(preserve_agent),
    native_model_aliases: &[],
    plugin_root: PluginRoot::Token("${CURSOR_PLUGIN_ROOT}"),
    check_hook,
    write_config: write_config_entries,
    finish: Finish::StandardManifest {
        path: ".cursor-plugin/plugin.json",
        description: "Cursor plugin manifest",
    },
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
    _harnesses: &[&'static PluginHarnessSpec],
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
