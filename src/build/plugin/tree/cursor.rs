use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::target::ConfigEntry;
use mars_schema::HarnessId;
use mars_schema::plugin_metadata::CURSOR_ROOT_FILE;

use super::super::spec::{
    Finish, NameRule, PluginHarnessSpec, PluginRoot, RootFile, TreeProfile,
    valid_cursor_plugin_name,
};

pub(in crate::build::plugin) static PROFILE: TreeProfile = TreeProfile {
    key: "cursor",
    harness: HarnessId::Cursor,
    root_file: RootFile::Marketplace {
        file: CURSOR_ROOT_FILE,
        source_prefix: "",
        name_rule: NameRule {
            validate: valid_cursor_plugin_name,
            message: "Cursor marketplace names must contain only lowercase letters, digits, dots, or hyphens and start and end with a letter or digit",
        },
    },
    dropped_agent_fields: &[],
    approximate_lowers_to_dropped: true,
    agents: None,
    native_model_aliases: &[],
    plugin_root: PluginRoot::Token("${CURSOR_PLUGIN_ROOT}"),
    check_hook,
    write_config: write_config_entries,
    finish: Finish::StandardManifest {
        path: ".cursor-plugin/plugin.json",
        description: "Cursor plugin manifest",
    },
};

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
        super::write_pretty_json(
            &root.join("mcp.json"),
            &serde_json::json!({
                "mcpServers": super::render_servers(&crate::target::cursor::CursorAdapter, &mcp)
            }),
            "Cursor MCP configuration",
        )?;
    }
    if !hooks.is_empty() {
        crate::target::cursor::write_cursor_hooks_json(&root.join("hooks/hooks.json"), &hooks)?;
    }
    Ok(())
}
